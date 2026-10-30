//! The sending side of one daemon socket: a writer task that owns the
//! socket's sink half and sends each channel's frames in turn.
//!
//! The reader is the socket loop that holds the stream half; it reads on its
//! own and never waits for a send, so input is not queued behind output.

// ADR-0082 D5: one channel cannot hold back the others. One bounded queue per
// channel; the writer sends one piece per non-empty channel in turn.
mod writer;

pub(crate) use writer::{ChannelSender, Mode, Writer, WriterClosed};
