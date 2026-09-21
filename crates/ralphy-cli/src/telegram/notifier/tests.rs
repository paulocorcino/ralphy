use super::*;
use crate::delivery::run_delivery_worker;
use crate::runstate::UsageLite;
use anyhow::{bail, Result};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Mutex;

/// Drive a [`TelegramEngine`] over the shared worker loop inline (or on a spawned
/// thread), the test seam replacing the old free `run_worker`: build the engine
/// from the same inputs and hand it to [`run_delivery_worker`].
fn drive_worker<T: Transport>(
    client: BotClient<T>,
    chat_id: i64,
    state: RunState,
    queue: Arc<EventQueue>,
    shutdown: Arc<AtomicBool>,
) {
    let engine = TelegramEngine {
        client,
        chat_id,
        state,
        message_id: None,
        last_card: String::new(),
        last_edit: Instant::now(),
        sleep_notice_id: None,
        ping: None,
        prev_sleeping: false,
        prev_degraded: false,
        net_warned: false,
    };
    run_delivery_worker(engine, queue, shutdown);
}

/// Live, opt-in demo that the notifier updates ONE message in place: it sends a
/// card then edits it repeatedly with visibly-changing content (issues
/// advancing + a live clock), ~2s apart, so the operator watches it animate.
/// Run with `cargo test -p ralphy-cli -- --ignored live_animate_card --nocapture`.
#[test]
#[ignore = "hits the live Telegram Bot API; needs `telegram setup` first"]
fn live_animate_card() {
    use crate::telegram::client::UreqTransport;
    use crate::telegram::config::{effective_token, TelegramConfig};

    let Some(cfg) = TelegramConfig::load().expect("load config") else {
        eprintln!("SKIP: Telegram not configured — run `ralphy telegram setup`");
        return;
    };
    let Some(chat_id) = cfg.chat_id else {
        eprintln!("SKIP: no chat captured — run `ralphy telegram setup`");
        return;
    };
    let token = effective_token(Some(&cfg.token)).expect("a token");
    let client = BotClient::new(UreqTransport::new(token));

    // Three issues; we walk them through planning → executing → done so each
    // rendered card differs from the last (no "message is not modified").
    let total = 3u64;
    let mut state = RunState::new("🔬 ralphy live card", total as usize);
    let card0 = render_card(&state, now_epoch());
    let sent = client.send_message(chat_id, &card0).expect("send card");
    let mid = sent["message_id"].as_i64().expect("message_id");
    eprintln!("animating message_id={mid}");

    let mut last_card = card0;
    // A helper that edits only when the render actually changed — the same guard
    // the worker uses — and reports each attempt.
    let push = |client: &BotClient<UreqTransport>, state: &RunState, last: &mut String| {
        let card = render_card(state, now_epoch());
        if &card == last {
            eprintln!("  (unchanged — skipped, as the worker would)");
            return;
        }
        match client.edit_message_text(chat_id, mid, &card) {
            Ok(_) => {
                *last = card;
                eprintln!("  edit OK");
            }
            Err(e) => eprintln!("  edit FAILED: {e}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    };

    for n in 1..=total {
        state.apply(RunEvent::IssueStarted {
            number: n,
            title: format!("W{} live step", n - 1),
        });
        push(&client, &state, &mut last_card);

        state.apply(RunEvent::Executing {
            number: n,
            budget_min: 45,
            model: String::new(),
            effort: None,
        });
        push(&client, &state, &mut last_card);

        state.apply(RunEvent::IssueClosed {
            number: n,
            tokens: 0,
            invocations: 0,
            usage: UsageLite::default(),
        });
        push(&client, &state, &mut last_card);
    }

    state.final_summary = Some("✅ live demo finished".into());
    state.finished = true;
    push(&client, &state, &mut last_card);
    eprintln!("done — final card left on the message");
}

/// Live, opt-in proof against the real Bot API that the no-op-edit fix holds:
/// run with `cargo test -p ralphy-cli -- --ignored live_edit_dedup_against_real_telegram --nocapture`.
/// It uses the operator's stored token + chat (auto-skips if unconfigured),
/// sends a card, edits it with CHANGED text (must succeed), then edits with
/// IDENTICAL text (the Bot API rejects this with "message is not modified" —
/// the exact bug), and finally confirms `render_card` is byte-identical across
/// two unchanged renders, so the worker's `card != last_card` guard skips it.
#[test]
#[ignore = "hits the live Telegram Bot API; needs `telegram setup` first"]
fn live_edit_dedup_against_real_telegram() {
    use crate::telegram::client::UreqTransport;
    use crate::telegram::config::{effective_token, TelegramConfig};

    let Some(cfg) = TelegramConfig::load().expect("load config") else {
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

/// A recording transport: records every call and returns a fresh `message_id`
/// for each `sendMessage`. Cloning shares the call log and id counter so a test
/// can inspect what the worker did after the thread joins.
#[derive(Clone)]
struct RecordingTransport {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    next_id: Arc<AtomicI64>,
    fail_edit: bool,
}

impl RecordingTransport {
    fn new() -> Self {
        RecordingTransport {
            calls: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicI64::new(100)),
            fail_edit: false,
        }
    }
}

impl Transport for RecordingTransport {
    fn get(&self, method: &str) -> Result<Value> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), Value::Null));
        Ok(json!({ "ok": true, "result": { "username": "ralphy_bot" } }))
    }

    fn post(&self, method: &str, body: Value) -> Result<Value> {
        self.calls.lock().unwrap().push((method.to_string(), body));
        match method {
            "sendMessage" => {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "ok": true, "result": { "message_id": id } }))
            }
            "editMessageText" if self.fail_edit => bail!("edit boom"),
            _ => Ok(json!({ "ok": true, "result": {} })),
        }
    }
}

