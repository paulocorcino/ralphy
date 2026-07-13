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
fn render_card_small_queue_one_line_per_issue() {
    let mut state = RunState::new("Repo · 2 issues", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        usage: UsageLite::default(),
    });
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "second".into(),
    });
    let card = render_card(&state, 0);
    assert!(card.contains("✅ #1 first"), "card: {card}");
    assert!(card.contains("🧠 #2 second"), "card: {card}");
    assert!(card.len() <= TELEGRAM_LIMIT);
}

#[test]
fn render_card_names_blocker_on_dependency_skip() {
    // A blocked-by skip carrying its open blocker(s) names them on the issue line
    // (`⏭️ #140 … (blocked by #139)`) so the operator knows which issue held it;
    // the counters are untouched.
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::Skipped {
        number: 140,
        kind: crate::runstate::SkipKind::BlockedBy,
        label: None,
        blockers: vec![139],
    });
    let card = render_card(&state, 0);
    assert!(card.contains("⏭️ #140  (blocked by #139)"), "card: {card}");
    // A skip with no resolved blocker adds no suffix.
    let mut bare = RunState::new("repo · 1 issues", 1);
    bare.apply(RunEvent::Skipped {
        number: 141,
        kind: crate::runstate::SkipKind::BlockedBy,
        label: None,
        blockers: vec![],
    });
    let bare_card = render_card(&bare, 0);
    assert!(!bare_card.contains("blocked by"), "card: {bare_card}");
}

#[test]
fn render_card_and_footer_surface_needs_split() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 3,
        title: "W1 bundle".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 3,
        open_steps: 0,
        usage: UsageLite::default(),
        steps: vec![],
    });
    state.apply(RunEvent::NeedsSplit { number: 3 });
    let card = render_card(&state, 0);
    assert!(card.contains("🧩 #3 W1 bundle"), "issue line: {card}");
    assert!(card.contains("· 🧩 1"), "counter: {card}");
    state.finished = true;
    let footer = render_final_push(&state);
    assert!(footer.contains("🧩 1 awaiting split"), "footer: {footer}");
    // Without a bundle, neither the counter nor the footer mention it.
    let clean = RunState::new("repo · 1 issues", 1);
    assert!(!render_card(&clean, 0).contains("🧩"));
    assert!(!render_final_push(&clean).contains("🧩"));
}

#[test]
fn footer_marks_a_run_that_processed_nothing_as_stopped() {
    // A run whose card reaches its terminal edge with zero issues finished,
    // skipped, or parked was interrupted (killed/superseded/bailed at startup) —
    // the footer must say so, not `🏁 … ✅ 0 done` which reads as a clean finish
    // (FinCal, 2026-07-13: an aborted run's finished card sat above the next run's
    // start card, reading "finished → started").
    let mut state = RunState::new("repo · 12 issues", 12);
    state.finished = true;
    let footer = render_final_push(&state);
    assert!(footer.contains("🛑"), "stopped marker: {footer}");
    assert!(
        footer.contains("stopped before any issue was processed"),
        "stopped wording: {footer}"
    );
    assert!(!footer.contains("🏁"), "no finish flag: {footer}");
    assert!(
        !footer.contains("✅ 0 done"),
        "no zero-done claim: {footer}"
    );

    // One issue done flips it back to the normal `🏁` completion footer.
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        usage: UsageLite::default(),
    });
    let done_footer = render_final_push(&state);
    assert!(done_footer.contains("🏁"), "finish flag: {done_footer}");
    assert!(
        done_footer.contains("✅ 1 done"),
        "done count: {done_footer}"
    );
}

#[test]
fn render_card_has_header_counters_and_blank_line_grouping() {
    let mut state = RunState::new("ocs-inventory · 2 issues [AFK]", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "first".into(),
    });
    let card = render_card(&state, 0);
    // Branding header with the binary version.
    assert!(card.contains("Ralphy - v"), "header missing: {card}");
    assert!(
        card.contains(env!("CARGO_PKG_VERSION")),
        "version missing: {card}"
    );
    // The counter line leads with `▶️ N`, the queue total (not `N issues`).
    assert!(card.contains("▶️ 2 · ✅ 0"), "counters: {card}");
    assert!(!card.contains("2 issues ·"), "old counter form: {card}");
    // Groups are separated by a blank line.
    assert!(card.contains("\n\n"), "blank-line grouping: {card}");
    // No footer mid-run — the issue list is the last group.
    assert!(!card.contains("🏁"), "footer must not show mid-run: {card}");
}

