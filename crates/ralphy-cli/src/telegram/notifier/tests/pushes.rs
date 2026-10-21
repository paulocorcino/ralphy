use super::*;

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
