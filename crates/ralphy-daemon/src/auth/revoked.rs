//! The revoked-login list (issue #717): the login IDs the operator revoked.
//! Every cookie check refuses a cookie whose login is on the list.
//!
//! The file `daemon-revoked-logins` in the daemon store holds one JSON line
//! per entry, `{"login":"<id>","cap":<unix seconds>}`, written owner-only.
//! `cap` is the revoke time plus the longest absolute life of a cookie
//! ([`crate::cookie::REMEMBERED_ABSOLUTE_TTL_SECS`]): after it no cookie of
//! that login can still be valid, so an entry whose `cap` has passed is
//! dropped on load and on add. The list stays as small as the logins revoked
//! in the last 30 days.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};

use crate::cookie::LoginId;

/// The revoked-login list path inside `dir`.
pub fn revoked_logins_path_in(dir: &Path) -> PathBuf {
    dir.join("daemon-revoked-logins")
}

/// One revoked login and the time after which no cookie of it is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) login: LoginId,
    pub(crate) cap: u64,
}

/// One line of the file.
#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    login: String,
    cap: u64,
}

/// The live list, shared by the auth state and every session policy built
/// from it, as the session epoch is: a revoke changes it in place, and the
/// next cookie check sees the change.
#[derive(Clone, Default)]
pub struct RevokedLogins(Arc<RwLock<Vec<Entry>>>);

impl RevokedLogins {
    /// Whether `login` is on the list.
    pub fn contains(&self, login: &LoginId) -> bool {
        self.0
            .read()
            .expect("revoked-login list lock poisoned")
            .iter()
            .any(|e| e.login == *login)
    }

    /// Replace the whole list with `entries`.
    pub(crate) fn replace(&self, entries: Vec<Entry>) {
        *self.0.write().expect("revoked-login list lock poisoned") = entries;
    }
}

/// The entries of the file at `path` whose `cap` is after `now`. A missing
/// file is an empty list; a line that is not an entry is skipped with a
/// warning, so one bad line does not lose the others.
pub(crate) fn load(path: &Path, now: u64) -> Result<Vec<Entry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_line)
        .filter(|e| e.cap > now)
        .collect())
}

/// One line as an entry, or `None` (with a warning) when it is not one.
fn parse_line(line: &str) -> Option<Entry> {
    let parsed = match serde_json::from_str::<Line>(line) {
        Ok(parsed) => parsed,
        Err(e) => {
            tracing::warn!(error = %e, "skipped a line of the revoked-login list that is not JSON");
            return None;
        }
    };
    let Some(login) = LoginId::parse(&parsed.login) else {
        tracing::warn!("skipped a line of the revoked-login list with no valid login ID");
        return None;
    };
    Some(Entry {
        login,
        cap: parsed.cap,
    })
}

/// `entries` with `login` added under `cap`, without the entries whose `cap`
/// is not after `now`. A login already on the list stays once, with the later
/// of its two caps.
pub(crate) fn add(mut entries: Vec<Entry>, login: LoginId, cap: u64, now: u64) -> Vec<Entry> {
    entries.retain(|e| e.cap > now);
    match entries.iter_mut().find(|e| e.login == login) {
        Some(found) => found.cap = found.cap.max(cap),
        None => entries.push(Entry { login, cap }),
    }
    entries
}

/// Write `entries` as the whole file at `path`, owner-only.
pub(crate) fn write(path: &Path, entries: &[Entry]) -> Result<()> {
    let mut text = String::new();
    for e in entries {
        let line = serde_json::to_string(&Line {
            login: e.login.to_string(),
            cap: e.cap,
        })
        .context("encoding a revoked-login entry")?;
        text.push_str(&line);
        text.push('\n');
    }
    if let Some(parent) = path.parent() {
        crate::owner_only::create_owner_only_dir(parent)?;
    }
    crate::owner_only::write_owner_only(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn revoking_twice_keeps_one_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = revoked_logins_path_in(dir.path());
        let login = LoginId::mint();
        for cap in [NOW + 100, NOW + 200] {
            let entries = add(load(&path, NOW).unwrap(), login, cap, NOW);
            write(&path, &entries).unwrap();
        }
        assert_eq!(
            load(&path, NOW).unwrap(),
            [Entry {
                login,
                cap: NOW + 200
            }]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1, "one line in the file: {text}");
    }

    #[test]
    fn an_entry_past_its_cap_is_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let path = revoked_logins_path_in(dir.path());
        let (old, live) = (LoginId::mint(), LoginId::mint());
        std::fs::write(
            &path,
            format!(
                "{{\"login\":\"{old}\",\"cap\":{NOW}}}\n{{\"login\":\"{live}\",\"cap\":{}}}\n",
                NOW + 1
            ),
        )
        .unwrap();
        assert_eq!(
            load(&path, NOW).unwrap(),
            [Entry {
                login: live,
                cap: NOW + 1
            }],
            "an entry whose cap is now is gone, one a second later stays"
        );
    }

    #[test]
    fn two_revokes_at_once_keep_both() {
        let now = crate::routes::now_unix();
        for _ in 0..20 {
            let auth = crate::auth::AuthState::localhost();
            let logins: Vec<LoginId> = (0..4).map(|_| LoginId::mint()).collect();
            std::thread::scope(|s| {
                for login in &logins {
                    let auth = &auth;
                    s.spawn(move || auth.revoke_login(*login, now).unwrap());
                }
            });
            for login in &logins {
                assert!(auth.login_revoked(login), "{login} is on the list");
            }
            let on_disk = load(&auth.revoked_path, now).unwrap();
            assert_eq!(on_disk.len(), logins.len(), "every revoke is in the file");
        }
    }
}
