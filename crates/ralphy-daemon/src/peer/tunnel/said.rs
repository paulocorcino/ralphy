//! The last line an `ssh` wrote on its stderr: why its tunnel closed.

use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The longest `ssh` line kept, in characters.
const SAID_MAX_CHARS: usize = 300;

/// How long a replaced `ssh`'s stderr reader may take to see EOF.
const SAID_SETTLE: Duration = Duration::from_millis(200);

/// The last non-empty line an `ssh` wrote on its stderr, kept by a thread that
/// reads the pipe to EOF.
pub(super) struct Said {
    line: Arc<Mutex<Option<String>>>,
    reader: Option<JoinHandle<()>>,
}

impl Said {
    /// The last line, once the reader has seen EOF or [`SAID_SETTLE`] passed.
    pub(super) fn settle(self) -> Option<String> {
        if let Some(reader) = &self.reader {
            let until = Instant::now() + SAID_SETTLE;
            while !reader.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.line
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

/// Take `child`'s stderr and keep its last non-empty line. The thread reads to
/// EOF, so the `ssh` never blocks on a full pipe while this daemon runs.
pub(super) fn watch_stderr(child: &mut Child) -> Said {
    let line = Arc::new(Mutex::new(None));
    let reader = child.stderr.take().and_then(|stderr| {
        let slot = line.clone();
        let started = std::thread::Builder::new()
            .name("tunnel-stderr".into())
            .spawn(move || keep_last_line(stderr, &slot));
        match started {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!(error = %e, "could not start the tunnel stderr reader");
                None
            }
        }
    });
    Said { line, reader }
}

fn keep_last_line(stderr: impl std::io::Read, slot: &Mutex<Option<String>>) {
    use std::io::BufRead;
    let mut stderr = std::io::BufReader::new(stderr);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match stderr.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim();
                if !text.is_empty() {
                    let kept: String = text.chars().take(SAID_MAX_CHARS).collect();
                    *slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(kept);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                tracing::debug!(error = %e, "reading the tunnel stderr failed");
                break;
            }
        }
    }
}
