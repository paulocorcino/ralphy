//! The codec must stay transport-agnostic (docs/adr/0032 §5): `src/protocol.rs`
//! turns frames into bytes and back, referencing no HTTP/WS/runtime type. This
//! guard lives in its own file so its own banned-word literals below do not
//! poison the substring check of the source it reads.

#[test]
fn protocol_module_references_no_transport() {
    let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/protocol.rs"));
    for needle in ["axum", "tokio", "WebSocket", "tungstenite", "hyper"] {
        assert!(
            !src.contains(needle),
            "protocol.rs must not reference `{needle}` — keep the codec transport-agnostic"
        );
    }
}
