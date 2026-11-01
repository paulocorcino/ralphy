use super::*;

/// `checkout` (ADR-0036 amendment, ADR-0063 §2) prefixes the rel under the
/// SAME root: `tree.list` at `""` lists the WORKTREE's root, not the primary's.
#[tokio::test]
async fn tree_list_with_checkout_lists_the_worktree() {
    let (port, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        port,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "", "checkout": "wt-a" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    let names = entry_names(reply);
    assert!(names.contains(&"inner.txt"), "names={names:?}");
    assert!(names.contains(&"sub"), "names={names:?}");
    assert!(
        !names.contains(&"visible.txt"),
        "the primary's file must not leak into the worktree listing: {names:?}"
    );
    assert!(
        !names.contains(&".git"),
        "the pointer file is noise: {names:?}"
    );
}

/// The ignore mark of a checkout listing follows the WORKTREE's rules. The
/// worktree lives under the primary's ignored `.ralphy/`; rules read from the
/// primary would mark every entry of it ignored.
#[tokio::test]
async fn tree_list_with_checkout_marks_ignored_by_the_worktree_rules() {
    let (port, slug, _root) = serve_checkout_repo().await;
    let (replies, _) = round_trip(
        port,
        1,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "", "checkout": "wt-a" }),
    )
    .await;
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    let ignored = |name: &str| {
        reply["entries"]
            .as_array()
            .expect("entries array")
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} not listed: {reply}"))["ignored"]
            .as_bool()
            .expect("ignored is a bool")
    };
    assert!(!ignored("inner.txt"), "reply={reply}");
    assert!(!ignored("sub"), "reply={reply}");
    assert!(ignored("build.log"), "reply={reply}");
}

#[tokio::test]
async fn tree_list_with_checkout_and_subdir_prefixes_the_rel() {
    let (port, slug, _root) = serve_checkout_repo().await;
    let (replies, _) = round_trip(
        port,
        2,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "sub", "checkout": "wt-a" }),
    )
    .await;
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(entry_names(reply), vec!["deep.txt"]);
}

