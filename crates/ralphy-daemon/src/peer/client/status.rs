//! What a probe learned about a peer, and the sentence the operator reads for it.

use std::time::Duration;

/// What a probe learned about a peer. Computed fresh on every request and never
/// persisted — a descriptor is a claim, liveness is an observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerStatus {
    Reachable,
    /// The peer refused the token. `tunnel` is true when the peer is reached
    /// through a tunnel, where the `--peer-store` advice for a WSL distro does
    /// not apply.
    Unauthorized {
        tunnel: bool,
    },
    VersionMismatch {
        theirs: u32,
        ours: u32,
    },
    /// The peer's WSL distro is not running, so neither is the daemon inside it.
    /// Distinct from [`PeerStatus::Unreachable`] because it is the ordinary case,
    /// not a fault: WSL terminates an idle distro and the daemon goes with it.
    Asleep {
        distro: String,
    },
    Unreachable {
        why: String,
    },
    Refused {
        why: String,
    },
    /// The `ssh` that carries this peer (ADR-0067 §2) is not running; the
    /// daemon is opening it again. Not a fault: a VPN drop ends up here.
    TunnelClosed {
        host: String,
        cause: Option<String>,
        /// The last line the `ssh` that exited wrote on its stderr.
        said: Option<String>,
        /// When the next `ssh` starts, if it waits because the ones before
        /// kept exiting.
        retry_in: Option<Duration>,
    },
    /// The tunnel is open, but the daemon on the other end does not answer.
    TunnelSilent {
        host: String,
        why: String,
    },
}

impl PeerStatus {
    /// The short state string the workbench renders and groups by.
    pub fn state(&self) -> &'static str {
        match self {
            PeerStatus::Reachable => "reachable",
            PeerStatus::Unauthorized { .. } => "unauthorized",
            PeerStatus::VersionMismatch { .. } => "version-mismatch",
            PeerStatus::Asleep { .. } => "asleep",
            PeerStatus::Unreachable { .. } => "unreachable",
            PeerStatus::Refused { .. } => "refused",
            PeerStatus::TunnelClosed { .. } => "tunnel-closed",
            PeerStatus::TunnelSilent { .. } => "tunnel-silent",
        }
    }

    /// Operator-facing sentences naming WHICH environment is in this state and
    /// what to do about it. The environment label is what makes a fleet
    /// diagnosis legible — "unauthorized" alone names no machine. The workbench
    /// shows them as they are, so they follow ADR-0065 (#432): a capital letter,
    /// a final period, and the remedy as a sentence of its own.
    pub fn diagnosis(&self, environment: &str) -> String {
        match self {
            PeerStatus::Reachable => format!("{environment} is connected."),
            PeerStatus::Unauthorized { tunnel: false } => {
                format!("{environment} refused the token. Restart its daemon with --peer-store.")
            }
            PeerStatus::Unauthorized { tunnel: true } => format!(
                "{environment} refused the token. Another daemon may answer on this port, \
                 or the token changed. Add the host again."
            ),
            PeerStatus::VersionMismatch { theirs, ours } => format!(
                "{environment} uses protocol {theirs}, not {ours}. Upgrade the older Ralphy."
            ),
            PeerStatus::Asleep { distro } => {
                format!("{environment} is asleep. Wake the distro {distro}.")
            }
            PeerStatus::Unreachable { why } => {
                format!("{environment} did not answer: {why}. Start its daemon.")
            }
            PeerStatus::Refused { why } => {
                format!("Ralphy did not connect to {environment}: {why}.")
            }
            PeerStatus::TunnelClosed {
                host,
                cause: Some(cause),
                ..
            } => format!("The tunnel to {host} is closed. Ralphy could not open it: {cause}."),
            PeerStatus::TunnelClosed {
                host,
                cause: None,
                said,
                retry_in,
            } => {
                let said = said
                    .as_ref()
                    .map(|said| format!(": ssh said \"{said}\""))
                    .unwrap_or_default();
                let next = match retry_in {
                    Some(wait) => format!("Ralphy tries again in {}.", in_words(*wait)),
                    None => "Ralphy is opening it again.".to_owned(),
                };
                format!("The tunnel to {host} is closed{said}. {next}")
            }
            PeerStatus::TunnelSilent { host, why } => format!(
                "The tunnel to {host} is open, but its daemon does not answer: {why}. Start it."
            ),
        }
    }
}

/// A wait as a person says it: seconds under a minute, else whole minutes,
/// rounded up.
fn in_words(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    match secs {
        1 => "1 second".to_owned(),
        2..=59 => format!("{secs} seconds"),
        _ => match secs.div_ceil(60) {
            1 => "1 minute".to_owned(),
            minutes => format!("{minutes} minutes"),
        },
    }
}
