//! The dismissals of the attention list (docs/adr/0085 D12): the one fact of
//! the list that the daemon stores. `<store>/attention-dismissed.jsonl` holds
//! one line `{"id","ts"}` per dismissed request, appended and never
//! rewritten, so a dismissal holds after a restart and on every device.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::owner_only;

/// The file name, next to the repo registry in the daemon store.
pub const FILE: &str = "attention-dismissed.jsonl";

/// One line of the file.
#[derive(serde::Serialize, serde::Deserialize)]
struct Line {
    id: String,
    ts: String,
}

/// The dismissed request ids, and the file they are kept in.
#[derive(Debug, Default)]
pub struct Dismissals {
    path: PathBuf,
    ids: HashSet<String>,
}

impl Dismissals {
    /// The dismissals in `path`. A missing file is none; a line that is not
    /// a dismissal is skipped, so one bad line does not lose the others. A
    /// file that cannot be read is logged and read as none.
    pub fn load(path: PathBuf) -> Self {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not read the dismissals");
                String::new()
            }
        };
        let ids = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Line>(line).ok())
            .map(|line| line.id)
            .collect();
        Self { path, ids }
    }

    /// The dismissals of the store whose repo registry is `registry_path`.
    pub fn in_store_of(registry_path: &Path) -> Self {
        Self::load(registry_path.with_file_name(FILE))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Dismiss `id`: append its line, then keep it. An id already dismissed
    /// writes nothing. `Ok(false)` says so.
    pub fn dismiss(&mut self, id: &str) -> Result<bool> {
        if self.ids.contains(id) {
            return Ok(false);
        }
        let line = Line {
            id: id.to_string(),
            ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        let mut bytes = serde_json::to_vec(&line).context("encoding a dismissal")?;
        bytes.push(b'\n');
        owner_only::append_owner_only(&self.path, &bytes)?;
        self.ids.insert(id.to_string());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_line_is_skipped_and_the_others_are_kept() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join(FILE);
        std::fs::write(
            &path,
            "{\"id\":\"issue:20261028-100000:71\",\"ts\":\"2026-10-28T13:00:00Z\"}\nnot json\n{\"id\":\"issue:20261028-100000:72\",\"ts\":\"2026-10-28T13:00:01Z\"}\n",
        )
        .expect("write the file");
        let dismissals = Dismissals::load(path);
        assert!(dismissals.contains("issue:20261028-100000:71"));
        assert!(dismissals.contains("issue:20261028-100000:72"));
        assert_eq!(dismissals.ids.len(), 2);
    }
}