#[tokio::test]
async fn file_read_with_checkout_reads_the_worktree_file() {
    let (port, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        port,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "inner.txt", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(reply["content"], "inside");

    // NEGATIVE CONTROL: the same rel WITHOUT `checkout` is the primary's miss.
    let (replies, _) = round_trip(
        port,
        4,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "inner.txt" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error", "reply={}", replies[0]);
}

/// A hit inside the worktree comes back relative to the WORKTREE (the
/// operator's rel), never with the `.ralphy/worktrees/wt-a/` prefix.
#[tokio::test]
async fn tree_find_with_checkout_answers_unprefixed_paths() {
    let (port, slug, _root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        port,
        5,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "inner", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok", "reply={reply}");
    assert_eq!(
        reply["hits"],
        serde_json::json!([{ "path": "inner.txt", "dir": false }])
    );

    // The same query WITHOUT `checkout` finds it under the primary's rel —
    // the prefix is what the checkout strips, not the walk.
    let (replies, _) = round_trip(
        port,
        6,
        "tree.find",
        serde_json::json!({ "repo": slug, "query": "inner" }),
    )
    .await;
    assert_eq!(
        replies[0]["hits"][0]["path"],
        ".ralphy/worktrees/wt-a/inner.txt"
    );
}

/// A name with no pointer file, or one that fails the shape gate, is refused
/// by NAME with the one message the shell resets its selection on.
#[tokio::test]
async fn unknown_checkout_is_refused_by_name() {
    let (port, slug, root) = serve_checkout_repo().await;
    // Plant a pointer file where `../x` WOULD resolve if the shape gate were
    // skipped and the name joined first (`.ralphy/worktrees/../x/.git`): a
    // gate-less daemon would then prefix, fail `confine` on the `..`, and
    // answer `not found` — not `unknown checkout`. The reply discriminates.
    std::fs::create_dir_all(root.join(".ralphy/x")).unwrap();
    std::fs::write(root.join(".ralphy/x/.git"), "gitdir: /r/.git/worktrees/x\n").unwrap();
    let unknown = serde_json::json!({ "status": "error", "message": "unknown checkout" });
    for (id, name) in [(7u64, "nope"), (8, "../x"), (9, ""), (12, "C:")] {
        let (replies, spawned) = round_trip(
            port,
            id,
            "tree.list",
            serde_json::json!({ "repo": slug, "path": "", "checkout": name }),
        )
        .await;
        assert_eq!(spawned, 0);
        assert_eq!(replies.len(), 1, "one reply for {name:?}");
        assert_eq!(replies[0], unknown, "checkout {name:?}");
    }
    // `null` is the primary tree, not an unknown name.
    let (replies, _) = round_trip(
        port,
        10,
        "tree.list",
        serde_json::json!({ "repo": slug, "path": "", "checkout": null }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert!(entry_names(&replies[0]).contains(&"visible.txt"));
}

/// A Write verb carrying `checkout` is refused (the `.ralphy` denylist would
/// refuse the prefixed path anyway) — and the PRIMARY's file at the same rel
/// is the negative control: the write must not land there either.
#[tokio::test]
async fn file_write_with_checkout_is_refused() {
    let (port, slug, root) = serve_checkout_repo().await;
    let (replies, spawned) = round_trip(
        port,
        11,
        "file.write",
        serde_json::json!({
            "repo": slug,
            "path": "visible.txt",
            "content": "clobbered",
            "checkout": "wt-a",
        }),
    )
    .await;
    assert_eq!(spawned, 0);
    let reply = &replies[0];
    assert_eq!(reply["status"], "error", "reply={reply}");
    assert_eq!(reply["reason"], "refused", "reply={reply}");
    assert_eq!(
        reply["message"],
        "Writing inside a worktree is not available yet."
    );
    assert_eq!(
        std::fs::read_to_string(root.join("visible.txt")).unwrap(),
        "hello",
        "the primary's file must be untouched"
    );
    assert!(
        !root.join(".ralphy/worktrees/wt-a/visible.txt").exists(),
        "nothing lands in the worktree either"
    );
}

/// A worktree dir that is a SYMLINK out of the repo — its target carrying a
/// real `gitdir:` pointer, as any linked worktree elsewhere on the host does —
/// must be refused by the searches exactly as `tree.list` refuses it: the walk
/// root is resolved through `confine` against the REGISTERED root, never
/// handed to the walker as a root that would confine against itself.
#[tokio::test]
async fn search_with_a_symlinked_checkout_is_refused_like_a_listing() {
    let (port, slug, root) = serve_checkout_repo().await;
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

    for (id, verb) in [(20u64, "tree.find"), (21, "tree.grep"), (22, "tree.list")] {
        let payload = if verb == "tree.list" {
            serde_json::json!({ "repo": slug, "path": "", "checkout": "wt-out" })
        } else {
            serde_json::json!({ "repo": slug, "query": "secret", "checkout": "wt-out" })
        };
        let (replies, spawned) = round_trip(port, id, verb, payload).await;
        assert_eq!(spawned, 0);
        let reply = &replies[0];
        assert_eq!(reply["status"], "error", "{verb} must refuse: {reply}");
        assert!(
            reply["hits"].is_null() && reply["entries"].is_null(),
            "{verb} leaked through the link: {reply}"
        );
    }
}

/// Send a tree-channel `watch` of the root of `checkout`.
async fn watch_checkout(ws: &mut page::Channel, slug: &str, checkout: &str) {
    let frame = Frame::Command(Command {
        id: 0,
        verb: "watch".to_string(),
        payload: serde_json::json!({ "repo": slug, "path": "", "checkout": checkout }),
    });
    ws.send(Message::Binary(protocol::encode(&frame).into()))
        .await
        .expect("send the frame");
}

/// The `checkout` of the first `tree.dirty` push within `window`; `None` when
/// no push came.
async fn dirty_checkout(ws: &mut page::Channel, window: Duration) -> Option<String> {
    tokio::time::timeout(window, async {
        while let Some(msg) = ws.next().await {
            let Ok(Message::Binary(bytes)) = msg else {
                continue;
            };
            if let Ok(Frame::Command(cmd)) = protocol::decode(&bytes) {
                if cmd.verb == "tree.dirty" {
                    return cmd.payload["checkout"].as_str().map(String::from);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// A worktree folder that is a symlink out of the repo is not watched, even
/// when the pointer file it holds names a gitdir inside the repo: a change in
/// the linked folder is never pushed. The normal worktree on the same channel
/// is the positive control.
#[tokio::test]
async fn a_symlinked_checkout_gets_no_watch() {
    let (port, slug, root) = serve_checkout_repo().await;
    let outside = tempfile::tempdir().expect("create the temp dir");
    let gitdir = root
        .join(".git/worktrees/wt-out")
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(outside.path().join(".git"), format!("gitdir: {gitdir}\n"))
        .expect("write the pointer");
    let link = root.join(".ralphy/worktrees/wt-out");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(outside.path(), &link);
    #[cfg(not(windows))]
    let linked = std::os::unix::fs::symlink(outside.path(), &link);
    linked.expect("create a directory symlink");

    let mut ws = page::tree(port).await;
    watch_checkout(&mut ws, &slug, "wt-out").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    std::fs::write(outside.path().join("new.txt"), b"x").expect("write new.txt");
    assert_eq!(
        dirty_checkout(&mut ws, Duration::from_secs(3)).await,
        None,
        "a change behind the link must not be pushed"
    );

    watch_checkout(&mut ws, &slug, "wt-a").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    std::fs::write(root.join(".ralphy/worktrees/wt-a/f.txt"), b"x").expect("write f.txt");
    assert_eq!(
        dirty_checkout(&mut ws, Duration::from_secs(10)).await,
        Some("wt-a".to_string()),
        "the normal worktree is still watched"
    );
}