fn methods(calls: &[(String, Value)]) -> Vec<&str> {
    calls.iter().map(|(m, _)| m.as_str()).collect()
}

#[test]
fn should_edit_respects_change_and_60s_floor() {
    let interval = Duration::from_secs(60);
    // A change always edits, regardless of elapsed time.
    assert!(should_edit(true, Duration::from_secs(0), interval));
    // Idle below the floor does not edit.
    assert!(!should_edit(false, Duration::from_secs(59), interval));
    // Idle at/after the floor edits.
    assert!(should_edit(false, Duration::from_secs(60), interval));
    assert!(should_edit(false, Duration::from_secs(120), interval));
}

#[test]
fn should_notify_truth_table() {
    assert!(should_notify(true, false, false));
    assert!(!should_notify(false, false, false));
    assert!(!should_notify(true, true, false));
    assert!(!should_notify(true, false, true));
}

#[test]
fn worker_sends_one_card_then_edits_in_place_no_pushes() {
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    queue.push(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    queue.push(RunEvent::Executing {
        number: 1,
        budget_min: 45,
        model: String::new(),
        effort: None,
    });
    queue.push(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        invocations: 0,
        usage: UsageLite::default(),
    });

    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let state = RunState::new("title", 1);
    let handle =
        std::thread::spawn(move || drive_worker(client, 7, state, worker_queue, worker_shutdown));

    shutdown.store(true, Ordering::SeqCst);
    queue.wake();
    handle.join().unwrap();

    let calls = calls.lock().unwrap();
    let m = methods(&calls);
    // Two sendMessages: the card itself, plus one disposable `🔔` progress ping
    // that fires on the genuine card edit (an edit is silent, so it buzzes the
    // phone). No start/final pushes; every card change is an in-place edit.
    let sends = m.iter().filter(|&&x| x == "sendMessage").count();
    assert_eq!(sends, 2, "card + one progress ping: {m:?}");
    assert!(
        send_texts(&calls).iter().any(|t| t.as_str() == "🔔"),
        "a progress ping was sent: {:?}",
        send_texts(&calls)
    );
    assert_eq!(m.first(), Some(&"sendMessage"));
    assert!(m.contains(&"editMessageText"));
    // The run ends on an edit (the terminal footer): the ping is deleted before
    // the terminal edit, never left as the last call.
    assert_eq!(m.last(), Some(&"editMessageText"));

    // Every edit targets the card's message_id (the first sendMessage's id).
    let edit_ids: Vec<i64> = calls
        .iter()
        .filter(|(method, _)| method == "editMessageText")
        .map(|(_, body)| body["message_id"].as_i64().unwrap())
        .collect();
    assert!(!edit_ids.is_empty());
    assert!(edit_ids.iter().all(|&id| id == 100));
}

