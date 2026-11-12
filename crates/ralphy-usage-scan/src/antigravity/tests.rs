use super::*;
use crate::RegisteredRepo;
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The listing `agy models` printed in the spike: `<id>\t<label>`, 13 rows.
const MODELS: &str = include_str!("../../tests/fixtures/agy-models.txt");

pub(crate) const ARGON: &str = "Gemini 4 Argon (Medium)";
const FLASH: &str = "Gemini 3.8 Flash (Low)";

fn varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("seven bits fit a byte") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("a value under 0x80 fits a byte"));
}

fn uint_field(field: u64, value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    varint(field << 3, &mut out);
    varint(value, &mut out);
    out
}

fn bytes_field(field: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    varint((field << 3) | 2, &mut out);
    varint(u64::try_from(bytes.len()).expect("a small field"), &mut out);
    out.extend_from_slice(bytes);
    out
}

/// One `gen_metadata.data` row in the shape of the spike store log: field 1
/// holds the usage at 1.4 (2 input, 3 output, 5 cache read, with the other
/// counts beside them), the backend id at 1.19 and the label at 1.21.
pub(crate) fn row(input: u64, output: u64, cache_read: u64, label: &str) -> Vec<u8> {
    let usage = [
        uint_field(1, 1418),
        uint_field(2, input),
        uint_field(3, output),
        uint_field(5, cache_read),
        uint_field(6, 24),
        uint_field(9, 11),
        uint_field(10, 66),
    ]
    .concat();
    let call = [
        bytes_field(4, &usage),
        bytes_field(19, b"gemini-4-argon-trial"),
        bytes_field(21, label.as_bytes()),
    ]
    .concat();
    bytes_field(1, &call)
}

/// Write `<root>/antigravity-cli/conversations/<id>.db` with these rows.
pub(crate) fn seed_conversation(root: &Path, id: &str, rows: &[Vec<u8>]) {
    let dir = root.join("antigravity-cli").join("conversations");
    std::fs::create_dir_all(&dir).expect("the conversations folder");
    let conn = Connection::open(dir.join(format!("{id}.db"))).expect("the fixture opens");
    conn.execute_batch(
        "CREATE TABLE `gen_metadata` (`idx` integer,`data` blob,\
         `size` integer NOT NULL DEFAULT 0,PRIMARY KEY (`idx`))",
    )
    .expect("the fixture schema runs");
    for (idx, data) in rows.iter().enumerate() {
        conn.execute(
            "INSERT INTO gen_metadata (idx, data, size) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                i64::try_from(idx).expect("a small index"),
                data,
                i64::try_from(data.len()).expect("a small row")
            ],
        )
        .expect("the fixture row");
    }
}

/// Write `<root>/antigravity-cli/conversation_summaries.db` with one row per
/// `(conversation id, workspace_uris, last_modified_time)`.
fn seed_summaries(root: &Path, rows: &[(&str, &str, &str)]) {
    let dir = root.join("antigravity-cli");
    std::fs::create_dir_all(&dir).expect("the app folder");
    let conn = Connection::open(dir.join("conversation_summaries.db")).expect("the fixture");
    conn.execute_batch(
        "CREATE TABLE `conversation_summaries` (`conversation_id` text,\
         `title` text NOT NULL DEFAULT \"\",`last_modified_time` datetime NOT NULL,\
         `workspace_uris` text NOT NULL,PRIMARY KEY (`conversation_id`))",
    )
    .expect("the fixture schema runs");
    for (id, uris, modified) in rows {
        conn.execute(
            "INSERT INTO conversation_summaries \
             (conversation_id, workspace_uris, last_modified_time) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, uris, modified],
        )
        .expect("the fixture row");
    }
}

fn scan_with(
    root: &Path,
    runs: &HashSet<String>,
    repos: &[RegisteredRepo],
) -> Vec<InteractiveRecord> {
    scan_antigravity(&AntigravityScan {
        root,
        run_session_ids: runs,
        repos,
        since: None,
    })
}

fn scan(root: &Path) -> Vec<InteractiveRecord> {
    scan_with(root, &HashSet::new(), &[])
}

fn tokens(record: &InteractiveRecord) -> (u64, u64, u64, u64) {
    let t = record.tokens.as_ref().expect("an agy record has counts");
    (t.input, t.output, t.cache_read, t.cache_creation)
}

