//! The session id sequence, kept across daemon restarts when it has a file.

use std::path::PathBuf;

use super::super::SessionId;

/// The session id sequence. With a `file`, the last id issued is written there
/// each time, and a manager built over the same file continues past it — so a
/// restarted daemon never re-issues an id the last one used. A browser still
/// holding `?id=N` from before the restart gets `404` instead of reattaching to
/// whichever NEW session drew N.
pub(super) struct IdSeq {
    pub(super) last: SessionId,
    pub(super) file: Option<PathBuf>,
}

impl IdSeq {
    /// Continue past what `file` recorded. A missing file is a first boot; an
    /// unreadable or corrupt one is said out loud and restarts the sequence,
    /// the pre-persistence behaviour, rather than refusing to serve sessions.
    pub(super) fn continuing(file: PathBuf) -> IdSeq {
        let last = match std::fs::read_to_string(&file) {
            Ok(text) => text.trim().parse::<SessionId>().unwrap_or_else(|e| {
                tracing::warn!(
                    file = %file.display(),
                    error = %e,
                    "session id record is corrupt; ids restart at 1"
                );
                0
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => {
                tracing::warn!(
                    file = %file.display(),
                    error = %e,
                    "session id record is unreadable; ids restart at 1"
                );
                0
            }
        };
        IdSeq {
            last,
            file: Some(file),
        }
    }

    /// Issue the next id and record it. Starts at 1: 0 reads as "unset" and the
    /// codec's default session. A failed write is logged, not fatal — the id is
    /// still unique within this daemon's life.
    pub(super) fn next(&mut self) -> SessionId {
        self.last += 1;
        if let Some(file) = &self.file {
            if let Err(e) = std::fs::write(file, self.last.to_string()) {
                tracing::warn!(
                    file = %file.display(),
                    error = %e,
                    "could not record the last session id"
                );
            }
        }
        self.last
    }
}
