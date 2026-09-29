//! After a workbench update on Windows, take the WSL peers too (ADR-0056 §11).
//!
//! A peer announces a `nudge` only from inside WSL, so the descriptors that
//! carry one are exactly the peers this host can reach with `wsl.exe`. Each is
//! updated by its own `ralphy update`, run through `wsl.exe` from this Windows
//! process: outside the peer daemon's process tree and its cgroup, so the
//! peer's restart does not end it.

use ralphy_daemon::peer::{NudgeSpec, PeerDescriptor};
use ralphy_release::Channel;

/// The peers this host updates: the ones that announced a WSL unit.
fn wsl_peers(peers: &[PeerDescriptor]) -> Vec<&NudgeSpec> {
    peers.iter().filter_map(|p| p.nudge.as_ref()).collect()
}

/// The `wsl.exe` argv that asks `spec.distro` where the unit's binary is.
/// `-e` starts no login shell, so the answer cannot come from `PATH`.
fn exec_start_argv(spec: &NudgeSpec) -> Vec<String> {
    [
        "-d",
        &spec.distro,
        "-e",
        "systemctl",
        "--user",
        "show",
        "-p",
        "ExecStart",
        "--value",
        &spec.unit,
    ]
    .iter()
    .map(|a| a.to_string())
    .collect()
}

/// The program path out of `systemctl show -p ExecStart --value`, which prints
/// `{ path=/home/u/.local/bin/ralphy ; argv[]=… ; … }`.
fn exec_start_path(text: &str) -> Option<String> {
    let rest = text.split("path=").nth(1)?;
    let path = rest.split(" ;").next()?.trim();
    (path.starts_with('/')).then(|| path.to_string())
}

/// The `wsl.exe` argv that updates the peer. `--force` reads the releases
/// again: the peer's cache may predate the release this host just took.
fn update_argv(spec: &NudgeSpec, program: &str, channel: Channel) -> Vec<String> {
    vec![
        "-d".to_string(),
        spec.distro.clone(),
        "-e".to_string(),
        program.to_string(),
        "update".to_string(),
        "--channel".to_string(),
        channel.to_string(),
        "--force".to_string(),
    ]
}

/// Update every WSL peer, one after the other, reporting each line to `say`.
/// A failure is reported and the next peer is still updated: this runs after
/// the Windows daemon is back, so nothing here can leave the host without one.
#[cfg(windows)]
pub(super) fn update_peers(channel: Channel, say: &mut dyn FnMut(&str)) {
    let peers_dir = match ralphy_daemon::registry::repos_toml_path() {
        Ok(path) => path.with_file_name("peers"),
        Err(e) => {
            say(&format!("could not find the peer store: {e:#}"));
            return;
        }
    };
    let (peers, _rejected) = ralphy_daemon::peer::read_store(&peers_dir);
    for spec in wsl_peers(&peers) {
        say(&format!("updating the WSL peer in {}", spec.distro));
        if let Err(e) = update_one(spec, channel, say) {
            say(&format!(
                "the WSL peer in {} was not updated: {e:#}",
                spec.distro
            ));
        }
    }
}

#[cfg(windows)]
fn update_one(spec: &NudgeSpec, channel: Channel, say: &mut dyn FnMut(&str)) -> anyhow::Result<()> {
    use anyhow::{bail, Context};

    let out = wsl(&exec_start_argv(spec)).context("asking the distro for the unit's binary")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(program) = exec_start_path(&text) else {
        bail!("{} has no ExecStart path for {}", spec.distro, spec.unit);
    };

    let out = wsl(&update_argv(spec, &program, channel)).context("running ralphy update")?;
    for line in String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
    {
        if !line.trim().is_empty() {
            say(&format!("  {line}"));
        }
    }
    if !out.status.success() {
        bail!("ralphy update exited with {}", out.status);
    }
    Ok(())
}

/// Run `wsl.exe` with `argv`, with no window: this process has no console, and
/// a console program started from it would open one.
#[cfg(windows)]
fn wsl(argv: &[String]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("wsl.exe")
        .args(argv)
        .stdin(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(nudge: Option<NudgeSpec>) -> PeerDescriptor {
        PeerDescriptor {
            daemon_id: "id".into(),
            name: "name".into(),
            avatar: String::new(),
            address: "127.0.0.1".into(),
            port: 7257,
            environment: "wsl".into(),
            token: String::new(),
            protocol_version: ralphy_daemon::peer::PEER_PROTOCOL_VERSION,
            tunnel: None,
            nudge,
        }
    }

    fn ubuntu() -> NudgeSpec {
        NudgeSpec {
            distro: "Ubuntu".into(),
            unit: "ralphy-daemon.service".into(),
        }
    }

    #[test]
    fn only_a_peer_that_announced_a_wsl_unit_is_updated() {
        let peers = [descriptor(None), descriptor(Some(ubuntu()))];
        let distros: Vec<&str> = wsl_peers(&peers)
            .iter()
            .map(|s| s.distro.as_str())
            .collect();
        assert_eq!(distros, ["Ubuntu"]);
    }

    #[test]
    fn the_binary_comes_from_the_unit_not_from_path() {
        // The shape systemd 255 prints for `show -p ExecStart --value`.
        let text = "{ path=/home/paulo/.local/bin/ralphy ; argv[]=/home/paulo/.local/bin/ralphy daemon ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }\n";
        assert_eq!(
            exec_start_path(text).as_deref(),
            Some("/home/paulo/.local/bin/ralphy")
        );
        assert_eq!(exec_start_path(""), None, "a unit that does not exist");
        assert_eq!(
            exec_start_path("{ path=ralphy ; }"),
            None,
            "a relative path would be resolved against a PATH that -e does not set"
        );
    }

    #[test]
    fn the_peer_is_updated_by_its_own_binary_with_a_fresh_read() {
        let argv = update_argv(&ubuntu(), "/home/paulo/.local/bin/ralphy", Channel::Rc);
        assert_eq!(
            argv,
            [
                "-d",
                "Ubuntu",
                "-e",
                "/home/paulo/.local/bin/ralphy",
                "update",
                "--channel",
                "rc",
                "--force"
            ]
        );
        assert_eq!(
            exec_start_argv(&ubuntu())[..3],
            ["-d", "Ubuntu", "-e"],
            "both calls go to the peer's own distro"
        );
    }
}
