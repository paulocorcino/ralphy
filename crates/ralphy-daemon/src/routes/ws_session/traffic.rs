//! The traffic summary of one console channel: the bytes it sent, and how
//! many of them were a replay (on the wire, and before a gzip). It is logged
//! once, when the channel closes, so that a session over a slow or metered
//! link can be measured.

use std::io::Read;
use std::time::Instant;

use crate::device::DeviceId;
use crate::protocol::{self, Command, Frame};

/// The browser tab that opened a console socket: the holder it named, the
/// device of its browser, and whether it can decode a gzip replay. The device
/// ID is written in the summary, never the cookie value (ADR-0074 D10).
#[derive(Clone, Debug, Default)]
pub(crate) struct Tab {
    pub(crate) holder: Option<String>,
    pub(crate) device: Option<DeviceId>,
    pub(crate) gzip: bool,
    pub(crate) link: Link,
}

/// The network the page's browser reported when it opened the socket
/// (`navigator.connection`). It is a client fact: it can be false, and only
/// the log reads it (ADR-0074 D2, D5). Safari reports none of it, and then
/// every field is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Link {
    pub(crate) effective_type: Option<EffectiveType>,
    pub(crate) rtt_ms: Option<u32>,
    pub(crate) downlink_mbps: Option<f64>,
}

/// The `effectiveType` values the Network Information API defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectiveType {
    Slow2g,
    G2,
    G3,
    G4,
}

impl EffectiveType {
    /// `None` for a value the API does not define.
    pub(crate) fn parse(value: &str) -> Option<EffectiveType> {
        match value {
            "slow-2g" => Some(EffectiveType::Slow2g),
            "2g" => Some(EffectiveType::G2),
            "3g" => Some(EffectiveType::G3),
            "4g" => Some(EffectiveType::G4),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EffectiveType::Slow2g => "slow-2g",
            EffectiveType::G2 => "2g",
            EffectiveType::G3 => "3g",
            EffectiveType::G4 => "4g",
        }
    }
}

/// Which loop owns the socket: the bridge to a session this daemon hosts, or
/// the relay to a session a peer hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Local,
    Peer,
}

impl Channel {
    fn as_str(self) -> &'static str {
        match self {
            Channel::Local => "local",
            Channel::Peer => "peer",
        }
    }
}

/// The `end` of a socket that closed with no end decided by either side: the
/// link failed, or a send failed.
const DROPPED: &str = "dropped";

/// The command verb that opens every console socket, before any output.
pub(crate) const SESSION_OPEN: &str = "session-open";

/// The command verb a page sends just before it closes a console socket, or
/// when it goes hidden.
const DETACH: &str = "detach";

/// The command verb a page sends when it is visible again after a `hidden`
/// detach. Only the log reads it.
const SHOWN: &str = "shown";

/// Why the page closed a console socket, as it announced it in a `detach`
/// command. `Dormant` and `Reconnect` are followed by a reattach that replays
/// the scrollback. `Hidden` does not close the socket: the page went hidden,
/// and a browser that suspends it ends the socket without a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Leave {
    /// The console window went off the viewport and gave its terminal back.
    Dormant,
    /// The page opens a new socket at once: a resume, a take-over, or a move
    /// to a detached fence.
    Reconnect,
    /// The console window left this page.
    WindowClosed,
    /// The page went hidden: a screen lock, an app switch, or a tab switch.
    Hidden,
}

impl Leave {
    fn as_str(self) -> &'static str {
        match self {
            Leave::Dormant => "dormant",
            Leave::Reconnect => "reconnect",
            Leave::WindowClosed => "window-closed",
            Leave::Hidden => "hidden",
        }
    }

    /// The leave a `detach` command names; `None` for another verb or a reason
    /// this daemon does not know.
    fn from_command(cmd: &Command) -> Option<Leave> {
        if cmd.verb != DETACH {
            return None;
        }
        match cmd.payload.get("reason")?.as_str()? {
            "dormant" => Some(Leave::Dormant),
            "reconnect" => Some(Leave::Reconnect),
            "window-closed" => Some(Leave::WindowClosed),
            "hidden" => Some(Leave::Hidden),
            _ => None,
        }
    }
}

/// The most bytes a relayed gzip replay is decoded to, only to count them. A
/// snapshot is a screen and its scrollback, far below this.
const RAW_COUNT_CAP: u64 = 16 * 1024 * 1024;

