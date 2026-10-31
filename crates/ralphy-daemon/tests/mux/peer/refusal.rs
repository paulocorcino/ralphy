//! The console a peer refuses: the page's channel ends `refused`, and the
//! local audit log keeps no line for an open that never happened.

use std::path::Path;

use super::{fleet, open, peer_repo, Page};

/// The `event` of each console line of the audit log in `store`.
fn console_events(store: &Path) -> Vec<String> {
    let log = std::fs::read_to_string(store.join("daemon-audit.jsonl")).unwrap_or_default();
    log.lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("the line is JSON"))
        .filter_map(|line| line["event"].as_str().map(str::to_string))
        .filter(|event| event.starts_with("console_"))
        .collect()
}

#[tokio::test]
async fn a_reattach_the_peer_refuses_closes_the_channel_refused() {
    let fleet = fleet().await;
    let mut page = Page::connect(&fleet).await;
    let fields = serde_json::json!({ "id": 999, "repo": peer_repo() });
    open(&mut page.ws, 1, "console", fields).await;
    assert_eq!(page.closed_without_a_frame(1).await, "refused");
}

#[tokio::test]
async fn a_relayed_takeover_the_peer_refuses_writes_no_console_line() {
    let fleet = fleet().await;
    let local_store = fleet.stores[1].path();
    let mut page = Page::connect(&fleet).await;
    // A relayed open the peer accepts writes its line in this log.
    page.launch(1, &peer_repo()).await;
    assert_eq!(console_events(local_store), ["console_launch"]);

    let fields = serde_json::json!({ "id": 999, "repo": peer_repo(), "takeover": 1 });
    open(&mut page.ws, 2, "console", fields).await;
    assert_eq!(page.closed_without_a_frame(2).await, "refused");
    assert_eq!(
        console_events(local_store),
        ["console_launch"],
        "a refused take-over writes no line"
    );
}

#[tokio::test]
async fn a_relayed_launch_the_peer_refuses_writes_no_console_line() {
    let fleet = fleet().await;
    let local_store = fleet.stores[1].path();
    let mut page = Page::connect(&fleet).await;
    // The peer refuses a checkout it does not know: it announces the refusal
    // with a `session-end` and then ends the channel `refused`.
    let fields = serde_json::json!({
        "repo": peer_repo(),
        "agent": "claude",
        "checkout": "no-such-checkout",
    });
    open(&mut page.ws, 1, "console", fields).await;
    let ended = page.command(1, "session-end").await;
    assert_eq!(ended.payload["reason"], "refused", "{ended:?}");
    assert_eq!(page.closed_without_a_frame(1).await, "refused");
    assert_eq!(
        console_events(local_store),
        Vec::<String>::new(),
        "a refused launch writes no line"
    );
}
