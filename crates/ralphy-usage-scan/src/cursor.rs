//! The Cursor module of the usage scan (ADR-0033 §2/§6, ADR-0042 D11). Enumerates
//! the interactive sessions Cursor left on disk under `~/.cursor/` and reports
//! each one with **no token count at all**.
//!
//! Tokens are `None`, not zero: Cursor bills in dollar-denominated credits and
//! records no per-session token totals anywhere in either store (ADR-0042 D11 —
//! the resolved model id and any counts live only in the content-addressed blob
//! graph, which this scan deliberately does not walk). A zeroed [`Tokens`](crate::Tokens) would
//! ship `0` on the wire and read as "this session spent nothing"; `None` forces
//! every consumer to render it as unavailable. `model` is the literal `"unknown"`
//! for the same reason.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;

use crate::{CursorScan, InteractiveRecord};

/// Scan the Cursor store into interactive records — one per session, always with
/// `tokens: None`. A missing or unreadable `cursor_dir` yields an empty vec (not
/// an error). Sessions whose id is in `run_session_ids` are Ralphy runs', never
/// interactive, and are excluded. `since` drops records whose `last_ts` is
/// strictly before it (§6: an unparseable bound or record keeps the record).
pub fn scan_cursor(input: &CursorScan<'_>) -> Vec<InteractiveRecord> {
    // Keyed by session id so the two stores union rather than duplicate.
    let mut by_id: BTreeMap<String, InteractiveRecord> = BTreeMap::new();
    // slug → resolved git actor email, computed at most once per attributed repo
    // (mirrors `claude.rs`/`opencode.rs`): the resolver spawns `git`, and Cursor
    // stores routinely hold hundreds of sessions in one repo.
    let mut email_cache: HashMap<String, Option<String>> = HashMap::new();
    // Order is load-bearing: `chats` first, and the transcripts leg only fills
    // ids it did not already claim, so a session in both stores keeps the
    // `meta.json` timestamps and cwd.
    scan_chats(input, &mut by_id, &mut email_cache);
    scan_transcripts(input, &mut by_id, &mut email_cache);

    let mut records: Vec<InteractiveRecord> = by_id.into_values().collect();
    if let Some(since) = input.since {
        if let Ok(since_dt) = chrono::DateTime::parse_from_rfc3339(since) {
            records.retain(|r| match chrono::DateTime::parse_from_rfc3339(&r.last_ts) {
                Ok(last) => last >= since_dt,
                Err(_) => true, // never hide a session on a parse miss
            });
        }
    }
    records
}

/// Walk `<cursor_dir>/chats/<hash>/<sid>/meta.json`. This store carries real unix-ms
/// timestamps and the verbatim `cwd`, so its record wins any collision with the
/// transcripts store.
fn scan_chats(
    input: &CursorScan<'_>,
    out: &mut BTreeMap<String, InteractiveRecord>,
    email_cache: &mut HashMap<String, Option<String>>,
) {
    let Ok(hashes) = fs::read_dir(input.cursor_dir.join("chats")) else {
        return;
    };
    let sessions = hashes
        .flatten()
        .filter_map(|hash| fs::read_dir(hash.path()).ok())
        .flat_map(Iterator::flatten);
    for session in sessions {
        let session_id = session.file_name().to_string_lossy().to_string();
        if input.run_session_ids.contains(&session_id) {
            continue;
        }
        if let Some(record) = chat_record(input, email_cache, &session.path(), session_id) {
            out.insert(record.session_id.clone(), record);
        }
    }
}

/// The record of one `chats/<hash>/<sid>/` session from its `meta.json`, or
/// `None` when the file is missing or is not JSON.
fn chat_record(
    input: &CursorScan<'_>,
    email_cache: &mut HashMap<String, Option<String>>,
    session_dir: &Path,
    session_id: String,
) -> Option<InteractiveRecord> {
    let text = fs::read_to_string(session_dir.join("meta.json")).ok()?;
    let meta = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let ms = |k: &str| meta.get(k).and_then(serde_json::Value::as_i64);
    let cwd = meta.get("cwd").and_then(|v| v.as_str());
    let matched = cwd.and_then(|c| crate::attribution::find_repo(input.repos, c));
    let (project, actor_email) = attribute(email_cache, matched);
    Some(InteractiveRecord {
        agent: "cursor".to_owned(),
        model: "unknown".to_owned(),
        session_id,
        project,
        actor_email,
        tokens: None,
        first_ts: ms_to_rfc3339(ms("createdAtMs")),
        last_ts: ms_to_rfc3339(ms("updatedAtMs")),
        lower_bound: false,
    })
}

