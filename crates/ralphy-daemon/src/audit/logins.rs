//! The login book (#717): the device each login signed in from, and the
//! pairs of login and other device the log already reported. The audit log
//! owns both facts; the book is read from it once and then kept with each
//! append, under the audit writer's lock.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{Event, EventKind};

/// A login ID and a device ID, as the log writes them. No device is a value
/// of its own.
type Pair = (String, Option<String>);

#[derive(Debug, Default)]
pub(crate) struct Logins {
    /// The device of each login's `login_ok`.
    device_of: HashMap<String, Option<String>>,
    /// The pairs a `login_other_device` line names.
    reported: HashSet<Pair>,
}

/// The fields of a line the book reads. A line of another kind, or one that
/// does not parse, is skipped.
#[derive(Deserialize)]
struct Line {
    event: EventKind,
    login: Option<String>,
    device: Option<String>,
}

impl Logins {
    /// The book of the log at `path`. A login whose `login_ok` is no longer
    /// in the log has no device, so it is never reported.
    pub(crate) fn read(path: &Path) -> Result<Logins> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut book = Logins::default();
        for line in text.lines() {
            if let Ok(line) = serde_json::from_str::<Line>(line) {
                book.add(line.event, line.login, line.device);
            }
        }
        Ok(book)
    }

    /// Keep the book in step with `event`, a line just appended.
    pub(crate) fn note(&mut self, event: &Event) {
        self.add(event.event, event.login.clone(), event.device.clone());
    }

    fn add(&mut self, kind: EventKind, login: Option<String>, device: Option<String>) {
        let Some(login) = login else { return };
        match kind {
            EventKind::LoginOk => {
                self.device_of.insert(login, device);
            }
            EventKind::LoginOtherDevice => {
                self.reported.insert((login, device));
            }
            EventKind::LoginFailed
            | EventKind::Logout
            | EventKind::DeviceFacts
            | EventKind::DeviceProfileChanged
            | EventKind::Action
            | EventKind::Command
            | EventKind::ConsoleLaunch
            | EventKind::ConsoleTakeover => {}
        }
    }

    /// Whether a use of `login` from `device` is the first use of a new pair:
    /// the login signed in from another device and the pair was not reported
    /// yet. A `true` marks the pair as reported.
    pub(crate) fn first_use_elsewhere(&mut self, login: &str, device: Option<&str>) -> bool {
        let Some(own) = self.device_of.get(login) else {
            return false;
        };
        if own.as_deref() == device {
            return false;
        }
        self.reported
            .insert((login.to_string(), device.map(str::to_string)))
    }
}
