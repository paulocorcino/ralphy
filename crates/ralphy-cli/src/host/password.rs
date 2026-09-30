//! Password sign-in (ADR-0067 §3): the operator's password is used once, to add
//! this computer's peer key on a host that refuses every key, then dropped.
//!
//! `ssh` asks for the password through its askpass program, and that program is
//! this `ralphy` binary: started by `ssh` with `RALPHY_ASKPASS` set, it reads the
//! password from the flow that started `ssh`, over a loopback socket guarded by
//! a one-time nonce. So the password never appears in an argv, an environment
//! variable or a file. `OpenSSH_for_Windows_9.5p2` started with no console
//! honours `SSH_ASKPASS_REQUIRE=force` with no `DISPLAY` (measured 2026-09-30).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use ralphy_daemon::peer::key::key_body;
use zeroize::Zeroizing;

use super::pair::os_of;
use super::shell::{render, HostOp, HostOs};
use super::ssh::{classify, ssh_error, HostOutput, HostShell, SshFailure};

/// Set on the `ssh` child: `<port>:<nonce>` of the socket that answers.
pub(crate) const ASKPASS_ENV: &str = "RALPHY_ASKPASS";

/// How long either side of the socket waits for the other.
const SOCKET_WAIT: Duration = Duration::from_secs(10);

/// The longest nonce and prompt the socket reads.
const MAX_REQUEST: u64 = 4096;

/// The operator's password. It is erased from memory when dropped and is never
/// printed by `Debug`.
pub(crate) struct Password(Zeroizing<String>);

impl Password {
    /// Read the password from `input` to its end. One final line break is not
    /// part of it.
    pub(crate) fn read(mut input: impl Read) -> Result<Password> {
        let mut text = Zeroizing::new(String::new());
        input
            .read_to_string(&mut text)
            .context("reading the password from standard input")?;
        let len = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .map_or(text.len(), str::len);
        text.truncate(len);
        if text.is_empty() {
            bail!("the password on standard input is empty");
        }
        Ok(Password(text))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(<hidden>)")
    }
}

/// Whether `prompt` asks for the account password, as opposed to a second
/// factor or a question.
pub(crate) fn is_password_prompt(prompt: &str) -> bool {
    prompt.to_ascii_lowercase().contains("password")
}

/// What the answering socket does with each request, apart from the socket.
#[derive(Default)]
struct Answers {
    /// The prompt the password answered.
    served: Option<String>,
    /// The first prompt that was not answered, and not a repeat of `served`.
    unhandled: Option<String>,
}

impl Answers {
    /// Whether to send the password for `prompt`. The same prompt a second time
    /// means the host refused the password: it is not sent again, so a wrong
    /// password costs the account one failed attempt, not several.
    fn decide(&mut self, prompt: &str) -> bool {
        match &self.served {
            None if is_password_prompt(prompt) => {
                self.served = Some(prompt.to_string());
                true
            }
            Some(first) if first == prompt => false,
            _ => {
                self.unhandled.get_or_insert_with(|| prompt.to_string());
                false
            }
        }
    }
}

/// Equal bytes, compared in a time that does not depend on where they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Run `run` with the environment that makes `ssh` ask this process for
/// `password`, and answer it until `run` returns. Returns what `run` returned
/// and the first prompt that was not answered.
pub(crate) fn serve<T>(
    password: &Password,
    run: impl FnOnce(&[(String, String)]) -> Result<T>,
) -> Result<(T, Option<String>)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .context("opening the loopback socket for the password")?;
    listener
        .set_nonblocking(true)
        .context("setting the password socket to non-blocking")?;
    let port = listener
        .local_addr()
        .context("reading the password socket")?
        .port();
    let nonce = ralphy_daemon::auth::generate_token();
    let exe = std::env::current_exe().context("finding this ralphy binary")?;
    let mut env = vec![
        ("SSH_ASKPASS".to_string(), exe.display().to_string()),
        ("SSH_ASKPASS_REQUIRE".to_string(), "force".to_string()),
        (ASKPASS_ENV.to_string(), format!("{port}:{nonce}")),
    ];
    // OpenSSH before 8.4 ignores SSH_ASKPASS_REQUIRE and uses askpass only
    // when DISPLAY is set.
    if std::env::var_os("DISPLAY").is_none() {
        env.push(("DISPLAY".to_string(), ":0".to_string()));
    }
    let answers = Mutex::new(Answers::default());
    let stop = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if let Err(e) = answer(stream, nonce.as_bytes(), password, &answers) {
                            tracing::debug!(error = %format!("{e:#}"), "a password request failed");
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "the password socket stopped");
                        return;
                    }
                }
            }
        });
        let result = run(&env);
        stop.store(true, Ordering::Relaxed);
        result
    });
    let unhandled = answers
        .into_inner()
        .map_err(|_| anyhow::anyhow!("the password socket thread panicked"))?
        .unhandled;
    Ok((result?, unhandled))
}

