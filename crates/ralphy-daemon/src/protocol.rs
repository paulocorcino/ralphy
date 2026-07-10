//! The daemon↔UI wire codec: a transport-agnostic tagged framing over one
//! duplex connection (docs/adr/0032 §4/§5). Byte 0 is a channel tag; the rest
//! is the channel's payload. Three channels are multiplexed over the single
//! connection:
//!
//! - `Terminal` — high-volume raw bytes for a session's PTY: `[0x01][session
//!   u64 BE][raw bytes]`. Raw, not JSON: base64-in-JSON would bloat it.
//! - `Command` — a structured request/response verb: `[0x02][JSON]`.
//! - `Presence` — the daemon's liveness heartbeat: `[0x03][JSON]`.
//!
//! This module knows nothing about the transport carrying a frame — it turns a
//! [`Frame`] into a `Vec<u8>` and back, and the caller rides that byte string
//! on whatever duplex message channel it has. Keeping it transport-agnostic is
//! what lets the same codec be exercised by pure round-trip tests with no I/O.

use serde::{Deserialize, Serialize};

/// Leading byte of a raw-PTY frame.
pub const TAG_TERMINAL: u8 = 0x01;
/// Leading byte of a structured command frame.
pub const TAG_COMMAND: u8 = 0x02;
/// Leading byte of a presence-heartbeat frame.
pub const TAG_PRESENCE: u8 = 0x03;

/// A single multiplexed frame. `PartialEq` (not `Eq`): `Command.payload` is a
/// `serde_json::Value`, which is not `Eq`.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// Raw PTY bytes for one session, keyed by its `session` id.
    Terminal { session: u64, data: Vec<u8> },
    /// A structured request/response verb.
    Command(Command),
    /// The daemon's liveness heartbeat.
    Presence(Presence),
}

/// A structured command with a correlation `id` for request/response pairing and
/// an opaque `payload`. The daemon grows verb executors in a later slice; this
/// slice only fixes the wire shape so it does not preclude request/response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub id: u64,
    pub verb: String,
    pub payload: serde_json::Value,
}

/// The daemon's presence heartbeat: who it is and how long it has been up. A
/// missing `name`/`avatar` means an un-baptized daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub uptime_secs: u64,
}

/// Why a byte string failed to decode into a [`Frame`].
#[derive(Debug, Clone, PartialEq)]
pub enum FrameError {
    /// Zero bytes — no tag to dispatch on.
    Empty,
    /// The leading tag byte matches no known channel.
    UnknownTag(u8),
    /// A terminal frame with fewer than 8 payload bytes (no room for the
    /// session id).
    ShortTerminal,
    /// A structured frame whose JSON payload did not parse.
    BadJson(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Empty => write!(f, "empty frame: no channel tag"),
            FrameError::UnknownTag(tag) => write!(f, "unknown channel tag {tag:#04x}"),
            FrameError::ShortTerminal => {
                write!(f, "terminal frame too short for an 8-byte session id")
            }
            FrameError::BadJson(msg) => write!(f, "malformed frame JSON: {msg}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Encode a [`Frame`] into its wire byte string.
pub fn encode(frame: &Frame) -> Vec<u8> {
    match frame {
        Frame::Terminal { session, data } => {
            let mut out = Vec::with_capacity(1 + 8 + data.len());
            out.push(TAG_TERMINAL);
            out.extend_from_slice(&session.to_be_bytes());
            out.extend_from_slice(data);
            out
        }
        Frame::Command(cmd) => encode_json(TAG_COMMAND, cmd),
        Frame::Presence(p) => encode_json(TAG_PRESENCE, p),
    }
}

fn encode_json<T: Serialize>(tag: u8, value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(tag);
    out.extend_from_slice(&serde_json::to_vec(value).expect("frame value serializes"));
    out
}

/// Decode a wire byte string back into a [`Frame`].
pub fn decode(bytes: &[u8]) -> Result<Frame, FrameError> {
    let (&tag, rest) = bytes.split_first().ok_or(FrameError::Empty)?;
    match tag {
        TAG_TERMINAL => {
            if rest.len() < 8 {
                return Err(FrameError::ShortTerminal);
            }
            let (id_bytes, data) = rest.split_at(8);
            let session = u64::from_be_bytes(id_bytes.try_into().expect("split_at(8) yields 8"));
            Ok(Frame::Terminal {
                session,
                data: data.to_vec(),
            })
        }
        TAG_COMMAND => serde_json::from_slice(rest)
            .map(Frame::Command)
            .map_err(|e| FrameError::BadJson(e.to_string())),
        TAG_PRESENCE => serde_json::from_slice(rest)
            .map(Frame::Presence)
            .map_err(|e| FrameError::BadJson(e.to_string())),
        other => Err(FrameError::UnknownTag(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> Command {
        Command {
            id: 99,
            verb: "forge.list".into(),
            payload: serde_json::json!({ "repo": "owner/name" }),
        }
    }

    #[test]
    fn round_trip_terminal() {
        for data in [vec![], vec![0u8, 255, 1, 2]] {
            let f = Frame::Terminal { session: 7, data };
            assert_eq!(decode(&encode(&f)).unwrap(), f);
        }
    }

    #[test]
    fn round_trip_command() {
        let f = Frame::Command(command());
        assert_eq!(decode(&encode(&f)).unwrap(), f);
    }

    #[test]
    fn round_trip_presence() {
        for name in [Some("anvil".to_string()), None] {
            let f = Frame::Presence(Presence {
                name,
                avatar: Some("🐙".into()),
                uptime_secs: 42,
            });
            assert_eq!(decode(&encode(&f)).unwrap(), f);
        }
    }

    #[test]
    fn channels_are_distinct() {
        let terminal = Frame::Terminal {
            session: 7,
            data: vec![],
        };
        let command = Frame::Command(command());
        let presence = Frame::Presence(Presence {
            name: None,
            avatar: None,
            uptime_secs: 0,
        });
        assert_eq!(encode(&terminal)[0], TAG_TERMINAL);
        assert_eq!(encode(&command)[0], TAG_COMMAND);
        assert_eq!(encode(&presence)[0], TAG_PRESENCE);

        let a = Frame::Terminal {
            session: 7,
            data: vec![],
        };
        let b = Frame::Terminal {
            session: 42,
            data: vec![],
        };
        match (decode(&encode(&a)).unwrap(), decode(&encode(&b)).unwrap()) {
            (Frame::Terminal { session: sa, .. }, Frame::Terminal { session: sb, .. }) => {
                assert_eq!(sa, 7);
                assert_eq!(sb, 42);
            }
            other => panic!("expected two terminal frames, got {other:?}"),
        }
    }

    #[test]
    fn malformed_empty() {
        assert_eq!(decode(&[]), Err(FrameError::Empty));
    }

    #[test]
    fn malformed_unknown_tag() {
        assert_eq!(decode(&[0x09]), Err(FrameError::UnknownTag(9)));
    }

    #[test]
    fn malformed_short_terminal() {
        assert_eq!(decode(&[0x01, 0, 0, 0]), Err(FrameError::ShortTerminal));
    }

    #[test]
    fn malformed_bad_json() {
        assert!(matches!(decode(&[0x03, b'{']), Err(FrameError::BadJson(_))));
    }
}
