//! The sending side of one daemon socket: a writer task that owns the
//! socket's sink half and sends each channel's frames in turn. And the page
//! socket `/ws`, whose channels the page opens and closes.
//!
//! The reader is the socket loop that holds the stream half; it reads on its
//! own and never waits for a send, so input is not queued behind output.

// ADR-0082 D5: one channel cannot hold back the others. One bounded queue per
// channel; the writer sends one piece per non-empty channel in turn.
mod writer;

// ADR-0082 D3: `/ws` carries channel 0 and the channels a page opens.
mod kinds;
mod socket;

pub(crate) use kinds::Ctx;
pub(crate) use socket::{serve, Login, Page, MAX_MESSAGE};
pub(crate) use writer::{ChannelSender, Out, Permit, Writer, WriterClosed};
