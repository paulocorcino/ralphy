//! The login book (#717): the device each login signed in from, the pairs of
//! login and other device the log already reported, and the pairs the
//! operator dismissed. The audit log owns these facts; the book is read from
//! it once and then kept with each append, under the audit writer's lock.
//!
//! The open notices are read from the log on each request: a reported pair
//! whose login is not revoked, that is not dismissed, and that is less than
//! [`NOTICE_MAX_AGE_SECS`] old.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::read::{self, DeviceSummary};
use super::{Event, EventKind};
use crate::device::DeviceId;

/// A notice older than this is not listed: the longest life of a cookie, so a
/// login it names has ended by then.
pub(crate) const NOTICE_MAX_AGE_SECS: i64 = 30 * 24 * 3600;

/// A login ID and a device ID, as the log writes them. No device is a value
/// of its own.
type Pair = (String, Option<String>);

#[derive(Debug, Default)]
pub(crate) struct Logins {
    /// The device of each login's `login_ok`.
    device_of: HashMap<String, Option<String>>,
    /// The pairs a `login_other_device` line names.
    reported: HashSet<Pair>,
    /// The pairs a `login_notice_dismissed` line names.
    dismissed: HashSet<Pair>,
}

/// The fields of a line the book reads. A line of another kind, or one that
/// does not parse, is skipped.
#[derive(Deserialize)]
struct Line {
    event: EventKind,
    login: Option<String>,
    device: Option<String>,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    did: Option<String>,
}

/// One open notice: a login used from another device.
#[derive(Debug, Serialize)]
pub(crate) struct Notice {
    pub(crate) login: String,
    /// The other device; `None` when the use came with no device cookie.
    pub(crate) device: Option<String>,
    /// What the login did from that device.
    pub(crate) did: Option<String>,
    pub(crate) at: String,
    /// What the log knows of the device from its page's reports; `None` when
    /// its page never reported facts.
    pub(crate) facts: Option<DeviceSummary>,
    /// Whether the notice names the device that reads the list. That device
    /// cannot dismiss it.
    pub(crate) here: bool,
}

/// The lines of the log at `path`, an absent file read as empty.
fn read_text(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn lines(text: &str) -> impl Iterator<Item = Line> + '_ {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Line>(line).ok())
}

/// The open notices of the log at `path` at `now` (unix seconds), the newest
/// first. `revoked` says whether a login ID is on the revoked-login list;
/// `this` is the device that asks, marked in the facts and in `here`.
pub(crate) fn notices(
    path: &Path,
    now: i64,
    this: Option<DeviceId>,
    revoked: impl Fn(&str) -> bool,
) -> Result<Vec<Notice>> {
    let text = read_text(path)?;
    let mut dismissed: HashSet<Pair> = HashSet::new();
    let mut open: Vec<Notice> = Vec::new();
    for line in lines(&text) {
        let Some(login) = line.login else { continue };
        match line.event {
            EventKind::LoginNoticeDismissed => {
                dismissed.insert((login, line.device));
            }
            EventKind::LoginOtherDevice => open.push(Notice {
                login,
                device: line.device,
                did: line.did,
                at: line.at.unwrap_or_default(),
                facts: None,
                here: false,
            }),
            EventKind::LoginOk
            | EventKind::LoginFailed
            | EventKind::Logout
            | EventKind::DeviceFacts
            | EventKind::DeviceProfileChanged
            | EventKind::Action
            | EventKind::Command
            | EventKind::ConsoleLaunch
            | EventKind::ConsoleTakeover => {}
        }
    }
    let cutoff = now - NOTICE_MAX_AGE_SECS;
    open.retain(|n| {
        !dismissed.contains(&(n.login.clone(), n.device.clone()))
            && !revoked(&n.login)
            && line_secs(&n.at).is_some_and(|at| at > cutoff)
    });
    for notice in &mut open {
        notice.here = is_device(notice.device.as_deref(), this);
    }
    if !open.is_empty() {
        add_facts(&mut open, read::devices(path, this)?);
    }
    open.reverse();
    Ok(open)
}

/// Whether the device of a notice, as the log writes it, is `this` device. No
/// device is a value of its own: it equals only no device.
pub(crate) fn is_device(device: Option<&str>, this: Option<DeviceId>) -> bool {
    match (device, this) {
        (Some(d), Some(id)) => d == id.to_string(),
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn line_secs(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|t| t.timestamp())
}

/// Give each notice the summary of its device, when that device's page
/// reported facts.
fn add_facts(open: &mut [Notice], devices: Vec<DeviceSummary>) {
    let by_id: HashMap<String, DeviceSummary> = devices
        .into_iter()
        .filter(has_facts)
        .map(|d| (d.device.clone(), d))
        .collect();
    for notice in open {
        notice.facts = notice.device.as_ref().and_then(|d| by_id.get(d).cloned());
    }
}

/// Whether a device summary holds facts a page reported, not only lines.
fn has_facts(d: &DeviceSummary) -> bool {
    [
        &d.os,
        &d.os_version,
        &d.browser,
        &d.browser_version,
        &d.form,
        &d.model,
        &d.gpu,
    ]
    .iter()
    .any(|f| f.is_some())
}

impl Logins {
    /// The book of the log at `path`. A login whose `login_ok` is no longer
    /// in the log has no device, so it is never reported.
    pub(crate) fn read(path: &Path) -> Result<Logins> {
        let text = read_text(path)?;
        let mut book = Logins::default();
        for line in lines(&text) {
            book.add(line.event, line.login, line.device);
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
            EventKind::LoginNoticeDismissed => {
                self.dismissed.insert((login, device));
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

    /// Whether the pair of `login` and `device` was dismissed.
    pub(crate) fn dismissed(&self, login: &str, device: Option<&str>) -> bool {
        self.dismissed
            .contains(&(login.to_string(), device.map(str::to_string)))
    }
}