/// Walk `<cursor_dir>/projects/<slug>/agent-transcripts/<sid>/<sid>.jsonl`. Six ids
/// live only here (measured on this host), so scanning `chats` alone would hide
/// them. This store carries no machine-readable instant — its only in-band
/// timestamp is human prose inside a user message — so the `<sid>.jsonl` mtime is
/// the honest cross-platform floor for both ends of the span.
fn scan_transcripts(
    input: &CursorScan<'_>,
    out: &mut BTreeMap<String, InteractiveRecord>,
    email_cache: &mut HashMap<String, Option<String>>,
) {
    let Ok(projects) = fs::read_dir(input.cursor_dir.join("projects")) else {
        return;
    };
    for project_dir in projects.flatten() {
        scan_project_transcripts(input, out, email_cache, &project_dir.path());
    }
}

/// Adds the sessions of one `projects/<slug>/agent-transcripts/` dir that no
/// earlier record claimed. Every session of the dir shares the attribution of
/// the repo whose slug matches `<slug>`.
fn scan_project_transcripts(
    input: &CursorScan<'_>,
    out: &mut BTreeMap<String, InteractiveRecord>,
    email_cache: &mut HashMap<String, Option<String>>,
    project_dir: &Path,
) {
    let slug_dir = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let matched = input
        .repos
        .iter()
        .find(|r| cursor_project_slug(&r.path).eq_ignore_ascii_case(&slug_dir));
    let (project, actor_email) = attribute(email_cache, matched);
    let Ok(sessions) = fs::read_dir(project_dir.join("agent-transcripts")) else {
        return;
    };
    for session in sessions.flatten() {
        let session_id = session.file_name().to_string_lossy().to_string();
        if input.run_session_ids.contains(&session_id) || out.contains_key(&session_id) {
            continue;
        }
        let transcript = session.path().join(format!("{session_id}.jsonl"));
        if !transcript.is_file() {
            continue;
        }
        let ts = ms_to_rfc3339(mtime_ms(&transcript));
        out.insert(
            session_id.clone(),
            InteractiveRecord {
                agent: "cursor".to_owned(),
                model: "unknown".to_owned(),
                session_id,
                project: project.clone(),
                actor_email: actor_email.clone(),
                tokens: None,
                first_ts: ts.clone(),
                last_ts: ts,
                lower_bound: false,
            },
        );
    }
}

/// A file's mtime as unix ms; `None` on any metadata or range error.
fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    i64::try_from(dur).ok()
}

/// Cursor's `projects/` directory-name encoding of a workspace path: every
/// non-alphanumeric byte becomes `-` and consecutive `-` COLLAPSE — Cursor names
/// `C:\Dev\FinCal` as `C-Dev-FinCal`, not Claude's `C--Dev-FinCal`
/// (`claude.rs::dashed_cwd`), so the two encodings cannot share one helper.
fn cursor_project_slug(path: &str) -> String {
    // Trailing separators are trimmed first, mirroring
    // `attribution::normalize_path`: a repo registered as `C:\Dev\FinCal\` must
    // slug to the same `C-Dev-FinCal`.
    let mut out = String::with_capacity(path.len());
    for ch in path.trim_end_matches(['/', '\\']).chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out
}

/// `(project slug, git actor email)` for `matched`; `(None, None)` when `None`
/// (§6: reported, never dropped). The email is resolved through `cache`, so one
/// `git` spawn serves every session of a repo rather than one per session.
fn attribute(
    cache: &mut HashMap<String, Option<String>>,
    matched: Option<&crate::RegisteredRepo>,
) -> (Option<String>, Option<String>) {
    match matched {
        Some(r) => (
            Some(r.slug.clone()),
            cache
                .entry(r.slug.clone())
                .or_insert_with(|| ralphy_git_read::user_email(Path::new(&r.path)))
                .clone(),
        ),
        None => (None, None),
    }
}

/// A unix-ms instant → RFC3339 UTC; empty string when absent or out of range.
/// Duplicated from `opencode.rs` (ADR-0033 §7 accepts per-vendor duplication).
fn ms_to_rfc3339(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
