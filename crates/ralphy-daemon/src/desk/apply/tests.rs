use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::desk::CONSOLE_NAME_MAX;

const NOW: i64 = 5_000;

/// The table the browser runs too (`wb-desk-sync.test.mjs`): one rule in two
/// languages, held by one file.
const CASES: &str = include_str!("../../../ui-tests/fixtures/api-desk--apply-cases.json");

/// `ts` is the server's clock, which the browser never sees: both sides
/// compare without it.
fn without_ts(mut d: DeskStore) -> DeskStore {
    d.windows.iter_mut().for_each(|w| w.ts = 0);
    d.fences.iter_mut().for_each(|f| f.ts = 0);
    d.notes.iter_mut().for_each(|n| n.ts = 0);
    d
}

#[test]
fn every_shared_case_gives_the_desk_the_browser_expects() {
    let cases: Vec<serde_json::Value> = serde_json::from_str(CASES).expect("the cases are JSON");
    assert!(cases.len() >= 15, "the table lost cases: {}", cases.len());
    for case in cases {
        let name = case["name"].as_str().expect("a case has a name");
        let desk: DeskStore = serde_json::from_value(case["desk"].clone()).expect(name);
        let expect: DeskStore = serde_json::from_value(case["expect"].clone()).expect(name);
        let changes = case["changes"].as_array().expect(name);
        let out = apply(desk, changes, &BTreeMap::new(), NOW);
        assert!(out.refused.is_empty(), "{name}: {:?}", out.refused);
        assert_eq!(without_ts(out.desk), without_ts(expect), "{name}");
        assert_eq!(out.changed, case["changed"] == true, "{name}: changed");
    }
}

fn window(id: &str) -> serde_json::Value {
    json!({
        "id": id, "repo": "o/r", "agent": "console", "kind": "console",
        "rect": { "left": 10.0, "top": 20.0, "width": 640.0, "height": 480.0 },
        "max": false, "sessionId": null, "ts": 1,
    })
}

fn create(kind: &str, record: serde_json::Value) -> serde_json::Value {
    json!({ "op": "create", "type": kind, "record": record })
}

fn desk_of(changes: &[serde_json::Value]) -> DeskStore {
    apply(DeskStore::default(), changes, &BTreeMap::new(), 1).desk
}

#[test]
fn a_refused_change_is_named_and_the_others_still_apply() {
    let mut off = window("w-off");
    off["rect"]["left"] = json!(-1.0);
    let changes = [
        create("window", window("w-1")),
        create("window", off),
        json!({ "op": "set", "type": "window", "id": "w-1", "fields": { "ts": 9 } }),
        json!({ "op": "teleport", "id": "w-1" }),
        json!({ "op": "set", "type": "window", "id": "w-1", "fields": { "checkout": "../up" } }),
        json!({ "op": "checkout", "repo": "o/r", "name": "a/b" }),
        json!({ "op": "set", "type": "window", "id": "w-1", "fields": { "locked": true } }),
    ];
    let out = apply(DeskStore::default(), &changes, &BTreeMap::new(), NOW);
    let refused: Vec<usize> = out.refused.iter().map(|r| r.index).collect();
    assert_eq!(refused, [1, 2, 3, 4, 5], "{:?}", out.refused);
    assert!(out.refused[0]
        .error
        .contains("record w-off has an out-of-frame rect"));
    assert!(
        out.refused[1].error.contains("unknown field `ts`"),
        "{:?}",
        out.refused[1]
    );
    assert!(out.refused[2].error.contains("unknown op teleport"));
    assert!(out.refused[3]
        .error
        .contains("checkout ../up on record w-1 is not a valid name"));
    assert!(out.refused[4]
        .error
        .contains("checkout a/b for o/r is not a valid name"));
    assert_eq!(out.desk.windows.len(), 1);
    assert!(
        out.desk.windows[0].locked,
        "the change after the refusals applied"
    );
    assert!(out.desk.checkouts.is_empty());
}

#[test]
fn a_change_stamps_the_server_time_and_a_no_op_does_not() {
    let desk = desk_of(&[
        create("window", window("w-1")),
        create("window", window("w-2")),
    ]);
    let out = apply(
        desk,
        &[
            json!({ "op": "set", "type": "window", "id": "w-1", "fields": { "max": true } }),
            json!({ "op": "set", "type": "window", "id": "w-2", "fields": { "max": false } }),
        ],
        &BTreeMap::new(),
        NOW,
    );
    assert_eq!(out.desk.windows[0].ts, NOW);
    assert_eq!(
        out.desk.windows[1].ts, 1,
        "a set to the stored value keeps its ts"
    );
}

#[test]
fn a_window_create_past_the_cap_and_its_slack_is_refused() {
    let windows: Vec<serde_json::Value> = (0..DESK_MAX + DESK_CREATE_SLACK + 1)
        .map(|n| create("window", window(&format!("w-{n}"))))
        .collect();
    let out = apply(DeskStore::default(), &windows, &BTreeMap::new(), NOW);
    assert_eq!(out.desk.windows.len(), DESK_MAX + DESK_CREATE_SLACK);
    assert_eq!(out.refused.len(), 1, "{:?}", out.refused);
    assert!(out.refused[0].error.contains("consoles"));
}

#[test]
fn a_change_names_its_project_by_the_canonical_key() {
    let aliases = BTreeMap::from([("path-abc".to_string(), "owner/repo".to_string())]);
    let mut stale = window("w-1");
    stale["repo"] = json!("path-abc");
    let mut desk = DeskStore::default();
    desk.checkouts.insert("owner/repo".into(), "wt-a".into());
    desk.notes.push(DeskNote {
        id: "n-1".into(),
        repo: "owner/repo".into(),
        ..DeskNote::default()
    });
    let out = apply(
        desk,
        &[
            create("window", stale),
            json!({ "op": "checkout-clear", "repo": "path-abc", "ifName": "wt-a" }),
            json!({ "op": "checkout", "repo": "path-abc", "name": "wt-b" }),
            json!({ "op": "set", "type": "note", "id": "n-1", "fields": { "file": { "repo": "path-abc", "path": "a.note" } } }),
        ],
        &aliases,
        NOW,
    );
    assert!(out.refused.is_empty(), "{:?}", out.refused);
    assert_eq!(out.desk.windows[0].repo, "owner/repo");
    assert_eq!(
        out.desk.checkouts,
        BTreeMap::from([("owner/repo".to_string(), "wt-b".to_string())]),
        "the clear matched the canonical key, then the new pick landed there"
    );
    assert_eq!(out.desk.notes[0].repo, "owner/repo");
}

#[test]
fn a_create_cuts_a_long_console_name_by_char() {
    let mut long = window("w-1");
    long["consoleName"] = json!("é".repeat(CONSOLE_NAME_MAX + 5));
    let desk = desk_of(&[create("window", long)]);
    let name = desk.windows[0].console_name.clone().expect("a name");
    assert_eq!(name.chars().count(), CONSOLE_NAME_MAX);
    let out = apply(
        desk,
        &[
            json!({ "op": "set", "type": "window", "id": "w-1", "fields": { "consoleName": "x".repeat(60) } }),
        ],
        &BTreeMap::new(),
        NOW,
    );
    assert_eq!(
        out.desk.windows[0].console_name.as_deref().map(str::len),
        Some(CONSOLE_NAME_MAX)
    );
}
