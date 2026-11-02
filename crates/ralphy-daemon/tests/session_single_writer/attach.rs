use super::*;

#[tokio::test]
async fn second_attach_needs_takeover_which_evicts_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    // ws1: launch and round-trip a keystroke so it is the attached single writer.
    let mut ws1 = page::console(
        port,
        json!({ "repo": "owner/workbench", "agent": "claude" }),
    )
    .await;
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    // A second attach WITHOUT takeover, while ws1 holds the writer slot, must be
    // refused before any frame — never a silently attached channel.
    refused(
        port,
        json!({ "id": id }),
        "a busy session refuses a non-takeover attach",
    )
    .await;
    // The refusal evicted nobody: ws1 still drives the child.
    ws1.send(terminal(b"still-first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:still-first", false).await;

    // The same attach WITH takeover: 1 succeeds and evicts ws1.
    let mut ws2 = page::console(port, json!({ "id": id, "takeover": 1 })).await;

    // ws1 is evicted: its channel ends within 5s (the server bridge broke and
    // closed its channel) — NOT a hang. And on the way out it is TOLD why, in a
    // data frame read before the stream ends: the close metadata cannot carry
    // that (issue #334), which is what let the old client mistake an eviction for
    // a flaky link and steal the session back.
    let (ended, announced) = drain_capturing_announcement(&mut ws1, 5).await;
    assert!(
        ended,
        "the evicted first client's stream must end, not hang"
    );
    assert_eq!(
        announced.as_deref(),
        Some("taken-over"),
        "the eviction must be ANNOUNCED before the socket closes, naming its reason"
    );
    assert_eq!(ws1.closed_reason(), Some("done"), "the daemon closed ws1");

    // The taker drives the child: a keystroke round-trips (no cursor answer — the
    // child is past startup on this reattach).
    ws2.send(terminal(b"takeover-ok\r")).await.unwrap();
    read_until(&mut ws2, "GOT:takeover-ok", false).await;

    // Close so the helper child (60s-capable in sleep mode) does not linger.
    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}

#[tokio::test]
async fn a_reattach_as_the_same_holder_reclaims_its_own_slot() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _shutdown) = start_daemon(dir.path()).await;

    let launch = json!({ "repo": "owner/workbench", "agent": "claude", "holder": "tab-A" });
    let mut ws1 = page::console(port, launch).await;
    ws1.send(terminal(b"first\r")).await.unwrap();
    read_until(&mut ws1, "GOT:first", true).await;

    let (_, body) = http_request(port, "GET", "/api/sessions").await;
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    let id = list.as_array().unwrap()[0]["id"].as_u64().unwrap();

    refused(
        port,
        json!({ "id": id }),
        "a reattach naming no holder is not the holder",
    )
    .await;
    refused(
        port,
        json!({ "id": id, "holder": "tab-B" }),
        "another tab is not the holder",
    )
    .await;
    // A malformed holder counts as none, so it cannot pass for tab-A.
    refused(
        port,
        json!({ "id": id, "holder": "tab-A&" }),
        "a malformed holder is no holder",
    )
    .await;

    // The launching tab reclaims its own slot without takeover.
    let mut ws2 = page::console(port, json!({ "id": id, "holder": "tab-A" })).await;
    let (ended, announced) = drain_capturing_announcement(&mut ws1, 5).await;
    assert!(ended, "the reclaimed socket's bridge must end");
    assert_eq!(announced.as_deref(), Some("taken-over"));
    assert_eq!(ws1.closed_reason(), Some("done"), "the daemon closed ws1");
    ws2.send(terminal(b"reclaimed\r")).await.unwrap();
    read_until(&mut ws2, "GOT:reclaimed", false).await;

    // The reclaim keeps the name: the slot is still tab-A's, and still nobody else's.
    refused(
        port,
        json!({ "id": id, "holder": "tab-B" }),
        "the reclaimed slot is tab-A's",
    )
    .await;
    drop(ws2);

    // A slot claimed with NO holder is reclaimable by nobody: `None` never matches.
    let mut anon = page::console(port, json!({ "id": id })).await;
    anon.send(terminal(b"anon\r")).await.unwrap();
    read_until(&mut anon, "GOT:anon", false).await;
    refused(
        port,
        json!({ "id": id, "holder": "tab-A" }),
        "a holder cannot claim a nameless slot",
    )
    .await;

    http_request(port, "POST", &format!("/api/sessions/close?id={id}")).await;
}
