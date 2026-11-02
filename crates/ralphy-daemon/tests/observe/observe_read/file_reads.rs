use super::*;

#[tokio::test]
async fn image_read_serves_a_png_as_base64() {
    // ADR-0049 §2: one reply on the id, carrying the VERIFIED media type and the
    // bytes base64'd — and, like every Observe verb, zero spawns.
    let (port, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        5,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "logo.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "an Observe read must never spawn");
    let reply = &replies[0];
    assert_eq!(reply["status"], "ok");
    assert_eq!(reply["mediaType"], "image/png");
    let decoded = data_encoding::BASE64
        .decode(
            reply["base64"]
                .as_str()
                .expect("a base64 string")
                .as_bytes(),
        )
        .expect("the reply's base64 decodes");
    assert_eq!(decoded, PNG_BYTES, "the bytes survive the round trip");
}

#[tokio::test]
async fn image_read_refuses_bytes_that_belie_the_extension() {
    // The magic-byte check over the wire (ADR-0049 §3): HTML named `.png` is
    // refused, never handed to the browser labelled `image/png`.
    let (port, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        6,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "evil.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "not an image");
    assert!(
        replies[0].get("base64").is_none(),
        "a refusal carries no bytes: {:?}",
        replies[0]
    );
}

#[tokio::test]
async fn image_read_masks_traversal_as_not_found() {
    // Confinement is unchanged by ADR-0049: an out-of-root image read is a plain
    // miss, never leaking whether the target exists (ADR-0036 §5).
    let (port, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        7,
        "file.image",
        serde_json::json!({ "repo": slug, "path": "../secret.png" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    assert_eq!(replies[0]["status"], "error");
    assert_eq!(replies[0]["reason"], "not found");
}

#[tokio::test]
async fn file_read_masks_traversal_as_not_found() {
    // A `..` traversal over the wire must return a plain "not found", never
    // leaking whether the out-of-root target exists (ADR-0036 §5).
    let (port, slug) = serve_repo().await;
    let (replies, spawned) = round_trip(
        port,
        3,
        "file.read",
        serde_json::json!({ "repo": slug, "path": "../secret" }),
    )
    .await;

    assert_eq!(replies.len(), 1, "exactly one reply on the id");
    assert_eq!(spawned, 0, "a refused read must never spawn");
    let reason = replies[0]["reason"].as_str().expect("a reason string");
    assert_eq!(replies[0]["status"], "error");
    assert!(reason.contains("not found"), "reason={reason:?}");
}
