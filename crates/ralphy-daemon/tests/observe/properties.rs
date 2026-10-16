//! `file.stat` over the real `/ws/command` wire (ADR-0036, amendment
//! 2026-10-15): the facts of one path answer on the requesting `id`, at once,
//! with ZERO `status:"spawned"` frames. The seeded repos are the ones of
//! `observe_read.rs`.

use serde_json::{json, Value};

use super::observe_read::{round_trip, serve_checkout_repo, serve_git_repo, serve_repo};

/// One `file.stat` on its own id: the single reply, after the proof that the
/// read never spawned.
async fn stat(url: &str, slug: &str, path: &str, checkout: Option<&str>) -> Value {
    let mut payload = json!({ "repo": slug, "path": path });
    if let Some(name) = checkout {
        payload["checkout"] = json!(name);
    }
    let (mut replies, spawned) = round_trip(url, 1, "file.stat", payload).await;
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    replies.remove(0)
}

/// The reply without its `modified`, after proof that it is a number: the
/// time of a file made a moment ago cannot be pinned.
fn without_modified(mut reply: Value) -> Value {
    let modified = reply.as_object_mut().and_then(|o| o.remove("modified"));
    assert!(
        modified.is_some_and(|m| m.is_u64()),
        "modified must be a number: {reply}"
    );
    reply
}

#[tokio::test]
async fn a_file_answers_its_facts() {
    let (url, slug) = serve_repo().await;

    let reply = stat(&url, &slug, "visible.txt", None).await;

    assert_eq!(
        without_modified(reply),
        json!({
            "status": "ok",
            "name": "visible.txt",
            "kind": "file",
            "size": 5,
            "readonly": false,
            "ignored": false,
            "type": { "class": "text", "encoding": "UTF-8", "bom": false },
        })
    );
}

#[tokio::test]
async fn the_type_is_what_the_file_verbs_would_serve() {
    let (url, slug) = serve_repo().await;
    let want = [
        ("bin.dat", json!({ "class": "binary" })),
        ("logo.png", json!({ "class": "image", "image": "png" })),
        // The bytes belie the extension, so `file.image` refuses it and the
        // type is the text that it is.
        (
            "evil.png",
            json!({ "class": "text", "encoding": "UTF-8", "bom": false }),
        ),
    ];

    for (path, expected) in want {
        let reply = stat(&url, &slug, path, None).await;
        assert_eq!(reply["status"], "ok", "{path}: {reply}");
        assert_eq!(reply["type"], expected, "{path}: {reply}");
    }
}

#[tokio::test]
async fn a_folder_and_the_root_answer_no_size_and_no_type() {
    let (url, slug) = serve_repo().await;

    // `node_modules` is a folder that the tree never lists. A stat is still
    // answered: a size counts what is on the disk.
    for path in ["node_modules", ""] {
        let reply = stat(&url, &slug, path, None).await;
        assert_eq!(reply["status"], "ok", "{path:?}: {reply}");
        assert_eq!(reply["kind"], "folder", "{path:?}: {reply}");
        assert!(reply.get("size").is_none(), "{path:?}: {reply}");
        assert!(reply.get("type").is_none(), "{path:?}: {reply}");
    }
}

#[tokio::test]
async fn the_ignored_mark_is_the_one_of_the_tree() {
    let (url, slug) = serve_git_repo().await;

    let mut marks = Vec::new();
    for path in [".secret", ".secret/key", "visible.txt"] {
        marks.push(stat(&url, &slug, path, None).await["ignored"].clone());
    }

    // An ignored folder, a file inside it, and the control that is not ignored.
    assert_eq!(marks, [json!(true), json!(true), json!(false)]);
}

#[tokio::test]
async fn a_miss_and_an_escape_are_both_not_found() {
    let (url, slug) = serve_repo().await;
    let not_found = json!({ "status": "error", "reason": "not found" });

    // `outer/secret` and `outer/secret.png` are real files just outside the
    // repo, so only confinement can refuse them.
    // `..` is `outer`, a folder that exists: only the stat's own confinement
    // refuses it, the type readers never see a folder.
    for path in ["nope.txt", "../secret", "../secret.png", ".."] {
        assert_eq!(stat(&url, &slug, path, None).await, not_found, "{path}");
    }
    // The control: the same server answers a path that is inside.
    assert_eq!(stat(&url, &slug, "visible.txt", None).await["status"], "ok");
}

#[tokio::test]
async fn a_checkout_is_judged_by_its_own_rules() {
    let (url, slug, _root) = serve_checkout_repo().await;

    // The primary ignores `.ralphy/`, where the worktree lives; the worktree
    // ignores `*.log`. Read from the primary root, every file would be ignored.
    let mut marks = Vec::new();
    for path in ["inner.txt", "sub", "build.log"] {
        let reply = stat(&url, &slug, path, Some("wt-a")).await;
        assert_eq!(reply["status"], "ok", "{path}: {reply}");
        marks.push(reply["ignored"].clone());
    }

    assert_eq!(marks, [json!(false), json!(false), json!(true)]);
}

/// A worktree dir that is a SYMLINK out of the repo is refused as `tree.list`
/// refuses it: the root is resolved through `confine` against the REGISTERED
/// root first.
#[tokio::test]
async fn a_symlinked_checkout_is_refused() {
    let (url, slug, root) = serve_checkout_repo().await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/wt-out\n",
    )
    .unwrap();
    std::fs::write(outside.path().join("secret.txt"), b"leak").unwrap();
    let link = root.join(".ralphy/worktrees/wt-out");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(outside.path(), &link);
    if let Err(e) = linked {
        // A Windows host without the symlink privilege cannot stage the escape;
        // the Linux leg of CI does. Say so rather than pass vacuously.
        eprintln!("SKIPPED: cannot create a directory symlink here: {e}");
        return;
    }
    std::mem::forget(outside);

    let reply = stat(&url, &slug, "secret.txt", Some("wt-out")).await;

    assert_eq!(reply["status"], "error", "must refuse: {reply}");
    assert!(
        reply.get("name").is_none(),
        "leaked through the link: {reply}"
    );
}
