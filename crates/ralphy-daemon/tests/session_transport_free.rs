//! Two source-text guards, one per transport-free module. They live in a test
//! file, not in `src/`, so their own banned-word literals do not poison the
//! substring check of the source they read.
//!
//! - The session manager must stay HTTP-free (docs/adr/0032 §2; issue #162
//!   AC3): `src/session.rs` and its `spec`/`manager` children drive a PTY and
//!   byte streams, referencing no `axum`/WebSocket type.
//! - The codec must stay transport-agnostic (docs/adr/0032 §5):
//!   `src/protocol.rs` turns frames into bytes and back, referencing no
//!   HTTP/WS/runtime type.
#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test crate is the test module"
)]

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
        (
            "session/manager/attachment.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/manager/attachment.rs"
            )),
        ),
        (
            "session/manager/id_seq.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/manager/id_seq.rs"
            )),
        ),
        (
            "session/manager/query.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/manager/query.rs"
            )),
        ),
        (
            "session/spec/naming.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/spec/naming.rs"
            )),
        ),
        (
            "session/spec/console.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/session/spec/console.rs"
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
