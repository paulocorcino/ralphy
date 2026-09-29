//! `ralphy host`: make a computer reached over SSH a peer, check it, and take
//! it out again (ADR-0067). Sign-in uses a key or an agent only; Ralphy never
//! prompts. The workbench can run these as verbs, so each one prints what it
//! did and exits non-zero when the work is incomplete.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use ralphy_daemon::{auth, identity, pidfile};

mod checks;
mod pair;
mod shell;
mod ssh;

#[derive(Subcommand)]
pub(crate) enum HostCommand {
    /// Add a computer you reach over SSH as a peer: its repos appear in the
    /// workbench. Sign-in uses your SSH config's key or agent, or Ralphy's own
    /// key.
    Add {
        /// An alias from your SSH config, or `user@host`.
        destination: String,
        /// The name to give the host's daemon when it has none.
        #[arg(long)]
        name: Option<String>,
    },
    /// Show what a computer needs before `ralphy host add`. Changes nothing.
    Check {
        /// An alias from your SSH config, or `user@host`.
        destination: String,
    },
    /// Remove a host: delete this computer's key line on the host and forget it
    /// here. The host's daemon, repos and runs are not changed.
    Remove {
        /// The host's name, as the workbench shows it.
        name: String,
        /// Also change the host's access token. Every other computer connected
        /// to the host is disconnected.
        #[arg(long)]
        rotate_token: bool,
    },
}

pub(crate) fn run(cmd: &HostCommand) -> Result<()> {
    let store = auth::store_dir()?;
    let me = identity::load_from(&identity::daemon_toml_path()?)?;
    let local = pair::Local {
        store: &store,
        daemon_id: me.as_ref().map(|i| i.id.to_string()),
        name: me.as_ref().map(|i| i.name.clone()),
        port: crate::daemon::port_from_args(&pidfile::read_args_in(&store)),
    };
    let mut out = std::io::stdout();
    match cmd {
        HostCommand::Add { destination, name } => {
            let mut shell = ssh::Ssh::new(destination)?;
            let keygen_program = shell.program().to_path_buf();
            let comment = format!("ralphy-peer@{}", local.name.as_deref().unwrap_or("ralphy"));
            let descriptor = pair::add(
                &mut shell,
                &local,
                destination,
                name.as_deref(),
                |path| keygen(&keygen_program, &comment, path),
                |p| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok(),
                &mut out,
            )?;
            nudge(local.port, &descriptor.daemon_id, &mut out)
        }
        HostCommand::Check { destination } => {
            let mut shell = ssh::Ssh::new(destination)?;
            pair::check(&mut shell, &local, destination, &mut out).map(|_| ())
        }
        HostCommand::Remove { name, rotate_token } => {
            let host = pair::find_host(&store, name)?;
            let destination = host
                .tunnel
                .as_ref()
                .map(|t| t.destination.clone())
                .with_context(|| format!("{name} is not reached through a tunnel"))?;
            let mut shell = ssh::Ssh::new(&destination)?;
            pair::remove(&mut shell, &store, &host, *rotate_token, &mut out)
        }
    }
}

/// The `ssh-keygen` next to `ssh`, else the first one on `PATH`.
fn ssh_keygen(ssh: &Path) -> Option<PathBuf> {
    let name = match ssh.extension() {
        Some(ext) => format!("ssh-keygen.{}", ext.to_string_lossy()),
        None => "ssh-keygen".to_string(),
    };
    Some(ssh.with_file_name(name))
        .filter(|p| p.is_file())
        .or_else(|| {
            ralphy_proc_util::find_program(
                "ssh-keygen",
                std::env::var_os("PATH"),
                std::env::var_os("PATHEXT"),
            )
        })
}

/// Generate the peer key at `path`: ed25519, no passphrase, so the tunnel can
/// reconnect with no person present.
fn keygen(ssh: &Path, comment: &str, path: &Path) -> Result<()> {
    let program = ssh_keygen(ssh).context("no ssh-keygen program found: install OpenSSH")?;
    let mut cmd = Command::new(&program);
    cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
        .arg(path);
    ralphy_proc_util::no_window(&mut cmd);
    let out = cmd
        .output()
        .with_context(|| format!("starting {}", program.display()))?;
    if !out.status.success() {
        bail!(
            "ssh-keygen failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Ask the local daemon to open the new tunnel now, and print what it says.
/// The daemon holds the tunnel, not this command, so it outlives this process.
fn nudge(port: u16, daemon_id: &str, out: &mut impl Write) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(40)))
        .timeout_recv_body(Some(Duration::from_secs(40)))
        .proxy(None)
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("http://127.0.0.1:{port}/api/fleet/nudge?daemon_id={daemon_id}");
    let mut req = agent.post(&url);
    let token = match auth::effective_token() {
        Ok(token) => token,
        Err(e) => {
            writeln!(out, "Could not read the access token of the daemon on this computer: {e:#}. The workbench opens the tunnel when it shows the host.")?;
            return Ok(());
        }
    };
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = match req.send_empty() {
        Ok(resp) => resp,
        Err(ureq::Error::Io(_)) | Err(ureq::Error::ConnectionFailed) => {
            writeln!(
                out,
                "The daemon on this computer is not running. The tunnel opens when it starts."
            )?;
            return Ok(());
        }
        Err(e) => {
            writeln!(out, "Could not ask the daemon on this computer to open the tunnel: {e}. The workbench opens it when it shows the host.")?;
            return Ok(());
        }
    };
    let status = resp.status().as_u16();
    let body: serde_json::Value = match resp.body_mut().read_json() {
        Ok(body) => body,
        Err(e) => {
            writeln!(
                out,
                "The daemon on this computer gave an answer that is not JSON ({status}): {e}."
            )?;
            return Ok(());
        }
    };
    if status != 200 {
        let why = body["error"].as_str().unwrap_or("no reason given");
        writeln!(
            out,
            "The daemon on this computer did not open the tunnel ({status}): {why}. The workbench opens it when it shows the host."
        )?;
        return Ok(());
    }
    let state = body["state"].as_str().unwrap_or("unknown");
    writeln!(out, "Tunnel: {state}")?;
    if let Some(diagnosis) = body["diagnosis"].as_str().filter(|d| !d.is_empty()) {
        writeln!(out, "{diagnosis}")?;
    }
    Ok(())
}