/// One askpass request: `<nonce>\n<prompt>`. The password is written back only
/// for the right nonce and a prompt [`Answers::decide`] accepts.
fn answer(
    mut stream: TcpStream,
    nonce: &[u8],
    password: &Password,
    answers: &Mutex<Answers>,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(SOCKET_WAIT))?;
    stream.set_write_timeout(Some(SOCKET_WAIT))?;
    let mut request = Vec::new();
    (&mut stream)
        .take(MAX_REQUEST)
        .read_to_end(&mut request)
        .context("reading the password request")?;
    let (given, prompt) = match request.iter().position(|b| *b == b'\n') {
        Some(at) => (&request[..at], &request[at + 1..]),
        None => (&request[..], &b""[..]),
    };
    if !same(given, nonce) {
        bail!("a password request came with the wrong nonce");
    }
    let prompt = String::from_utf8_lossy(prompt);
    let send = answers
        .lock()
        .map_err(|_| anyhow::anyhow!("the password answers are poisoned"))?
        .decide(prompt.trim());
    if send {
        stream.write_all(password.expose().as_bytes())?;
    }
    stream.shutdown(Shutdown::Both)?;
    Ok(())
}

/// The askpass side, run when `ssh` starts this binary with `RALPHY_ASKPASS`:
/// ask the flow for the answer to `prompt` and print it. An error means no
/// answer, so `ssh` stops asking.
pub(crate) fn askpass(env: &str, prompt: &str) -> Result<()> {
    let reply = request(env, prompt)?;
    let mut out = std::io::stdout().lock();
    out.write_all(reply.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// Ask the socket named by `env` for the answer to `prompt`.
fn request(env: &str, prompt: &str) -> Result<Zeroizing<String>> {
    let (port, nonce) = env
        .split_once(':')
        .with_context(|| format!("{ASKPASS_ENV} is not <port>:<nonce>"))?;
    let port: u16 = port
        .parse()
        .with_context(|| format!("{ASKPASS_ENV} has no port"))?;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, SOCKET_WAIT)
        .context("connecting to the flow that holds the password")?;
    stream.set_read_timeout(Some(SOCKET_WAIT))?;
    stream.set_write_timeout(Some(SOCKET_WAIT))?;
    stream.write_all(format!("{nonce}\n{prompt}").as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Zeroizing::new(String::new());
    stream
        .read_to_string(&mut reply)
        .context("reading the password")?;
    if reply.is_empty() {
        bail!("no answer for the prompt {prompt:?}");
    }
    Ok(reply)
}

/// Sign in to `dest` with the operator's password and add `public_line`, this
/// computer's peer key, to the host's keys file. Every command is its own
/// sign-in, and the host key must already be known, as for a key.
pub(crate) fn add_peer_key(
    shell: &mut impl HostShell,
    dest: &str,
    public_line: &str,
) -> Result<()> {
    let body = key_body(public_line)
        .with_context(|| format!("the peer key line {public_line:?} has no key"))?;
    let mut run = |command: &str, stdin: &[u8]| -> Result<HostOutput> {
        let out = shell.run_with_password(command, stdin)?;
        match classify(&out) {
            None => Ok(out),
            Some(SshFailure::AuthRefused) => Err(ssh_error(
                SshFailure::PasswordRefused,
                format!("the host {dest} refused the password"),
            )),
            Some(kind) => Err(ssh_error(
                kind,
                format!("the connection to {dest} failed: {}", out.stderr.trim()),
            )),
        }
    };
    let uname = run(&render(None, &HostOp::Uname)?, b"")?;
    let os = os_of(dest, &uname, || {
        run(&render(None, &HostOp::WindowsVer)?, b"")
    })?;
    let admin = os == HostOs::Windows
        && run(&render(Some(os), &HostOp::Probe)?, b"")?
            .stdout
            .contains("S-1-5-32-544");
    let op = HostOp::AppendKey {
        admin,
        body: body.to_string(),
    };
    let line = format!("{}\n", public_line.trim());
    let added = run(&render(Some(os), &op)?, line.as_bytes())?;
    if !added.ok() {
        bail!(
            "adding this computer's key on {dest} failed: {}",
            added.stderr.trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
