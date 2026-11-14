//! The texts of the wording catalog (`assets/wording/*.json`) as rows of copy.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Found, Kind};

/// The catalog folder, from the repo root.
pub(super) const WORDING_DIR: &str = "assets/wording";

/// The fields of a value that a user reads; `note` is for the writer.
const READ_FIELDS: [&str; 2] = ["text", "fallback"];

/// One `Found` per `text` and `fallback` of every value of one catalog file.
pub(super) fn scan(src: &str, out: &mut Vec<Found>) -> Result<()> {
    let doc: Value = serde_json::from_str(src).context("parsing a wording catalog file")?;
    for (set, values) in entries(doc.get("values")) {
        for (key, value) in entries(Some(values)) {
            for field in READ_FIELDS {
                if let Some(text) = value.get(field).and_then(Value::as_str) {
                    let line = line_of(src, &[set, key], field, text);
                    out.push(found(set, text, line));
                }
            }
        }
    }
    Ok(())
}

/// The members of a JSON object; none for a missing or another kind of value.
fn entries(value: Option<&Value>) -> impl Iterator<Item = (&str, &Value)> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, v)| (name.as_str(), v))
}

/// Every wording file of the catalog folder, as `(file name, rows)`.
pub(super) fn rows_by_file(root: &Path) -> Result<Vec<(String, Vec<Found>)>> {
    let dir = root.join(WORDING_DIR);
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("listing {}", dir.display()))? {
        let name = entry
            .with_context(|| format!("listing {}", dir.display()))?
            .file_name()
            .to_string_lossy()
            .into_owned();
        if Path::new(&name).extension().is_some_and(|e| e == "json") {
            names.push(name);
        }
    }
    names.sort();
    let mut files = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut found = Vec::new();
        scan(&src, &mut found).with_context(|| format!("reading {}", path.display()))?;
        files.push((format!("wording/{name}"), found));
    }
    Ok(files)
}

fn found(set: &str, text: &str, line: usize) -> Found {
    Found {
        text: text.to_owned(),
        line,
        kind: Kind::WordingValue,
        concatenated: false,
        area: Some(set.to_owned()),
        fragments: Vec::new(),
        anchor: None,
        continues: false,
    }
}

/// The line of `"field": "text"` after the set and key names; line 1 when the
/// file is not laid out as expected.
fn line_of(src: &str, path: &[&str], field: &str, text: &str) -> usize {
    let wanted = path
        .iter()
        .map(|name| format!("\"{name}\":"))
        .chain([format!("\"{field}\": {}", Value::from(text))]);
    let mut lines = 0;
    let mut rest = src;
    for needle in wanted {
        let Some((before, after)) = rest.split_once(&needle) else {
            return 1;
        };
        lines += before.matches('\n').count();
        rest = after;
    }
    lines + 1
}
