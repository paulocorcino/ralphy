//! The main daemon sends the run requests of a peer whose machine has no bot
//! (ADR-0085 D14, amendment of 2026-11-03, A4): a run sends its own message
//! with the bot of its machine, so with none it would reach no one. A peer
//! with a bot, and an older peer that does not say, keep their own runs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use super::peer_tests::{fake_peer, main_daemon, peer_reply, real_peer};
use super::*;

/// A run request of the peer's own list, opened `open_secs` ago: its agent
/// waits on issue #71.
fn run_waiting(open_secs: u64) -> AttentionRequest {
    let since = SystemTime::now() - Duration::from_secs(open_secs);
    AttentionRequest {
        id: "run:01RUN".to_owned(),
        kind: Kind::RunWaiting,
        vendor: "codex".to_owned(),
        console: None,
        name: None,
        repo: Some("owner/app".to_owned()),
        run: Some("01RUN".to_owned()),
        issue: Some("71".to_owned()),
        since: chrono::DateTime::<chrono::Utc>::from(since)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        detail: Some("permission: Bash".to_owned()),
        dismissable: false,
    }
}

/// The texts a main with a bot sends over two ticks, for a peer whose own
/// list is `list`.
async fn sent_for(list: Value) -> Vec<String> {
    let port = fake_peer(Arc::new(Mutex::new(Some(list))), false).await;
    let mut main = main_daemon(true);
    main.announce(port);
    main.tick(&[]).await;
    main.tick(&[]).await;
    main.texts.all()
}

#[tokio::test]
async fn a_bot_less_peers_waiting_run_is_sent_once() {
    let texts = sent_for(json!({ "requests": [run_waiting(121)], "own_bot": false })).await;
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert_eq!(
        texts[0],
        "Codex run on issue #71 in owner/app is waiting: permission: Bash"
    );
}

#[tokio::test]
async fn a_peer_with_a_bot_keeps_its_runs() {
    let texts = sent_for(json!({ "requests": [run_waiting(121)], "own_bot": true })).await;
    assert_eq!(texts, Vec::<String>::new(), "its own runs send them");
}

#[tokio::test]
async fn an_older_peer_keeps_its_runs() {
    let texts = sent_for(json!({ "requests": [run_waiting(121)] })).await;
    assert_eq!(texts, Vec::<String>::new(), "an older peer does not say");
}

#[tokio::test]
async fn a_daemon_says_whether_its_machine_has_a_bot() {
    for (with_bot, want) in [(true, true), (false, false)] {
        let home = tempfile::tempdir().expect("a temp dir");
        let port = real_peer(home.path(), with_bot).await;
        let main = main_daemon(false);
        main.announce(port);
        let list = peer_reply(&main, "/api/attention?local=1").await;
        assert_eq!(list["own_bot"], json!(want), "{list}");
        assert!(list["requests"].is_array(), "{list}");
    }
}
