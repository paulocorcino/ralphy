//! `file.upload` through the `round_trip` helper of `workspace_write.rs`
//! (ADR-0036 amendment 2026-10-19): parts land in `.ralphy/uploads/`, the
//! last part moves the file to its target, and a worktree is refused like
//! every other Write.

use super::workspace_write::{round_trip, serve_repo, serve_worktree_repo};

const ID: &str = "abcdefabcdefabcdefabcdefabcdef01";

fn b64(bytes: &[u8]) -> String {
    data_encoding::BASE64.encode(bytes)
}

#[tokio::test]
async fn two_parts_land_one_file_and_a_resend_learns_the_offset() {
    let (url, slug, root) = serve_repo().await;
    let part = |offset: u64, data: &[u8]| {
        serde_json::json!({
            "repo": slug, "path": "up.bin", "id": ID,
            "offset": offset, "total": 6, "data": b64(data),
        })
    };
    let (replies, spawned) = round_trip(&url, 1, "file.upload", part(0, b"abc")).await;
    assert_eq!(spawned, 0, "a Write must never spawn");
    assert_eq!(replies[0]["status"], "ok", "{:?}", replies[0]);
    assert_eq!(replies[0]["received"], 3);
    assert!(!root.join("up.bin").exists(), "not before the last part");

    let (replies, _) = round_trip(&url, 2, "file.upload", part(0, b"abc")).await;
    assert_eq!(replies[0]["reason"], "offset", "{:?}", replies[0]);
    assert_eq!(replies[0]["expected"], 3);

    let (replies, _) = round_trip(&url, 3, "file.upload", part(3, b"def")).await;
    assert_eq!(replies[0]["status"], "ok", "{:?}", replies[0]);
    assert_eq!(replies[0]["path"], "up.bin");
    assert_eq!(replies[0]["result"], "created");
    super::golden::check("file.upload", replies[0].clone(), &[]);
    assert_eq!(std::fs::read(root.join("up.bin")).unwrap(), b"abcdef");
}

#[tokio::test]
async fn an_upload_under_a_checkout_is_refused() {
    let (url, slug, root) = serve_worktree_repo().await;
    let (replies, _) = round_trip(
        &url,
        1,
        "file.upload",
        serde_json::json!({
            "repo": slug, "path": "docs/up.txt", "id": ID,
            "offset": 0, "total": 1, "data": b64(b"x"), "checkout": "wt-a",
        }),
    )
    .await;
    assert_eq!(replies[0]["reason"], "refused", "{:?}", replies[0]);
    assert!(
        !root.join(".ralphy/uploads").exists(),
        "no part was written"
    );
}
