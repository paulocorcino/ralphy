use super::*;

/// The note round trip end to end (ADR-0064 §5): `note.write` creates the
/// landing directory the denylist otherwise owns, the bytes on disk are the
/// container and not the markdown, and `note.read` answers with the text.
#[tokio::test]
async fn note_write_then_read_round_trips_through_the_container() {
    let (url, slug, root) = serve_worktree_repo().await;
    let markdown = "---\ncolor: sage\n---\n# Standup\n\nrotate the staging token\n";
    let (replies, spawned) = round_trip(
        &url,
        1,
        "note.write",
        serde_json::json!({
            "repo": slug,
            "path": ".ralphy/notes/standup.note",
            "markdown": markdown,
        }),
    )
    .await;
    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a Write must never spawn");
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);

    let bytes = std::fs::read(root.join(".ralphy/notes/standup.note")).unwrap();
    assert!(bytes.starts_with(b"RNOT"), "the container's magic");
    assert!(
        !bytes.windows(7).any(|w| w == b"staging"),
        "the markdown must not be readable in the file"
    );

    let (replies, _) = round_trip(
        &url,
        2,
        "note.read",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/standup.note" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert_eq!(replies[0]["markdown"], markdown);
}

/// The verb fixes the target CLASS, not the path: the operator may keep a note
/// in a committable directory, and nothing but a `.note` is a note.
#[tokio::test]
async fn note_verbs_take_any_directory_but_only_a_note_extension() {
    let (url, slug, root) = serve_worktree_repo().await;
    std::fs::create_dir(root.join("docs")).unwrap();
    let (replies, _) = round_trip(
        &url,
        1,
        "note.write",
        serde_json::json!({ "repo": slug, "path": "docs/plan.note", "markdown": "# Plan\n" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert!(root.join("docs/plan.note").exists());

    for path in [
        "docs/plan.md",
        ".ralphy/notes/x.md",
        ".ralphy/settings.json",
    ] {
        let (replies, _) = round_trip(
            &url,
            2,
            "note.write",
            serde_json::json!({ "repo": slug, "path": path, "markdown": "# no\n" }),
        )
        .await;
        assert_eq!(replies[0]["status"], "error", "{path}");
        assert_eq!(replies[0]["reason"], "not a note", "{path}");
    }
    assert!(!root.join("docs/plan.md").exists());

    // A file that is not a container refuses on its bytes, not its name.
    std::fs::write(root.join("docs/fake.note"), b"# plain").unwrap();
    let (replies, _) = round_trip(
        &url,
        3,
        "note.read",
        serde_json::json!({ "repo": slug, "path": "docs/fake.note" }),
    )
    .await;
    assert_eq!(replies[0]["reason"], "not a note", "reply={}", replies[0]);
}

/// A malformed payload must never be the way a note is emptied.
#[tokio::test]
async fn note_write_without_markdown_is_refused() {
    let (url, slug, root) = serve_worktree_repo().await;
    round_trip(
        &url,
        1,
        "note.write",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/a.note", "markdown": "# Kept\n" }),
    )
    .await;
    let (replies, _) = round_trip(
        &url,
        2,
        "note.write",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/a.note" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error", "reply={}", replies[0]);
    assert_eq!(replies[0]["reason"], "refused");
    // Read it BACK rather than sniffing the magic: a refusal that nevertheless
    // rewrote the note would leave a container there too.
    let (replies, _) = round_trip(
        &url,
        3,
        "note.read",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/a.note" }),
    )
    .await;
    assert_eq!(
        replies[0]["markdown"],
        "# Kept
"
    );
    assert!(root.join(".ralphy/notes/a.note").exists());
}

/// The missing-file state's wire half (ADR-0064 §11): the card branches on
/// this exact literal to paint itself, so it is pinned end to end and not only
/// in `NoteError`'s string table.
#[tokio::test]
async fn note_read_of_an_absent_file_is_not_found() {
    let (url, slug, _root) = serve_worktree_repo().await;
    let (replies, _) = round_trip(
        &url,
        1,
        "note.read",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/gone.note" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error", "reply={}", replies[0]);
    assert_eq!(replies[0]["reason"], "not found");
    // And a note under a checkout that resolves, but is not there either.
    let (replies, _) = round_trip(
        &url,
        2,
        "note.read",
        serde_json::json!({ "repo": slug, "path": "docs/gone.note", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(replies[0]["reason"], "not found");
}

/// `note.write` is the ONE Write a `checkout` is honoured on (ADR-0064 §5):
/// it saves into the worktree's own tree, while its Write siblings are still
/// refused there. The negative control is what makes this a decision and not
/// an accident.
#[tokio::test]
async fn note_write_honours_a_checkout_while_file_write_is_still_refused() {
    let (url, slug, root) = serve_worktree_repo().await;
    let (replies, _) = round_trip(
        &url,
        1,
        "note.write",
        serde_json::json!({
            "repo": slug,
            "path": "docs/wt.note",
            "markdown": "# In the worktree\n",
            "checkout": "wt-a",
        }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert!(
        root.join(".ralphy/worktrees/wt-a/docs/wt.note").exists(),
        "the note landed in the worktree"
    );
    assert!(
        !root.join("docs/wt.note").exists(),
        "and never on the primary's file at the same rel"
    );

    // Read it back through the same checkout.
    let (replies, _) = round_trip(
        &url,
        2,
        "note.read",
        serde_json::json!({ "repo": slug, "path": "docs/wt.note", "checkout": "wt-a" }),
    )
    .await;
    assert_eq!(replies[0]["markdown"], "# In the worktree\n");

    // The negative control: a generic Write under the same checkout is refused.
    let (replies, _) = round_trip(
        &url,
        3,
        "file.write",
        serde_json::json!({
            "repo": slug,
            "path": "docs/wt.txt",
            "content": "x",
            "checkout": "wt-a",
        }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "refused");

    // And a checkout that does not resolve is refused, not silently dropped.
    let (replies, _) = round_trip(
        &url,
        4,
        "note.write",
        serde_json::json!({
            "repo": slug,
            "path": "docs/wt.note",
            "markdown": "# no\n",
            "checkout": "nope",
        }),
    )
    .await;
    assert_eq!(replies[0]["status"], "error", "reply={}", replies[0]);
    assert_eq!(replies[0]["message"], "unknown checkout");
}

/// The explorer's rename and delete reach a note through the denylist's one
/// carve-out — the whole reason there is no `note.rename`/`note.delete`.
#[tokio::test]
async fn the_generic_byte_ops_reach_a_note_in_the_landing_dir() {
    let (url, slug, root) = serve_worktree_repo().await;
    round_trip(
        &url,
        1,
        "note.write",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/a.note", "markdown": "# A\n" }),
    )
    .await;
    let (replies, _) = round_trip(
        &url,
        2,
        "file.rename",
        serde_json::json!({
            "repo": slug,
            "path": ".ralphy/notes/a.note",
            "to": ".ralphy/notes/b.note",
        }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    let (replies, _) = round_trip(
        &url,
        3,
        "file.delete",
        serde_json::json!({ "repo": slug, "path": ".ralphy/notes/b.note" }),
    )
    .await;
    assert_eq!(replies[0]["status"], "ok", "reply={}", replies[0]);
    assert!(!root.join(".ralphy/notes/b.note").exists());

    // The rest of `.ralphy` is exactly as closed as it was.
    let (replies, _) = round_trip(
        &url,
        4,
        "file.write",
        serde_json::json!({ "repo": slug, "path": ".ralphy/settings.json", "content": "{}" }),
    )
    .await;
    assert_eq!(replies[0]["reason"], "refused", "reply={}", replies[0]);
}
