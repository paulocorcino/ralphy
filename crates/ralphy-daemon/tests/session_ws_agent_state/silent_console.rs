//! A console whose daemon start check failed is silent at once: its row
//! carries `source: "silent"` and a reason, and no state, until the first
//! hook line.

use super::*;

/// A daemon whose start check failed, and a Claude console on it. Claude is
/// `session_test_child` found on `PATH`, so the check reads the version
/// `2.1.312` and runs the exec form; the hook program does not exist, so the
/// check fails. Returns the port, the console's status file and its socket.
async fn silent_console(
    dir: &std::path::Path,
) -> (
    u16,
    std::path::PathBuf,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join(format!("claude{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_session_test_child"), &claude).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs = vec![bin];
    dirs.extend(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    std::env::set_var("RALPHY_EXE_OVERRIDE", dir.join("no-such-ralphy"));
    tokio::task::spawn_blocking(ralphy_daemon::agent_state::run_start_check)
        .await
        .unwrap();
    let cause = ralphy_daemon::agent_state::start_check_failure()
        .expect("the hook program is missing, so the start check fails");
    assert!(cause.contains("no-such-ralphy"), "{cause}");

    let (port, store) = start_daemon(dir).await;
    let url = format!("ws://127.0.0.1:{port}/ws/session?repo=owner%2Fstatelab&agent=claude");
    let (ws, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("a claude console launches");
    let status = store.join("sessions").join("1.agent-status.jsonl");
    (port, status, ws)
}

#[tokio::test]
async fn a_silent_console_carries_source_and_reason_and_no_state() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _status, _ws) = silent_console(dir.path()).await;
    let block = wait_for_block(port, |b| b.get("source").is_some()).await;
    assert_eq!(block["source"], "silent", "{block}");
    let reason = block["reason"].as_str().unwrap_or("");
    assert!(reason.contains("no-such-ralphy"), "{block}");
    assert!(block.get("state").is_none(), "{block}");
}

#[tokio::test]
async fn the_first_hook_line_clears_the_silence() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let (port, status, _ws) = silent_console(dir.path()).await;
    wait_for_block(port, |b| b["source"] == "silent").await;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&status)
        .unwrap();
    writeln!(
        f,
        r#"{{"event":"UserPromptSubmit","ts":"2026-10-28T10:00:00-03:00"}}"#
    )
    .unwrap();
    let block = wait_for_block(port, |b| b.get("state").is_some()).await;
    assert_eq!(block["state"], "working", "{block}");
    assert!(block.get("source").is_none(), "{block}");
    assert!(block.get("reason").is_none(), "{block}");
}