#[test]
fn render_card_shows_live_consolidation_line_then_footer_segment() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        usage: UsageLite::default(),
    });
    // Mid-consolidation: the live 📚 line shows, no footer yet.
    state.apply(RunEvent::KnowledgeConsolidating { notes: 4 });
    let live = render_card(&state, 0);
    assert!(
        live.contains("📚 consolidating 4 knowledge note(s)"),
        "live consolidation line: {live}"
    );
    assert!(!live.contains("🏁"), "no footer mid-run: {live}");

    // Completion + terminal: the live line is gone, the footer carries the count.
    state.apply(RunEvent::KnowledgeConsolidated { archived: 4 });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(
        !card.contains("consolidating 4"),
        "live line hidden once finished: {card}"
    );
    assert!(card.contains("📚 4 consolidated"), "footer segment: {card}");
}

#[test]
fn render_card_hides_stale_consolidating_line_on_finished_card() {
    // A failed session never emits `KnowledgeConsolidated`, so `consolidating`
    // stays set — the terminal card must still drop the stale 📚 line.
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::KnowledgeConsolidating { notes: 2 });
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(
        !card.contains("consolidating"),
        "no stale live line: {card}"
    );
    assert!(
        !card.contains("📚"),
        "no consolidated footer segment: {card}"
    );
}

#[test]
fn render_card_shows_footer_only_when_finished() {
    let mut state = RunState::new("repo · 1 issues", 1);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "a".into(),
    });
    state.apply(RunEvent::IssueClosed {
        number: 1,
        tokens: 0,
        usage: UsageLite::default(),
    });
    // During the run: no footer.
    assert!(!render_card(&state, 0).contains("🏁"), "no footer mid-run");
    // Finished: the footer appears with the done/skipped tally.
    state.finished = true;
    let card = render_card(&state, 0);
    assert!(card.contains("🏁"), "footer missing when finished: {card}");
    assert!(card.contains("run finished"), "footer head: {card}");
    assert!(card.contains("✅ 1 done"), "footer tally: {card}");
}

#[test]
fn header_face_is_stable_per_title_but_varies_across_titles() {
    // Same title → same face on every edit (so the card never re-edits just to
    // animate the face).
    assert_eq!(
        header_line(&RunState::new("ocs-inventory · 10 issues", 10)),
        header_line(&RunState::new("ocs-inventory · 10 issues", 10))
    );
    // The face is drawn from the curated pool.
    let face = crate::runstate::header_face("ocs-inventory · 10 issues");
    assert!(
        crate::runstate::HEADER_FACES.contains(&face),
        "face off-pool: {face}"
    );
}

#[test]
fn render_card_collapses_large_queue_within_limit() {
    let mut state = RunState::new("Big run", 200);
    for n in 1..=200u64 {
        state.apply(RunEvent::IssueStarted {
            number: n,
            title: format!("issue {n} with a moderately long descriptive title to pad bytes"),
        });
        if n < 200 {
            state.apply(RunEvent::IssueClosed {
                number: n,
                tokens: 0,
                usage: UsageLite::default(),
            });
        }
    }
    let card = render_card(&state, 0);
    assert!(card.len() <= TELEGRAM_LIMIT, "len {}", card.len());
    assert!(card.contains("▶️ 200"), "card: {card}");
    // Collapsed: active issue #200 and a last-finished line are shown.
    assert!(card.contains("#200"), "card: {card}");
}

#[test]
fn render_card_shows_sleep_line_with_live_countdown() {
    use crate::runstate::SleepState;
    let mut state = RunState::new("Repo", 1);
    state.sleep = Some(SleepState {
        reset: "14:30".into(),
        // 2h13m ahead of `now`.
        target_epoch: 1_700_000_000 + 2 * 3600 + 13 * 60,
    });
    let card = render_card(&state, 1_700_000_000);
    assert!(card.contains('🌙'), "card: {card}");
    assert!(card.contains("14:30"), "card: {card}");
    assert!(card.contains("resumes in ~"), "card: {card}");
    assert!(card.contains("~2h 13m"), "card: {card}");
}

#[test]
fn render_sleep_line_clamps_to_zero_when_reset_due() {
    use crate::runstate::SleepState;
    // `now` is past the target: the countdown degrades to `~0m`, not negative.
    let sleep = SleepState {
        reset: "09:00".into(),
        target_epoch: 1_700_000_000,
    };
    let line = render_sleep_line(&sleep, 1_700_000_500);
    assert!(line.contains("~0m"), "line: {line}");
    assert!(!line.contains('-'), "line should not go negative: {line}");
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
fn derive_title_covers_all_three_branches() {
    // --title wins.
    assert_eq!(
        derive_title("repo", 3, &["AFK".into()], None, Some("Override")),
        "Override"
    );
    // --only-issue: the single title.
    assert_eq!(
        derive_title("repo", 1, &[], Some("Only one"), None),
        "Only one"
    );
    // Auto-derived with labels.
    assert_eq!(
        derive_title("myrepo", 3, &["AFK".into(), "ready".into()], None, None),
        "myrepo · 3 issues [AFK, ready]"
    );
    // A blank --title falls through to the auto form.
    assert_eq!(
        derive_title("myrepo", 1, &[], None, Some("  ")),
        "myrepo · 1 issues"
    );
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
