//! The Antigravity module of the usage scan (ADR-0033 §2, ADR-0086 D5). Reads the
//! SQLite conversation store of an `agy` root read-only into one interactive
//! record per conversation × model.
//!
//! `<root>/antigravity-cli/conversations/<id>.db`, table `gen_metadata`, holds one
//! row per model call; its `data` is protobuf with no published schema. The
//! field paths come from a schema-less decode of live rows
//! (`docs/spike/live/antigravity-782-store.log`): 1.4.2 input (cache reads not
//! in it), 1.4.3 output (thinking in it), 1.4.5 cache read, 1.21 the label
//! `agy models` prints. `agy` keeps no cache-creation count, so it is 0.
//!
//! A row the decoder cannot read gives the model as unknown, never a guess, and
//! its conversation's record is a lower bound. Every store error gives no
//! record: the scan never fails.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::{AntigravityScan, InteractiveRecord, Tokens};

/// The folder `agy` keeps its state in, inside the `--gemini_dir` root.
const APP_DIR: &str = "antigravity-cli";
const UNKNOWN_MODEL: &str = "unknown";

/// The model families whose `agy models` id writes the dot of a version as a
/// dash (`Claude Opus 5.5` is `claude-opus-5-5`; `Gemini 3.8 Flash` keeps
/// `3.8`), as the listing in `tests/fixtures/agy-models.txt` shows.
const DASHED_VERSION_FAMILIES: &[&str] = &["claude-"];

/// Scan every conversation store under `input.root` into interactive records.
/// A missing root, a locked or corrupt file, or a missing table gives no record
/// for that store, never an error.
pub fn scan_antigravity(input: &AntigravityScan<'_>) -> Vec<InteractiveRecord> {
    let app_dir = input.root.join(APP_DIR);
    let summaries = read_summaries(&app_dir.join("conversation_summaries.db")).unwrap_or_default();
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut records = Vec::new();
    for (session_id, path) in conversation_stores(&app_dir.join("conversations")) {
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        let calls = read_calls(&path).unwrap_or_default();
        let summary = summaries.get(&session_id);
        let (project, actor_email) = attribute(input, summary, &mut email_cache);
        let last_ts = summary.map(|s| s.last_modified.clone()).unwrap_or_default();
        for (model, agg) in fold_calls(&calls) {
            records.push(InteractiveRecord {
                agent: "antigravity".to_owned(),
                model,
                session_id: session_id.clone(),
                project: project.clone(),
                actor_email: actor_email.clone(),
                tokens: Some(agg.tokens),
                first_ts: last_ts.clone(),
                last_ts: last_ts.clone(),
                lower_bound: agg.unreadable,
            });
        }
    }
    crate::since::retain_since(&mut records, input.since);
    records
}

/// The `<id>.db` files of the conversations folder, as `(id, path)`.
fn conversation_stores(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "db"))
        .filter_map(|path| {
            let id = path.file_stem()?.to_str()?.to_owned();
            Some((id, path))
        })
        .collect()
}

/// What one `gen_metadata` row says; `None` when the decoder cannot read it.
type Call = Option<DecodedCall>;

struct DecodedCall {
    tokens: Tokens,
    /// Empty when the row has no label.
    label: String,
}

/// The `data` of every `gen_metadata` row, decoded, in call order.
fn read_calls(path: &Path) -> rusqlite::Result<Vec<Call>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare("SELECT data FROM gen_metadata ORDER BY idx")?;
    let rows = stmt.query_map([], |row| row.get::<_, Option<Vec<u8>>>(0))?;
    Ok(rows
        .map(|data| data.ok().flatten().and_then(|data| decode_call(&data)))
        .collect())
}

/// The counts of one model in a conversation; `unreadable` when a call of the
/// conversation could not be read, so the counts are a floor.
#[derive(Default)]
struct ModelAgg {
    tokens: Tokens,
    unreadable: bool,
}

/// Sum the calls per model. A model whose calls all count zero is dropped, so
/// a conversation cut before its first model call gives no record.
fn fold_calls(calls: &[Call]) -> BTreeMap<String, ModelAgg> {
    let mut groups: BTreeMap<String, ModelAgg> = BTreeMap::new();
    for call in calls {
        let Some(call) = call else {
            groups
                .entry(UNKNOWN_MODEL.to_owned())
                .or_default()
                .unreadable = true;
            continue;
        };
        let model = model_id(&call.label).unwrap_or_else(|| UNKNOWN_MODEL.to_owned());
        add_tokens(&mut groups.entry(model).or_default().tokens, &call.tokens);
    }
    groups.retain(|_, agg| agg.unreadable || agg.tokens != Tokens::default());
    groups
}

fn add_tokens(sum: &mut Tokens, call: &Tokens) {
    sum.input = sum.input.saturating_add(call.input);
    sum.output = sum.output.saturating_add(call.output);
    sum.cache_read = sum.cache_read.saturating_add(call.cache_read);
}

/// The `agy models` id of a label: `Gemini 3.8 Flash (Low)` is
/// `gemini-3.8-flash-low`. `None` for a label outside that shape.
fn model_id(label: &str) -> Option<String> {
    let (name, rest) = label.split_once(" (")?;
    let effort = rest.strip_suffix(')')?;
    let name_ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-'));
    let effort_ok = effort.chars().all(|c| c.is_ascii_alphabetic());
    if name.is_empty() || effort.is_empty() || !name_ok || !effort_ok {
        return None;
    }
    let mut base = name.to_ascii_lowercase().replace(' ', "-");
    if DASHED_VERSION_FAMILIES
        .iter()
        .any(|family| base.starts_with(family))
    {
        base = base.replace('.', "-");
    }
    Some(format!("{base}-{}", effort.to_ascii_lowercase()))
}

