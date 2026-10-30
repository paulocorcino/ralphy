//! The channel frame: `[0x04][channel: u32 BE][flags: u8][piece]`. The pieces
//! of one channel, joined in order, are one inner frame in the codec of the
//! parent module. Flag bit 0 is MORE: another piece of the same inner frame
//! follows on this channel. Every other bit is 0, and a frame with another bit
//! set is refused. Nothing sends this frame yet.

use super::{Frame, FrameError, TAG_CHANNEL};
use crate::tree::MAX_COMMAND_BYTES;

/// The largest piece one channel frame carries, in both directions.
pub const MAX_PIECE: usize = 16 * 1024;

/// Flag bit 0: another piece of the same inner frame follows.
const FLAG_MORE: u8 = 0b0000_0001;

/// Why a channel frame or a joined inner frame is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// The flags byte has a bit set other than MORE.
    UnknownFlags(u8),
    /// The pieces joined so far are larger than `MAX_COMMAND_BYTES`.
    OverCap,
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelError::UnknownFlags(flags) => {
                write!(f, "channel frame with unknown flags {flags:#04x}")
            }
            ChannelError::OverCap => {
                write!(
                    f,
                    "joined channel frame is larger than {MAX_COMMAND_BYTES} bytes"
                )
            }
        }
    }
}

impl std::error::Error for ChannelError {}

/// The wire bytes of a channel frame.
pub(super) fn encode(channel: u32, more: bool, piece: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + 1 + piece.len());
    out.push(TAG_CHANNEL);
    out.extend_from_slice(&channel.to_be_bytes());
    out.push(if more { FLAG_MORE } else { 0 });
    out.extend_from_slice(piece);
    out
}

/// Read a channel frame from the bytes after its tag.
pub(super) fn decode(rest: &[u8]) -> Result<Frame, FrameError> {
    let Some((head, piece)) = rest.split_first_chunk::<5>() else {
        return Err(FrameError::ShortChannel);
    };
    let [c0, c1, c2, c3, flags] = *head;
    if flags & !FLAG_MORE != 0 {
        return Err(FrameError::Channel(ChannelError::UnknownFlags(flags)));
    }
    Ok(Frame::Channel {
        channel: u32::from_be_bytes([c0, c1, c2, c3]),
        more: flags & FLAG_MORE != 0,
        piece: piece.to_vec(),
    })
}

/// Cut one inner frame into the channel frames that carry it: pieces of at
/// most [`MAX_PIECE`] bytes, MORE set on every piece but the last. An empty
/// inner frame is one empty piece.
pub fn cut(channel: u32, inner: &[u8]) -> Vec<Frame> {
    let pieces: Vec<&[u8]> = if inner.is_empty() {
        vec![inner]
    } else {
        inner.chunks(MAX_PIECE).collect()
    };
    let last = pieces.len() - 1;
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, piece)| Frame::Channel {
            channel,
            more: i < last,
            piece: piece.to_vec(),
        })
        .collect()
}

/// Joins the pieces of one channel back into inner frames. One joiner per
/// channel: the pieces of two channels never mix.
#[derive(Debug, Default)]
pub struct Joiner {
    buf: Vec<u8>,
}

impl Joiner {
    /// Add one piece. Returns the whole inner frame when `more` is false, and
    /// `None` while pieces are still due. A frame past `MAX_COMMAND_BYTES` is an
    /// error and the partial frame is dropped.
    pub fn push(&mut self, more: bool, piece: &[u8]) -> Result<Option<Vec<u8>>, ChannelError> {
        if self.buf.len() + piece.len() > MAX_COMMAND_BYTES {
            self.buf = Vec::new();
            return Err(ChannelError::OverCap);
        }
        self.buf.extend_from_slice(piece);
        Ok((!more).then(|| std::mem::take(&mut self.buf)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{decode as decode_frame, encode as encode_frame};

    fn piece_of(frame: &Frame) -> (bool, &[u8]) {
        match frame {
            Frame::Channel { more, piece, .. } => (*more, piece),
            other @ (Frame::Terminal { .. } | Frame::Command(_) | Frame::Presence(_)) => {
                panic!("expected a channel frame, got {other:?}")
            }
        }
    }

    #[test]
    fn a_large_frame_is_cut_into_pieces_of_16_kib() {
        let inner: Vec<u8> = (0..40 * 1024).map(|i| (i % 251) as u8).collect();
        let frames = cut(7, &inner);
        let shape: Vec<(bool, usize)> = frames
            .iter()
            .map(|f| {
                let (more, piece) = piece_of(f);
                (more, piece.len())
            })
            .collect();
        assert_eq!(shape, [(true, 16384), (true, 16384), (false, 8192)]);
        let joined: Vec<u8> = frames.iter().flat_map(|f| piece_of(f).1.to_vec()).collect();
        assert_eq!(joined, inner);
    }

    #[test]
    fn a_channel_frame_with_an_unknown_flag_is_refused() {
        let bytes = [TAG_CHANNEL, 0, 0, 0, 1, 0x02, b'x'];
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::Channel(ChannelError::UnknownFlags(0x02)))
        );
        // MORE together with another bit is refused too.
        let bytes = [TAG_CHANNEL, 0, 0, 0, 1, 0x81];
        assert_eq!(
            decode_frame(&bytes),
            Err(FrameError::Channel(ChannelError::UnknownFlags(0x81)))
        );
    }

    #[test]
    fn a_joined_frame_past_the_cap_is_refused() {
        // A frame of exactly the cap is joined.
        let mut joiner = Joiner::default();
        let at_cap = vec![1u8; MAX_COMMAND_BYTES];
        assert_eq!(joiner.push(false, &at_cap), Ok(Some(at_cap.clone())));

        // One byte more is refused, and the partial frame is dropped.
        let mut joiner = Joiner::default();
        assert_eq!(joiner.push(true, &at_cap), Ok(None));
        assert_eq!(joiner.push(true, b"x"), Err(ChannelError::OverCap));
        assert_eq!(joiner.push(false, b"ok"), Ok(Some(b"ok".to_vec())));
    }

    #[test]
    fn a_channel_frame_round_trips() {
        let rows = [
            ("last piece", u32::MAX, false, b"abc".to_vec()),
            ("first piece", u32::MAX, true, vec![0u8, 255, 1]),
            ("empty piece", 1, false, vec![]),
        ];
        for (case, channel, more, piece) in rows {
            let frame = Frame::Channel {
                channel,
                more,
                piece,
            };
            assert_eq!(decode_frame(&encode_frame(&frame)), Ok(frame), "{case}");
        }
    }

    #[test]
    fn a_cut_frame_joins_back_whole() {
        let inner = vec![9u8; 3 * MAX_PIECE];
        let mut joiner = Joiner::default();
        let mut whole = None;
        for frame in cut(3, &inner) {
            let (more, piece) = piece_of(&frame);
            whole = joiner.push(more, piece).expect("within the cap");
        }
        assert_eq!(whole, Some(inner));
    }

    #[test]
    fn a_channel_frame_without_its_header_is_refused() {
        assert_eq!(
            decode_frame(&[TAG_CHANNEL, 0, 0, 0, 1]),
            Err(FrameError::ShortChannel)
        );
    }
}