/// Block (bounded) until `pred` holds over the recorded calls, so the sleep
/// test waits for the worker to fold one event before enqueuing the next
/// without a fixed sleep. Panics if it never holds (a real regression).
fn wait_until(calls: &Arc<Mutex<Vec<(String, Value)>>>, pred: impl Fn(&[(String, Value)]) -> bool) {
    for _ in 0..200 {
        if pred(&calls.lock().unwrap()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("condition never held within timeout");
}

fn send_texts(calls: &[(String, Value)]) -> Vec<String> {
    calls
        .iter()
        .filter(|(m, _)| m == "sendMessage")
        .map(|(_, b)| b["text"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The `message_id`s targeted by `deleteMessage` calls, in order.
fn delete_ids(calls: &[(String, Value)]) -> Vec<i64> {
    calls
        .iter()
        .filter(|(m, _)| m == "deleteMessage")
        .filter_map(|(_, b)| b["message_id"].as_i64())
        .collect()
}

#[test]
fn worker_pushes_on_sleep_enter_and_resume() {
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let state = RunState::new("title", 1);
    let handle =
        std::thread::spawn(move || drive_worker(client, 7, state, worker_queue, worker_shutdown));

    // Enter a sleep, then wait for the worker to fold it and buzz the phone.
    queue.push(RunEvent::SleepStarted {
        reset: "14:30".into(),
        target_epoch: 1_700_000_000,
    });
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("usage limit"))
    });

    // Resume, then wait for the disposable notice to be deleted (no resume push).
    queue.push(RunEvent::SleepEnded);
    queue.wake();
    wait_until(&calls, |c| !delete_ids(c).is_empty());

    shutdown.store(true, Ordering::SeqCst);
    queue.wake();
    handle.join().unwrap();

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    assert!(
        texts.iter().any(|t| t.contains("usage limit")),
        "sleep notice sent: {texts:?}"
    );
    // Resume no longer posts a "resuming" message — it deletes the notice; the
    // card's resume edit may fire a disposable `🔔`, which is fine.
    assert!(
        !texts.iter().any(|t| t.contains("resuming")),
        "resume posts no lingering message: {texts:?}"
    );
    // The sleep notice (send #2, id 101) is deleted on resume.
    assert!(
        delete_ids(&calls).contains(&101),
        "resume deletes the sleep notice: {:?}",
        delete_ids(&calls)
    );
}

#[test]
fn worker_fires_notice_and_delete_when_sleep_events_co_batch() {
    // A SleepStarted immediately followed by a SleepEnded drained in ONE batch
    // nets to `sleep = None`; per-event edge detection must still fire the
    // sleep-in notice AND its delete (a batch-to-batch compare would swallow both).
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    // Inline run: shutdown already set, so the first drain takes both events.
    let shutdown = Arc::new(AtomicBool::new(true));

    queue.push(RunEvent::SleepStarted {
        reset: "14:30".into(),
        target_epoch: 1_700_000_000,
    });
    queue.push(RunEvent::SleepEnded);

    drive_worker(client, 7, RunState::new("t", 1), queue.clone(), shutdown);

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    assert!(
        texts.iter().any(|t| t.contains("usage limit")),
        "sleep notice fired: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("resuming")),
        "resume no longer posts a message: {texts:?}"
    );
    // The notice (send #2, id 101) is deleted on the resume edge.
    assert_eq!(delete_ids(&calls), vec![101], "notice deleted on resume");
}

#[test]
fn worker_pushes_on_api_degraded_and_recover_edges() {
    // The matched-pair edge (issue #149): one buzz on the false→true degraded
    // edge, one on the true→false recover edge — mirrors the sleep-edge test.
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let state = RunState::new("title", 1);
    let handle =
        std::thread::spawn(move || drive_worker(client, 7, state, worker_queue, worker_shutdown));

    queue.push(RunEvent::ApiDegraded);
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("API degraded"))
    });

    queue.push(RunEvent::ApiRecovered);
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("API recovered"))
    });

    shutdown.store(true, Ordering::SeqCst);
    queue.wake();
    handle.join().unwrap();

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    let degraded_idx = texts
        .iter()
        .position(|t| t.contains("API degraded"))
        .expect("degraded push");
    let recover_idx = texts
        .iter()
        .position(|t| t.contains("API recovered"))
        .expect("recover push");
    assert!(
        degraded_idx < recover_idx,
        "degraded push must precede recover push: {texts:?}"
    );
    // initial card + degraded + recover = three sendMessage calls.
    assert_eq!(
        texts.len(),
        3,
        "expected exactly 3 sendMessage, got {texts:?}"
    );
}