/// Decode one row: usage at 1.4 (2 input, 3 output, 5 cache read) and the
/// label at 1.21. `None` when the bytes are not a well-formed message or a
/// field on the path has another wire type.
fn decode_call(data: &[u8]) -> Option<DecodedCall> {
    let call = fields(bytes_of(&fields(data)?, 1)?)?;
    let usage = fields(bytes_of(&call, 4)?)?;
    let label = std::str::from_utf8(bytes_of(&call, 21)?).unwrap_or_default();
    Some(DecodedCall {
        tokens: Tokens {
            input: varint_of(&usage, 2)?,
            output: varint_of(&usage, 3)?,
            cache_read: varint_of(&usage, 5)?,
            cache_creation: 0,
        },
        label: label.to_owned(),
    })
}

/// A protobuf field value, by wire type.
enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

/// The last value of `field` as bytes: empty when absent, as protobuf reads
/// it; `None` when it has another wire type.
fn bytes_of<'a>(fields: &[(u64, Value<'a>)], field: u64) -> Option<&'a [u8]> {
    match fields.iter().rev().find(|(n, _)| *n == field) {
        None => Some(&[]),
        Some((_, Value::Bytes(bytes))) => Some(bytes),
        Some(_) => None,
    }
}

/// The last value of `field` as a varint: 0 when absent, `None` when it has
/// another wire type.
fn varint_of(fields: &[(u64, Value<'_>)], field: u64) -> Option<u64> {
    match fields.iter().rev().find(|(n, _)| *n == field) {
        None => Some(0),
        Some((_, Value::Varint(value))) => Some(*value),
        Some(_) => None,
    }
}

/// Every `(field number, value)` of one message; `None` on any malformed byte.
fn fields(buf: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    let mut out = Vec::new();
    let mut rest = buf;
    while !rest.is_empty() {
        let key = read_varint(&mut rest)?;
        let value = match key & 7 {
            0 => Value::Varint(read_varint(&mut rest)?),
            1 => skip(&mut rest, 8).map(|_| Value::Fixed)?,
            2 => {
                let len = usize::try_from(read_varint(&mut rest)?).ok()?;
                Value::Bytes(skip(&mut rest, len)?)
            }
            5 => skip(&mut rest, 4).map(|_| Value::Fixed)?,
            _ => return None,
        };
        out.push((key >> 3, value));
    }
    Some(out)
}

/// Take `len` bytes off the front of `rest`.
fn skip<'a>(rest: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
    let taken = rest.get(..len)?;
    *rest = rest.get(len..)?;
    Some(taken)
}

/// Read one base-128 varint off the front of `rest`; `None` past ten bytes.
fn read_varint(rest: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, tail) = rest.split_first()?;
        *rest = tail;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// What `conversation_summaries.db` says of one conversation.
struct Summary {
    workspace: Option<String>,
    last_modified: String,
}

/// The workspace and last change of every conversation the summary store lists.
fn read_summaries(path: &Path) -> rusqlite::Result<HashMap<String, Summary>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT conversation_id, workspace_uris, CAST(last_modified_time AS TEXT) \
         FROM conversation_summaries",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    Ok(rows
        .flatten()
        .map(|(id, uris, modified)| {
            let summary = Summary {
                workspace: uris.as_deref().and_then(workspace_path),
                last_modified: modified.as_deref().map(rfc3339).unwrap_or_default(),
            };
            (id, summary)
        })
        .collect())
}

/// The path of the first workspace URI: a JSON list or a comma list of
/// `file://` URIs, or a plain path.
fn workspace_path(uris: &str) -> Option<String> {
    let first = match serde_json::from_str::<Vec<String>>(uris) {
        Ok(list) => list.into_iter().next()?,
        Err(_) => uris.split([',', '\n']).next()?.trim().to_owned(),
    };
    let path = percent_decode(first.strip_prefix("file://").unwrap_or(&first));
    let bytes = path.as_bytes();
    let drive_after_slash = bytes.first() == Some(&b'/') && bytes.get(2) == Some(&b':');
    let path = if drive_after_slash {
        path.get(1..)?.to_owned()
    } else {
        path
    };
    (!path.is_empty()).then_some(path)
}

/// Decode the `%XX` escapes of a URI path; a broken escape stays as it is.
fn percent_decode(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        let escaped = (byte == b'%')
            .then(|| tail.get(..2))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(decoded) = escaped {
            out.push(decoded);
            rest = tail.get(2..).unwrap_or_default();
        } else {
            out.push(byte);
            rest = tail;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A stored time as RFC 3339 UTC; empty when it does not parse.
fn rfc3339(stored: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(stored)
        .or_else(|_| chrono::DateTime::parse_from_str(stored, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .map(|time| time.with_timezone(&chrono::Utc).to_rfc3339())
        .unwrap_or_default()
}

/// The repo a conversation's workspace sits in and that repo's git actor email,
/// looked up once per slug through `email_cache`.
fn attribute(
    input: &AntigravityScan<'_>,
    summary: Option<&Summary>,
    email_cache: &mut HashMap<String, Option<String>>,
) -> (Option<String>, Option<String>) {
    let workspace = summary.and_then(|s| s.workspace.as_deref());
    let Some(repo) = workspace.and_then(|w| crate::attribution::find_repo(input.repos, w)) else {
        return (None, None);
    };
    let email = email_cache
        .entry(repo.slug.clone())
        .or_insert_with(|| ralphy_git_read::user_email(Path::new(&repo.path)))
        .clone();
    (Some(repo.slug.clone()), email)
}

#[cfg(test)]
pub(crate) mod tests;
