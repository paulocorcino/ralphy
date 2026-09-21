//! The session manager must stay HTTP-free (docs/adr/0032 §2; issue #162 AC3):
//! `src/session.rs` drives a PTY and byte streams, referencing no `axum`/
//! WebSocket type. This guard lives in its own file so its own banned-word
//! literals below do not poison the substring check of the source it reads.

#[test]
fn session_module_references_no_http_transport() {
    let sources = [
        (
            "session.rs",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/session.rs")),
        ),
        (
            "session/spec.rs",
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/session/spec.rs")),
        ),
        (
            "session/manager.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/manager.rs"
            )),
        ),
    ];
    for (name, src) in sources {
        for needle in ["axum", "WebSocket"] {
            assert!(
                !src.contains(needle),
                "{name} must not reference `{needle}` — keep the session manager transport-free"
            );
        }
    }
}