#[test]
fn worker_reap_after_degraded_never_claims_recovery() {
    // The trap this guards (docs/adr/0038): an idle reap also clears `degraded`,
    // so the true→false edge would fire the recover push — telling the operator
    // "API recovered, resuming" about a child Ralphy had just killed for going
    // silent. The reap must speak for itself and suppress that edge.
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let state = RunState::new("title", 1);
    let handle =
        std::thread::spawn(move || drive_worker(client, 7, state, worker_queue, worker_shutdown));

    queue.push(RunEvent::ApiDegraded);
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("API degraded"))
    });

    queue.push(RunEvent::IdleReaped { idle_minutes: 20 });
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("child reaped"))
    });

    shutdown.store(true, Ordering::SeqCst);
    queue.wake();
    handle.join().unwrap();

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    assert!(
        !texts.iter().any(|t| t.contains("API recovered")),
        "a reap must never be reported as a recovery: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("no progress for 20 min")),
        "the reap push carries its window: {texts:?}"
    );
    // initial card + degraded + reap = three sendMessage calls.
    assert_eq!(
        texts.len(),
        3,
        "expected exactly 3 sendMessage, got {texts:?}"
    );
}

#[test]
fn worker_lone_api_recover_pushes_nothing() {
    // A lone `ApiRecovered` with no prior degraded folded is a no-op: matched
    // pairs only (`prev_degraded` starts false), so no recover buzz fires.
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(true)); // run inline: drain then finish.

    queue.push(RunEvent::ApiRecovered);

    drive_worker(client, 7, RunState::new("t", 1), queue.clone(), shutdown);

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    assert!(
        !texts.iter().any(|t| t.contains("API recovered")),
        "a lone recover must not push: {texts:?}"
    );
    // Only the initial card was sent.
    assert_eq!(texts.len(), 1, "expected only the card: {texts:?}");
}

#[test]
fn worker_swallows_edit_error_and_finishes_cleanly() {
    let mut transport = RecordingTransport::new();
    transport.fail_edit = true;
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(true)); // run inline: drain then finish.

    queue.push(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    queue.push(RunEvent::NonGreen {
        number: 1,
        outcome: "Stuck".into(),
    });

    drive_worker(client, 7, RunState::new("t", 1), queue.clone(), shutdown);

    let calls = calls.lock().unwrap();
    let m = methods(&calls);
    // The failing edit was swallowed, not fatal: the worker still attempted the
    // edit and returned. Only the card was sent (no pushes exist to fall back on).
    assert!(m.contains(&"editMessageText"));
    let sends = m.iter().filter(|&&x| x == "sendMessage").count();
    assert_eq!(sends, 1, "only the card is sent: {m:?}");
}

