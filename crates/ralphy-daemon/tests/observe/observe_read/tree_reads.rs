use super::*;

#[tokio::test]
async fn tree_list_answers_on_id_without_spawn() {
    let (port, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    let names: Vec<&str> = reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"visible.txt"), "names={names:?}");
    assert!(
        !names.contains(&"node_modules"),
        "noise filtered: {names:?}"
    );
}

#[tokio::test]
async fn tree_list_surfaces_committed_dotfolders_and_ralphy() {
    // A4 oracle (issue #203): `tree.list` at the repo root surfaces committed
    // dot-folders (`.github`), `.ralphy`, and — since ADR-0036's 2026-07-26
    // amendment — gitignored entries (`.secret`); only the `HARD_EXCLUDE` noise
    // dirs (`.git`, `node_modules`, `target`) are dropped.
    let (port, slug) = serve_git_repo().await;
    let (replies, spawned) = round_trip(
        port,
        4,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    let names: Vec<&str> = reply["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&".github"),
        "committed dot-folder: {names:?}"
    );
    assert!(names.contains(&".ralphy"), "`.ralphy` surfaced: {names:?}");
    assert!(!names.contains(&".git"), "`.git` dropped: {names:?}");
    assert!(
        !names.contains(&"node_modules"),
        "noise filtered: {names:?}"
    );
    assert!(!names.contains(&"target"), "noise filtered: {names:?}");
    // ADR-0036, amendment 2026-07-26: gitignored entries are LISTED. The operator
    // works in the ignored files, and `file.read` served them all along.
    assert!(names.contains(&".secret"), "gitignored listed: {names:?}");
}

/// `tree.find` (ADR-0036 amendment 2026-09-15) answers on the id without a
/// spawn, with `/`-joined rel paths, and with the TREE's policy: an ignored
/// dotfolder is found (the tree shows it), the noise dirs never are.
#[tokio::test]
async fn tree_find_answers_on_id_without_spawn() {
    let (port, slug) = serve_git_repo().await;
    let (replies, spawned) = round_trip(
        port,
        1,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "KEY" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe search must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(reply["truncated"], false);
    let paths: Vec<&str> = reply["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| h["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec![".secret/key"], "paths={paths:?}");

    // NEGATIVE CONTROL: a name inside a noise dir is not found.
    let (replies, _) = round_trip(
        port,
        2,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "junk" }),
    )
    .await;
    assert_eq!(replies[0]["hits"].as_array().unwrap().len(), 0);
}

/// `tree.grep` consults `.gitignore` — the one policy divergence from the
/// tree — but `.ralphy/` is always searched, and the count is per occurrence.
#[tokio::test]
async fn tree_grep_respects_gitignore_but_searches_ralphy() {
    let (port, slug) = serve_git_repo().await;
    // "plan" is in BOTH `.ralphy/plan.md` (ignored, always searched) and
    // `.secret/key` (ignored, honoured): only the former is a hit.
    let (replies, spawned) = round_trip(
        port,
        1,
        "tree.grep",
        serde_json::json!({ "repo": slug, "query": "PLAN" }),
    )
    .await;

    assert_eq!(replies.len(), 1);
    assert_eq!(spawned, 0, "an Observe search must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    let hits: Vec<(&str, u64)> = reply["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .map(|h| (h["path"].as_str().unwrap(), h["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(hits, vec![(".ralphy/plan.md", 1)], "hits={hits:?}");

    // NEGATIVE CONTROL: `tree.find` on the same repo DOES see the ignored
    // file — the two policies differ on purpose.
    let (replies, _) = round_trip(
        port,
        2,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "key" }),
    )
    .await;
    assert_eq!(replies[0]["hits"][0]["path"], ".secret/key");
}

/// An unknown repo is refused the same way every Observe verb is.
#[tokio::test]
async fn tree_find_refuses_an_unknown_repo() {
    let (port, _slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        1,
        "tree.find",
        serde_json::json!({ "repo": "owner/nowhere", "query": "vis" }),
    )
    .await;
    assert_eq!(spawned, 0);
    assert_eq!(replies[0]["status"], "error");
}
