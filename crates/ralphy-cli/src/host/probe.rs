//! The tunnel probe of `ralphy host add`: open the tunnel the daemon would open,
//! ask `/api/peer/hello` through it once, and close it. The descriptor is
//! written only for a tunnel that reached the right daemon.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ralphy_daemon::peer::tunnel::{ssh_program, tunnel_argv};
use ralphy_daemon::peer::TunnelSpec;

/// How long `ssh` has to sign in and open its local end.
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(100);

/// The `daemon_id` that answers through a tunnel with `spec`, or why nothing
/// did. The tunnel uses a free local port of its own, so it never meets the
/// tunnel the daemon may already hold for this host.
pub(crate) fn probe(spec: &TunnelSpec, token: Option<&str>) -> Result<String, String> {
    let ssh = ssh_program().ok_or("this computer has no ssh")?;
    let port = free_port()?;
    let spec = TunnelSpec {
        local_port: port,
        ..spec.clone()
    };
    let argv = tunnel_argv(&ssh, &spec);
    let (program, rest) = argv.split_first().ok_or("the tunnel command is empty")?;
    let mut cmd = Command::new(program);
    cmd.args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    ralphy_proc_util::no_window(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start ssh: {e}"))?;
    let answer = hello_through(&mut child, port, token);
    let said = end(child);
    // The line `ssh` printed names the refusal; the local socket error that
    // came with it says only that the connection was closed.
    answer.map_err(|why| match said {
        Some(said) => format!("ssh said: {said}"),
        None => why,
    })
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("no free local port: {e}"))
}

/// Wait for the local end, then ask the daemon who it is.
fn hello_through(child: &mut Child, port: u16, token: Option<&str>) -> Result<String, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + OPEN_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("ssh ended before the tunnel opened ({status})"));
        }
        if TcpStream::connect_timeout(&addr, POLL).is_ok() {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the tunnel did not open within {}s",
                OPEN_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(&format!("http://127.0.0.1:{port}/api/peer/hello"));
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req
        .call()
        .map_err(|e| format!("nothing answered through the tunnel: {e}"))?;
    let status = resp.status().as_u16();
    if status == 401 {
        return Err("a daemon answers there, but it refused this account's access token".into());
    }
    if status != 200 {
        return Err(format!("the daemon answered {status}"));
    }
    let body: serde_json::Value = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("the daemon's answer is not JSON: {e}"))?;
    body["daemon_id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "the daemon's answer has no daemon_id".to_string())
}

/// End the probe's `ssh` and return the last line it wrote.
fn end(mut child: Child) -> Option<String> {
    if let Err(e) = child.kill() {
        tracing::debug!(error = %e, "the probe ssh had already ended");
    }
    if let Err(e) = child.wait() {
        tracing::debug!(error = %e, "could not reap the probe ssh");
    }
    let mut text = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        if let Err(e) = stderr.read_to_string(&mut text) {
            tracing::debug!(error = %e, "could not read the probe ssh output");
        }
    }
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
}