/// Where a relayed socket is in the frames a peer sends: before the
/// `session-open` announcement, before the first terminal frame (a replay when
/// the announcement says so, gzipped when it says that too), or in the live
/// stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PeerStage {
    Open,
    FirstOutput { replay: bool, gzip: bool },
    Live,
}

/// The keys that join a summary to other records: the session this daemon
/// hosts, or the peer that hosts it; and the browser tab (the holder) and its
/// device.
struct Keys {
    session: Option<u64>,
    peer_id: Option<String>,
    peer: Option<String>,
    holder: Option<String>,
    device: Option<DeviceId>,
    link: Link,
}

pub(crate) struct Traffic {
    channel: Channel,
    keys: Keys,
    started: Instant,
    replay_bytes: u64,
    replay_raw_bytes: u64,
    live_bytes: u64,
    live_frames: u64,
    in_bytes: u64,
    lagged_events: u64,
    lagged_skipped: u64,
    peer_stage: PeerStage,
    left: Option<Leave>,
}

/// The numbers one log line carries.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) replay_bytes: u64,
    /// The replay frame as it would be without the gzip: equal to
    /// `replay_bytes` when the replay went raw.
    pub(crate) replay_raw_bytes: u64,
    pub(crate) live_bytes: u64,
    pub(crate) live_frames: u64,
    pub(crate) in_bytes: u64,
    pub(crate) lagged_events: u64,
    pub(crate) lagged_skipped: u64,
}

impl Traffic {
    /// A socket bridged to session `session` of this daemon.
    pub(crate) fn local(session: u64, tab: Tab, now: Instant) -> Traffic {
        let keys = Keys {
            session: Some(session),
            peer_id: None,
            peer: None,
            holder: tab.holder,
            device: tab.device,
            link: tab.link,
        };
        Traffic::new(Channel::Local, keys, now)
    }

    /// A socket relayed to a session the peer `peer_id` hosts; `peer` is its
    /// environment label, for people.
    pub(crate) fn peer(peer_id: String, peer: String, tab: Tab, now: Instant) -> Traffic {
        let keys = Keys {
            session: None,
            peer_id: Some(peer_id),
            peer: Some(peer),
            holder: tab.holder,
            device: tab.device,
            link: tab.link,
        };
        Traffic::new(Channel::Peer, keys, now)
    }

    fn new(channel: Channel, keys: Keys, now: Instant) -> Traffic {
        Traffic {
            channel,
            keys,
            started: now,
            replay_bytes: 0,
            replay_raw_bytes: 0,
            live_bytes: 0,
            live_frames: 0,
            in_bytes: 0,
            lagged_events: 0,
            lagged_skipped: 0,
            peer_stage: PeerStage::Open,
            left: None,
        }
    }

    /// Count a replay frame: `wire` bytes sent, `raw` bytes before a gzip.
    pub(crate) fn replay(&mut self, wire: usize, raw: usize) {
        self.replay_bytes += wire as u64;
        self.replay_raw_bytes += raw as u64;
    }

    pub(crate) fn live(&mut self, bytes: usize) {
        self.live_bytes += bytes as u64;
        self.live_frames += 1;
    }

    /// Count one binary frame a peer sent to the browser. The relay forwards
    /// the bytes unchanged; this reads them only until the replay is known.
    /// The `session-open` announcement names the session and says whether a
    /// replay follows; it is not counted, as the bridge does not count it. A
    /// peer older than the `replay` field gets its replay counted as live. A
    /// later `session-open` (the peer's resync after a lag) starts the count
    /// of its replay again.
    pub(crate) fn peer_frame(&mut self, bytes: &[u8]) {
        if self.peer_open(bytes) {
            return;
        }
        match self.peer_stage {
            PeerStage::Live | PeerStage::Open => self.live(bytes.len()),
            PeerStage::FirstOutput { replay, gzip } => self.peer_first_output(bytes, replay, gzip),
        }
    }

    /// Count a frame that follows a `session-open`: its first terminal frame
    /// is the replay when the announcement said one follows.
    fn peer_first_output(&mut self, bytes: &[u8], replay: bool, gzip: bool) {
        if bytes.first() == Some(&protocol::TAG_TERMINAL) {
            self.peer_stage = PeerStage::Live;
            if replay {
                let raw = if gzip {
                    gunzipped_frame_len(bytes)
                } else {
                    bytes.len()
                };
                self.replay(bytes.len(), raw);
                return;
            }
        }
        self.live(bytes.len());
    }

