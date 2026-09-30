//! What the host flows print. Text mode is for a person at a terminal. JSON
//! mode, for the workbench, prints one self-describing object per line as soon
//! as it is known, because the daemon relays the output in raw chunks.

use std::io::Write;

use anyhow::Result;
use ralphy_daemon::peer::PeerDescriptor;
use serde_json::{json, Value};

use super::checks::{print_checks, CheckId, CheckStatus, HostCheck};
use super::install::Offer;
use super::shell::{render, HostOs};
use super::ssh::SshError;

pub(crate) struct Report<W: Write> {
    out: W,
    json: bool,
}

impl<W: Write> Report<W> {
    pub(crate) fn text(out: W) -> Self {
        Report { out, json: false }
    }

    pub(crate) fn json(out: W) -> Self {
        Report { out, json: true }
    }

    #[cfg(test)]
    pub(crate) fn into_inner(self) -> W {
        self.out
    }

    fn event(&mut self, value: Value) -> Result<()> {
        writeln!(self.out, "{value}")?;
        self.out.flush()?;
        Ok(())
    }

    pub(crate) fn note(&mut self, text: &str) -> Result<()> {
        if self.json {
            return self.event(json!({"event": "note", "text": text}));
        }
        writeln!(self.out, "{text}")?;
        Ok(())
    }

    /// Signed in. Text mode prints nothing: the checks header names the OS.
    pub(crate) fn connected(&mut self, os: HostOs) -> Result<()> {
        if self.json {
            return self.event(json!({"event": "connected", "os": os.label()}));
        }
        Ok(())
    }

    /// The checks, then what `ralphy host install` would send when Ralphy on
    /// the host must be installed and can be.
    pub(crate) fn checks(
        &mut self,
        dest: &str,
        os: HostOs,
        checks: &[HostCheck],
        offer: Option<&Offer>,
    ) -> Result<()> {
        if !self.json {
            writeln!(self.out, "Checks for {dest} ({}):", os.label())?;
            print_checks(checks, &mut self.out)?;
            if let Some(o) = offer {
                writeln!(
                    self.out,
                    "`ralphy host install {dest}` sends Ralphy {} for {} from {} to {}.",
                    o.version,
                    o.target,
                    o.source.describe(),
                    o.folder
                )?;
            }
            return Ok(());
        }
        for c in checks {
            let command = match &c.status {
                CheckStatus::Copy(cmd) => Some(cmd.clone()),
                CheckStatus::Fix(op) => Some(render(Some(os), op)?),
                _ => None,
            };
            self.event(json!({
                "event": "check",
                "id": c.id.key(),
                "label": c.id.label(),
                "status": c.status.key(),
                "text": c.text,
                "command": command,
            }))?;
        }
        if let Some(o) = offer {
            self.event(json!({
                "event": "install",
                "version": o.version,
                "target": o.target,
                "source": o.source.key(),
                "folder": o.folder,
            }))?;
        }
        Ok(())
    }

    /// `check`'s fix ran; `rendered` is the host command.
    pub(crate) fn fixed(&mut self, check: &HostCheck, rendered: &str) -> Result<()> {
        if self.json {
            return self.event(json!({"event": "fixed", "id": check.id.key()}));
        }
        writeln!(self.out, "Done: {rendered}")?;
        Ok(())
    }

    /// Lingering is off and the user may not turn it on without `sudo`.
    pub(crate) fn linger_needs_sudo(&mut self, user: &str) -> Result<()> {
        let command = format!("sudo loginctl enable-linger {user}");
        if self.json {
            return self.event(json!({
                "event": "check",
                "id": CheckId::Linger.key(),
                "label": CheckId::Linger.label(),
                "status": CheckStatus::Copy(String::new()).key(),
                "text": "turning on lingering needs an administrator",
                "command": command,
            }));
        }
        writeln!(
            self.out,
            "Turning on lingering needs an administrator. On the host, run: {command}"
        )?;
        Ok(())
    }

    pub(crate) fn added(&mut self, d: &PeerDescriptor, dest: &str, port: u16) -> Result<()> {
        if self.json {
            return self.event(json!({
                "event": "added",
                "name": d.name,
                "daemon_id": d.daemon_id,
                "port": port,
            }));
        }
        writeln!(
            self.out,
            "Added {} ({dest}). This computer reaches it through 127.0.0.1:{port}.",
            d.name
        )?;
        Ok(())
    }

    /// The last line of a failed flow, in JSON mode only: in text mode the
    /// error goes to stderr as for every command.
    pub(crate) fn failed(&mut self, e: &anyhow::Error) -> Result<()> {
        if !self.json {
            return Ok(());
        }
        let ssh = e.chain().find_map(|c| c.downcast_ref::<SshError>());
        let kind = ssh.map_or("other", |s| s.kind.key());
        let key_line = ssh.and_then(|s| s.key_line.as_deref());
        self.event(json!({
            "event": "failed",
            "kind": kind,
            "message": format!("{e:#}"),
            "key_line": key_line,
        }))
    }
}

#[cfg(test)]
mod tests;