#[test]
fn worker_terminal_edit_adds_footer_as_the_last_call() {
    // With no state-changing events the idle loop makes no edit (an identical
    // body would be rejected as "message is not modified"). The one terminal
    // edit is the `finished` flip growing the footer — a genuine change — and it
    // is the LAST call: there is no final push after it. A run with no folded
    // issue processed nothing, so that footer is the `🛑` stopped marker (never
    // the celebratory `🏁 … ✅ 0 done`).
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(true));

    drive_worker(client, 7, RunState::new("idle", 1), queue, shutdown);

    let calls = calls.lock().unwrap();
    let m = methods(&calls);
    // Initial card (sent once), then exactly one terminal footer edit — last.
    assert_eq!(m.first(), Some(&"sendMessage"));
    assert_eq!(m.last(), Some(&"editMessageText"));
    let edits: Vec<&Value> = calls
        .iter()
        .filter(|(method, _)| method == "editMessageText")
        .map(|(_, body)| body)
        .collect();
    assert_eq!(edits.len(), 1, "exactly one terminal footer edit: {m:?}");
    let edited_text = edits[0]["text"].as_str().unwrap_or("");
    assert!(
        edited_text.contains("🛑")
            && edited_text.contains("stopped before any issue was processed"),
        "terminal edit must carry the stopped footer: {edited_text}"
    );
}