#[test]
fn scan_antigravity_sums_gen_metadata_rows_per_conversation() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let root = tmp.path();
    seed_conversation(
        root,
        "8adbb1bf",
        &[
            row(8293, 196, 0, ARGON),
            row(3315, 77, 6239, ARGON),
            row(2123, 375, 8147, ARGON),
        ],
    );
    seed_conversation(
        root,
        "cf415564",
        &[row(8325, 50, 0, FLASH), row(8451, 38, 0, FLASH)],
    );

    let mut records = scan(root);
    records.sort_by(|a, b| a.session_id.cmp(&b.session_id));

    assert_eq!(records.len(), 2, "one record per conversation: {records:?}");
    let argon = &records[0];
    assert_eq!(argon.agent, "antigravity");
    assert_eq!(argon.session_id, "8adbb1bf");
    assert_eq!(argon.model, "gemini-4-argon-medium");
    assert_eq!(tokens(argon), (13731, 648, 14386, 0));
    assert!(!argon.lower_bound, "the store has every count agy keeps");
    let flash = &records[1];
    assert_eq!(flash.session_id, "cf415564");
    assert_eq!(flash.model, "gemini-3.8-flash-low");
    assert_eq!(tokens(flash), (16776, 88, 0, 0));
}

#[test]
fn scan_antigravity_maps_each_label_of_agy_models_to_its_id() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let listing: Vec<(&str, &str)> = MODELS
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    assert_eq!(listing.len(), 13, "the fixture is the 13-row listing");
    for (n, (_, label)) in listing.iter().enumerate() {
        seed_conversation(tmp.path(), &format!("conv-{n:02}"), &[row(10, 1, 0, label)]);
    }

    let records = scan(tmp.path());

    assert_eq!(records.len(), 13);
    for (n, (id, label)) in listing.iter().enumerate() {
        let session = format!("conv-{n:02}");
        let record = records
            .iter()
            .find(|r| r.session_id == session)
            .unwrap_or_else(|| panic!("no record for {label}"));
        assert_eq!(record.model, *id, "the label {label}");
    }
}

#[test]
fn scan_antigravity_skips_a_conversation_with_no_usage() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    seed_conversation(tmp.path(), "cut-before-the-call", &[]);
    seed_conversation(tmp.path(), "zero-counts", &[row(0, 0, 0, FLASH)]);
    seed_conversation(tmp.path(), "used", &[row(10, 1, 0, FLASH)]);

    let records = scan(tmp.path());

    let sessions: Vec<&str> = records.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(sessions, ["used"]);
}

#[test]
fn scan_antigravity_of_a_missing_root_is_empty() {
    let tmp = tempfile::tempdir().expect("a temp dir");

    assert_eq!(scan(&tmp.path().join("no-such-root")), Vec::new());
    std::fs::create_dir_all(tmp.path().join("antigravity-cli")).expect("an app folder");
    assert_eq!(scan(tmp.path()), Vec::new(), "no conversations folder");
}

#[test]
fn a_row_the_decoder_cannot_read_gives_model_unknown() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    // A length that runs past the end of the row.
    let truncated = vec![0x0a, 0x40, 0x22, 0x02];
    seed_conversation(tmp.path(), "garbage", &[truncated]);
    seed_conversation(
        tmp.path(),
        "odd-label",
        &[row(10, 1, 0, "A label with no effort")],
    );

    let mut records = scan(tmp.path());
    records.sort_by(|a, b| a.session_id.cmp(&b.session_id));

    assert_eq!(records.len(), 2, "{records:?}");
    let garbage = &records[0];
    assert_eq!(garbage.model, "unknown");
    assert_eq!(tokens(garbage), (0, 0, 0, 0));
    assert!(
        garbage.lower_bound,
        "a call whose counts are unreadable makes a floor"
    );
    let odd = &records[1];
    assert_eq!(
        odd.model, "unknown",
        "a label outside the rule is not guessed"
    );
    assert_eq!(tokens(odd), (10, 1, 0, 0));
}

#[test]
fn scan_antigravity_attributes_a_conversation_by_its_workspace_uri() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let repo: PathBuf = tmp.path().join("my repo");
    std::fs::create_dir_all(&repo).expect("the repo folder");
    let fwd = repo
        .to_string_lossy()
        .replace('\\', "/")
        .replace(' ', "%20");
    let uri = if fwd.starts_with('/') {
        format!("file://{fwd}")
    } else {
        format!("file:///{fwd}")
    };
    let root = tmp.path().join("root");
    seed_conversation(&root, "in-repo", &[row(10, 1, 0, FLASH)]);
    seed_summaries(&root, &[("in-repo", &uri, "2026-11-12 12:10:43.5+00:00")]);
    let repos = [RegisteredRepo {
        slug: "o/repo".into(),
        path: repo.to_string_lossy().into_owned(),
    }];

    let records = scan_with(&root, &HashSet::new(), &repos);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].project.as_deref(), Some("o/repo"));
    assert_eq!(records[0].last_ts, "2026-11-12T12:10:43.500+00:00");
}

#[test]
fn scan_antigravity_excludes_run_owned_conversations() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    seed_conversation(tmp.path(), "run", &[row(10, 1, 0, FLASH)]);
    seed_conversation(tmp.path(), "interactive", &[row(10, 1, 0, FLASH)]);
    let runs: HashSet<String> = ["run".to_owned()].into();

    let records = scan_with(tmp.path(), &runs, &[]);

    let sessions: Vec<&str> = records.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(sessions, ["interactive"]);
}
