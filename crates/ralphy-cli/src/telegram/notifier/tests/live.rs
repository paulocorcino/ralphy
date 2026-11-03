//! The opt-in proof against the live Bot API.

use super::*;

/// Live, opt-in proof against the real Bot API that the no-op-edit fix holds:
/// run with
/// `cargo test -p ralphy-cli -- --ignored live_edit_dedup_against_real_telegram --nocapture`.
/// It uses the operator's stored token + chat (auto-skips if unconfigured),
/// sends a card, edits it with CHANGED text (must succeed), then edits with
/// IDENTICAL text (the Bot API rejects this with "message is not modified" —
/// the exact bug), and finally confirms `render_card` is byte-identical across
/// two unchanged renders, so the worker's `card != last_card` guard skips it.
#[test]
#[ignore = "hits the live Telegram Bot API; needs `telegram setup` first"]
fn live_edit_dedup_against_real_telegram() {
    use crate::telegram::client::UreqTransport;
    use crate::telegram::config::{effective_token, load};

    let Some(cfg) = load().expect("load config") else {
        eprintln!("SKIP: Telegram not configured — run `ralphy telegram setup`");
        return;
    };
    let Some(chat_id) = cfg.chat_id else {
        eprintln!("SKIP: no chat captured — run `ralphy telegram setup`");
        return;
    };
    let token = effective_token(Some(&cfg.token)).expect("a token");
    let client = BotClient::new(UreqTransport::new(token));

    // A run state matching the stuck-in-planning scenario from the bug report.
    let mut state = RunState::new("🔬 ralphy dedup self-test", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "W0: planning (live notifier self-test)".into(),
    });

    // 1) Send the initial card and capture its message_id.
    let card_v1 = render_card(&state, now_epoch());
    let sent = client.send_message(chat_id, &card_v1).expect("send card");
    let mid = sent["message_id"].as_i64().expect("message_id");
    eprintln!("sent card message_id={mid}");

    // 2) A genuinely changed render must edit successfully.
    state.apply(RunEvent::Executing {
        number: 1,
        budget_min: 45,
        model: String::new(),
        effort: None,
    });
    let card_v2 = render_card(&state, now_epoch());
    assert_ne!(card_v1, card_v2, "state change should alter the render");
    client
        .edit_message_text(chat_id, mid, &card_v2)
        .expect("changed edit must succeed");
    eprintln!("changed edit OK");

    // 3) Re-editing with the SAME body is exactly what Telegram rejects — this
    // documents the root cause the guard exists to avoid.
    let err = client
        .edit_message_text(chat_id, mid, &card_v2)
        .expect_err("identical edit must be rejected by Telegram");
    let msg = err.to_string();
    eprintln!("identical edit rejected as expected: {msg}");
    assert!(
        msg.contains("message is not modified"),
        "expected the not-modified rejection, got: {msg}"
    );

    // 4) The guard's premise: two unchanged renders are byte-identical, so
    // `card != last_card` is false and the worker never makes call (3).
    let card_again = render_card(&state, now_epoch());
    assert_eq!(
        card_v2, card_again,
        "unchanged state must render identically — the guard relies on this"
    );
    eprintln!("PASS: unchanged render is identical → idle refresh is skipped");
}