#[test]
fn progress_edit_fires_ping_coalesces_then_expires_and_deletes() {
    // Drive the engine directly so the ping lifecycle is exercised without real
    // 2s waits: a genuine card edit posts a `🔔`, a burst coalesces into it, and
    // once aged past PING_TTL the next tick deletes it.
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let mut engine = TelegramEngine {
        client,
        chat_id: 7,
        state: RunState::new("t", 1),
        message_id: None,
        last_card: String::new(),
        last_edit: Instant::now(),
        sleep_notice_id: None,
        ping: None,
        prev_sleeping: false,
        prev_degraded: false,
        net_warned: false,
    };
    engine.on_start(); // card sent, id 100

    // A genuine progress change, then a tick that edits the card and pings.
    engine.on_event(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    engine.on_tick(true);
    assert!(engine.ping.is_some(), "ping pending after a progress edit");
    let ping_count = |calls: &Arc<Mutex<Vec<(String, Value)>>>| {
        send_texts(&calls.lock().unwrap())
            .iter()
            .filter(|t| t.as_str() == "🔔")
            .count()
    };
    assert_eq!(ping_count(&calls), 1, "one ping on the first progress edit");

    // A second edit while the ping is still live coalesces — no second buzz.
    engine.on_event(RunEvent::Executing {
        number: 1,
        budget_min: 45,
        model: String::new(),
        effort: None,
    });
    engine.on_tick(true);
    assert_eq!(ping_count(&calls), 1, "a burst coalesces into one ping");

    // Age the ping past its TTL; the next tick deletes it and clears the slot.
    let (id, _) = engine.ping.expect("ping still pending");
    engine.ping = Some((id, Instant::now() - PING_TTL - Duration::from_millis(1)));
    engine.on_tick(false);
    assert!(engine.ping.is_none(), "expired ping cleared");
    assert_eq!(
        delete_ids(&calls.lock().unwrap()),
        vec![id],
        "expired ping deleted"
    );
}

#[test]
fn progress_ping_is_suppressed_while_sleeping() {
    // The countdown card re-renders each refresh while parked; that edit must not
    // ping every minute — the disposable sleep notice already buzzes.
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let mut engine = TelegramEngine {
        client,
        chat_id: 7,
        state: RunState::new("t", 1),
        message_id: None,
        last_card: String::new(),
        last_edit: Instant::now(),
        sleep_notice_id: None,
        ping: None,
        prev_sleeping: false,
        prev_degraded: false,
        net_warned: false,
    };
    engine.on_start();
    engine.on_event(RunEvent::SleepStarted {
        reset: "14:30".into(),
        target_epoch: 1_700_000_000,
    });
    engine.on_tick(true);
    assert!(engine.ping.is_none(), "no progress ping while sleeping");
    assert!(
        !send_texts(&calls.lock().unwrap())
            .iter()
            .any(|t| t.as_str() == "🔔"),
        "sleeping card edits do not ping"
    );
}

#[test]
fn short_reason_collapses_the_multiline_network_chain() {
    // The exact shape the run reported: a DNS failure whose anyhow chain repeats
    // the OS message three times over two lines. It must collapse to one clause.
    let dns = anyhow::anyhow!(
        "editMessageText request failed: Dns Failed: resolve dns name \
         'api.telegram.org:443': host not known (os error 11001): host not known \
         (os error 11001)"
    );
    let r = short_reason(&dns);
    assert_eq!(r, "network unreachable (DNS)", "got: {r}");
    assert!(!r.contains('\n'), "reason must be one line: {r}");

    // A connect timeout (os error 10060) is the other outage face.
    let timeout = anyhow::anyhow!("sendMessage request failed: Network Error (os error 10060)");
    assert_eq!(short_reason(&timeout), "network unreachable (timeout)");

    // A genuine API rejection is NOT a network drop: keep it legible (first line),
    // never silently reclassified as "unreachable".
    let api = anyhow::anyhow!("sendMessage failed: Bad Request: chat not found");
    assert_eq!(
        short_reason(&api),
        "sendMessage failed: Bad Request: chat not found"
    );
}

#[test]
fn gate_warns_once_then_resets_on_success() {
    // A wedged network fails every edit; the gate must warn on the FIRST failure
    // only (net_warned latches), and a later successful call clears the latch so a
    // genuinely new drop warns again. Only edits fail here, so a successful send
    // (the ping) is the recovery that resets the gate.
    let mut transport = RecordingTransport::new();
    transport.fail_edit = true;
    let client = BotClient::new(transport);
    let mut engine = TelegramEngine {
        client,
        chat_id: 7,
        state: RunState::new("t", 1),
        message_id: None,
        last_card: String::new(),
        last_edit: Instant::now(),
        sleep_notice_id: None,
        ping: None,
        prev_sleeping: false,
        prev_degraded: false,
        net_warned: false,
    };
    engine.on_start(); // send ok → gate clear
    assert!(!engine.net_warned);

    // A failing edit latches the gate.
    engine.on_event(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    engine.on_tick(true);
    assert!(engine.net_warned, "first edit failure latches the gate");

    // A successful send (the ping) clears it — recovery re-arms the warning.
    engine.fire_ping();
    assert!(!engine.net_warned, "a success resets the gate");
}

#[test]
fn try_start_notifier_returns_none_on_get_me_error() {
    struct ErrTransport;
    impl Transport for ErrTransport {
        fn get(&self, _method: &str) -> Result<Value> {
            Ok(json!({ "ok": false, "description": "Unauthorized" }))
        }
        fn post(&self, _method: &str, _body: Value) -> Result<Value> {
            Ok(json!({ "ok": true, "result": {} }))
        }
    }
    let client = BotClient::new(ErrTransport);
    let queue = Arc::new(EventQueue::new());
    let handle = try_start_notifier(client, 1, RunState::new("t", 0), queue);
    assert!(handle.is_none());
}

/// ADR-0059 §2: a `waiting` agent state is a push naming what it asks;
/// `working` and `done` fold into the card and buzz nothing.
#[test]
fn worker_pushes_on_a_waiting_agent_state_only() {
    let transport = RecordingTransport::new();
    let calls = transport.calls.clone();
    let client = BotClient::new(transport);
    let queue = Arc::new(EventQueue::new());
    let shutdown = Arc::new(AtomicBool::new(false));

    let worker_queue = queue.clone();
    let worker_shutdown = shutdown.clone();
    let state = RunState::new("title", 1);
    let handle =
        std::thread::spawn(move || drive_worker(client, 7, state, worker_queue, worker_shutdown));

    let agent = |state: &str, detail: Option<&str>| RunEvent::AgentState {
        state: state.into(),
        since: "t".into(),
        detail: detail.map(str::to_string),
    };
    queue.push(agent("working", None));
    queue.push(agent("waiting", Some("AskUserQuestion: which port?")));
    queue.push(agent("done", None));
    queue.wake();
    wait_until(&calls, |c| {
        send_texts(c).iter().any(|t| t.contains("agent is waiting"))
    });

    shutdown.store(true, Ordering::SeqCst);
    queue.wake();
    handle.join().unwrap();

    let calls = calls.lock().unwrap();
    let texts = send_texts(&calls);
    assert!(
        texts
            .iter()
            .any(|t| t.contains("agent is waiting: AskUserQuestion: which port?")),
        "the waiting push names the ask: {texts:?}"
    );
    // initial card + the one waiting push: working/done buzz nothing.
    assert_eq!(
        texts.len(),
        2,
        "exactly one push for three states: {texts:?}"
    );
}
