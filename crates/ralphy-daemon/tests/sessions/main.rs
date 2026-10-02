//! Agent-session and console tests over a real loopback WebSocket, one test
//! binary. Every module points the session launcher at the helper child
//! through the same process-wide `RALPHY_DAEMON_AGENT_OVERRIDE` value and sets
//! no other env var, so they can share a process.

mod console_command_ws;
mod console_reattach;
mod console_ws;
mod session_persistence;
mod session_ws;
mod session_ws_checkout;
mod session_ws_cursor;
mod session_ws_gemini;

#[path = "../support/golden.rs"]
mod golden;

/// Open a NEW launch the daemon must refuse, and return the refusal's
/// payload. The browser cannot read a refused upgrade, so the refusal is an
/// upgrade, then ONE `session-end` frame with `reason: "refused"`, then the end
/// of the stream — no terminal byte, so no child ever spoke.
async fn refused(url: &str) -> serde_json::Value {
    use futures_util::StreamExt;
    use ralphy_daemon::protocol::{self, Frame};
    use tokio_tungstenite::tungstenite::Message;

    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a refused NEW launch still upgrades, so the browser can read why");
    let mut payload = None;
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(msg) = ws.next().await {
            let bytes = match msg {
                Ok(Message::Binary(b)) => b,
                Ok(Message::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            match protocol::decode(&bytes).expect("a well-formed frame") {
                Frame::Command(cmd) => {
                    assert_eq!(
                        cmd.verb, "session-end",
                        "the only frame is the end: {cmd:?}"
                    );
                    assert_eq!(cmd.payload["reason"], "refused", "{cmd:?}");
                    assert!(cmd.payload["message"].is_string(), "{cmd:?}");
                    assert!(payload.is_none(), "exactly one refusal frame");
                    payload = Some(cmd.payload);
                }
                other => panic!("a refusal carries no other frame; got {other:?}"),
            }
        }
    })
    .await
    .expect("the daemon closes a refused launch");
    payload.expect("a session-end frame")
}

/// Point the session launcher at `session_test_child` instead of a real agent
/// or shell. The env var is set exactly ONCE: the tests of this binary can run
/// in parallel threads, and a repeated `set_var` across threads is a data race
/// even when every write carries the same value.
fn point_launcher_at_test_child() {
    static OVERRIDE: std::sync::Once = std::sync::Once::new();
    OVERRIDE.call_once(|| {
        std::env::set_var(
            "RALPHY_DAEMON_AGENT_OVERRIDE",
            env!("CARGO_BIN_EXE_session_test_child"),
        );
    });
}