    /// Whether `bytes` is a `session-open` announcement. It moves the stage
    /// to the first output after it, and names the session when none is
    /// known.
    fn peer_open(&mut self, bytes: &[u8]) -> bool {
        if bytes.first() != Some(&protocol::TAG_COMMAND) {
            return false;
        }
        let Ok(Frame::Command(cmd)) = protocol::decode(bytes) else {
            return false;
        };
        if cmd.verb != SESSION_OPEN {
            return false;
        }
        if self.keys.session.is_none() {
            self.keys.session = cmd.payload.get("session").and_then(|v| v.as_u64());
        }
        let replay = cmd.payload.get("replay").and_then(|v| v.as_bool()) == Some(true);
        let gzip = cmd.payload.get("replay_encoding").and_then(|v| v.as_str()) == Some("gzip");
        self.peer_stage = PeerStage::FirstOutput { replay, gzip };
        true
    }

    pub(crate) fn inbound(&mut self, bytes: usize) {
        self.in_bytes += bytes as u64;
    }

    /// Read a command the page sent, for the log only: a `detach` names the
    /// leave, and a `shown` takes back a `hidden` one. A later leave replaces
    /// an earlier one.
    pub(crate) fn client_command(&mut self, cmd: &Command) {
        if let Some(leave) = Leave::from_command(cmd) {
            self.left = Some(leave);
        } else if cmd.verb == SHOWN && self.left == Some(Leave::Hidden) {
            self.left = None;
        }
    }

    /// How the socket ended, most specific first: the end this daemon decided
    /// (`daemon_end`), then the leave the page announced, then `dropped`.
    fn end_label<'a>(&self, daemon_end: Option<&'a str>) -> &'a str {
        if let Some(end) = daemon_end {
            return end;
        }
        if let Some(leave) = self.left {
            return leave.as_str();
        }
        DROPPED
    }

    /// A burst outran this socket and `skipped` messages never reached it.
    pub(crate) fn lagged(&mut self, skipped: u64) {
        self.lagged_events += 1;
        self.lagged_skipped += skipped;
    }

    pub(crate) fn summary(&self) -> Summary {
        Summary {
            replay_bytes: self.replay_bytes,
            replay_raw_bytes: self.replay_raw_bytes,
            live_bytes: self.live_bytes,
            live_frames: self.live_frames,
            in_bytes: self.in_bytes,
            lagged_events: self.lagged_events,
            lagged_skipped: self.lagged_skipped,
        }
    }

    /// `daemon_end` is the end this daemon decided, if it decided one.
    pub(crate) fn log(&self, daemon_end: Option<&str>, now: Instant) {
        let s = self.summary();
        let end = self.end_label(daemon_end);
        tracing::info!(
            channel = self.channel.as_str(),
            session = self.keys.session,
            peer_id = self.keys.peer_id.as_deref(),
            peer = self.keys.peer.as_deref(),
            holder = self.keys.holder.as_deref(),
            device = self.keys.device.map(|d| d.to_string()),
            net_type = self.keys.link.effective_type.map(EffectiveType::as_str),
            net_rtt_ms = self.keys.link.rtt_ms,
            net_downlink_mbps = self.keys.link.downlink_mbps,
            end,
            secs = now.duration_since(self.started).as_secs(),
            replay_bytes = s.replay_bytes,
            replay_raw_bytes = s.replay_raw_bytes,
            live_bytes = s.live_bytes,
            live_frames = s.live_frames,
            in_bytes = s.in_bytes,
            lagged_events = s.lagged_events,
            lagged_skipped = s.lagged_skipped,
            "console socket traffic"
        );
    }
}

/// The length a gzipped terminal frame would have raw: its header plus its
/// data decoded. A frame that does not decode counts as it was sent, so the
/// summary still has a number.
fn gunzipped_frame_len(frame: &[u8]) -> usize {
    let Ok(Frame::Terminal { data, .. }) = protocol::decode(frame) else {
        return frame.len();
    };
    let header = frame.len() - data.len();
    let mut decoder = flate2::read::GzDecoder::new(data.as_slice()).take(RAW_COUNT_CAP);
    match std::io::copy(&mut decoder, &mut std::io::sink()) {
        Ok(raw) => header + usize::try_from(raw).unwrap_or(usize::MAX - header),
        Err(error) => {
            tracing::debug!(%error, "a relayed gzip replay did not decode; counted as sent");
            frame.len()
        }
    }
}

#[cfg(test)]
mod tests;
