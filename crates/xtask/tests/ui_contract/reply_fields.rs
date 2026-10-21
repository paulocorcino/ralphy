//! The reply fields of the verbs, checked against the daemon (ADR-0070
//! Compliance): where each ok reply file carries its JSON. Split from
//! `ui_contract/main.rs`, which holds the other checks of the contract.

use regex::Regex;

use super::{fixture_names, read, workspace_root, FIXTURES};

/// Verbs whose ok reply is flat, as the ADR-0036 amendment "the registry
/// verbs" fixes it (the two properties verbs follow it), and the daemon file
/// that writes each key: a `"key"` literal or a `pub key:` field of the
/// serialized struct.
const FLAT_REPLIES: &[(&str, &str)] = &[
    ("file.stat", "crates/ralphy-daemon/src/tree/stat.rs"),
    ("tree.size", "crates/ralphy-daemon/src/tree/size.rs"),
    ("dir.list", "crates/ralphy-daemon/src/dir_list.rs"),
    (
        "file.upload",
        "crates/ralphy-daemon/src/routes/ws_command/oneshot/write.rs",
    ),
    (
        "project.add",
        "crates/ralphy-daemon/src/routes/ws_command/registry_verbs.rs",
    ),
];

/// The daemon files that name the field a Query reply carries its JSON in.
const REPLY_FIELD_SOURCES: &[&str] = &[
    "crates/ralphy-daemon/src/routes/ws_command/oneshot/collect.rs",
    "crates/ralphy-daemon/src/routes/ws_command/host.rs",
];

#[test]
fn every_ok_fixture_carries_its_json_in_the_field_the_daemon_uses() {
    let root = workspace_root();
    let dispatch = read(&root.join("crates/ralphy-daemon/src/dispatch.rs"));
    let daemon: String = REPLY_FIELD_SOURCES
        .iter()
        .map(|f| read(&root.join(f)))
        .collect::<Vec<_>>()
        .join("\n");
    let mut checked = 0;
    for name in fixture_names(&root) {
        let path = root.join(FIXTURES).join(format!("{name}.json"));
        let reply: serde_json::Value = serde_json::from_str(&read(&path))
            .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
        if reply["status"] != "ok" {
            continue;
        }
        let verb = name.split_once("--").map_or(name.as_str(), |(v, _)| v);
        if let Some((_, source)) = FLAT_REPLIES.iter().find(|(v, _)| *v == verb) {
            let src = read(&root.join(source));
            for key in reply.as_object().expect("an ok reply is an object").keys() {
                assert!(
                    key == "status"
                        || src.contains(&format!("\"{key}\""))
                        || src.contains(&format!("pub {key}:")),
                    "{name}.json: `{key}` is not a field {source} writes"
                );
            }
            checked += 1;
            continue;
        }
        let variant = verb_variant(&dispatch, verb)
            .unwrap_or_else(|| panic!("{verb} is not a verb of Verb::from_query"));
        let field = reply_field(&daemon, &variant)
            .unwrap_or_else(|| panic!("the daemon names no reply field for Verb::{variant}"));
        let keys: Vec<&String> = reply
            .as_object()
            .expect("an ok reply is an object")
            .keys()
            .filter(|k| *k != "status")
            .collect();
        assert_eq!(
            keys,
            [&field],
            "{name}.json: the daemon sends {verb} in `{field}`"
        );
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} ok fixtures were checked");
}

#[test]
fn reply_field_scan_reads_both_arm_shapes() {
    let dispatch = r#"fn from_query(value: &str) -> Option<Verb> {
        match value {
            "board.list" => Some(Verb::BoardList),
            _ => None,"#;
    assert_eq!(
        verb_variant(dispatch, "board.list").as_deref(),
        Some("BoardList")
    );
    assert_eq!(verb_variant(dispatch, "nope"), None);
    let daemon = r#"
        dispatch::Verb::BoardList => (Ok(dispatch::board_argv()), "board"),
        Verb::HostKey => Some("key"),"#;
    assert_eq!(reply_field(daemon, "BoardList").as_deref(), Some("board"));
    assert_eq!(reply_field(daemon, "HostKey").as_deref(), Some("key"));
    assert_eq!(reply_field(daemon, "Board"), None);
}

/// The `Verb` variant `Verb::from_query` maps `verb` to.
fn verb_variant(dispatch: &str, verb: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#""{}"\s*=>\s*Some\(Verb::(\w+)\)"#,
        regex::escape(verb)
    ))
    .expect("a valid regex");
    re.captures(dispatch).map(|c| c[1].to_string())
}

/// The last string literal on the match arm of `Verb::<variant>`.
fn reply_field(daemon: &str, variant: &str) -> Option<String> {
    let arm = Regex::new(&format!(r"\bVerb::{variant}\s*=>")).expect("a valid regex");
    let quoted = Regex::new(r#""(\w+)""#).expect("a valid regex");
    let line = daemon.lines().find(|l| arm.is_match(l))?;
    quoted.captures_iter(line).last().map(|c| c[1].to_string())
}
