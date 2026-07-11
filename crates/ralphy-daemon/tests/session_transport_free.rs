//! The session manager must stay HTTP-free (docs/adr/0032 §2; issue #162 AC3):
//! `src/session.rs` drives a PTY and byte streams, referencing no `axum`/
//! WebSocket type. This guard lives in its own file so its own banned-word
//! literals below do not poison the substring check of the source it reads.

#[test]
fn session_module_references_no_http_transport() {
    let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/session.rs"));
    for needle in ["axum", "WebSocket"] {
        assert!(
            !src.contains(needle),
            "session.rs must not reference `{needle}` — keep the session manager transport-free"
        );
    }
}
