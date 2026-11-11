use super::*;
use crate::events::sink::new_queue;
use crate::runstate::{RunEvent, UsageLite};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;

/// A test [`EventCtx`] with a stub emitter carrying a known `pid`.
fn test_ctx() -> EventCtx {
    EventCtx {
        source: "ralphy/o/r".to_owned(),
        runid: "01TESTRUNIDTESTRUNIDTE".to_owned(),
        emitter: json!({ "version": "0.0.0", "pid": 4242 }),
        git: json!({ "repository": "o/r", "branch": "afk/run-t" }),
    }
}

/// Read one HTTP request (request line + headers + Content-Length body) off a
/// stream, returning the raw bytes. Loops until the declared body is fully read
/// so a fragmented POST is still captured whole.
fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        // Once the headers are complete, stop as soon as the declared body is in.
        if let Some(headers_end) = find_subslice(&buf, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..headers_end]).to_lowercase();
            let content_len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let body_start = headers_end + 4;
            if buf.len() >= body_start + content_len {
                break;
            }
        }
        let n = stream.read(&mut chunk).expect("read request");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    buf
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A fake sink that returns a scripted sequence of outcomes, then a default
/// once the script is exhausted, counting every call.
struct ScriptedSink {
    script: std::sync::Mutex<std::collections::VecDeque<PostOutcome>>,
    default: PostOutcome,
    calls: std::sync::atomic::AtomicU32,
}

impl ScriptedSink {
    fn new(script: Vec<PostOutcome>, default: PostOutcome) -> Self {
        ScriptedSink {
            script: std::sync::Mutex::new(script.into()),
            default,
            calls: std::sync::atomic::AtomicU32::new(0),
        }
    }
    fn call_count(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl EventSink for ScriptedSink {
    fn post(&self, _body: &Value) -> anyhow::Result<PostOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(self.default))
    }
}

// Zero backoff so the retry tests never sleep on the real clock.
const NO_BACKOFF: Duration = Duration::ZERO;

#[test]
fn retry_delivers_on_fourth_attempt_after_three_transients() {
    // Three transients then a success: exactly 4 attempts, no drop warning.
    let sink = ScriptedSink::new(
        vec![
            PostOutcome::Transient,
            PostOutcome::Transient,
            PostOutcome::Transient,
        ],
        PostOutcome::Delivered,
    );
    let warned = AtomicBool::new(false);
    let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
    assert_eq!(attempts, 4, "1 initial + 3 retries");
    assert_eq!(sink.call_count(), 4);
    assert!(
        !warned.load(Ordering::SeqCst),
        "a delivered event must not warn"
    );
}

#[test]
fn retry_exhausts_and_drops_on_repeated_transient() {
    // Always transient: 4 attempts then drop with a warn.
    let sink = ScriptedSink::new(vec![], PostOutcome::Transient);
    let warned = AtomicBool::new(false);
    let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
    assert_eq!(attempts, 4);
    assert!(warned.load(Ordering::SeqCst), "exhaustion must warn");
}

#[test]
fn permanent_drops_after_one_attempt() {
    // A 4xx is a config error: one attempt, dropped without retry.
    let sink = ScriptedSink::new(vec![], PostOutcome::Permanent);
    let warned = AtomicBool::new(false);
    let attempts = deliver(&sink, &json!({}), &warned, NO_BACKOFF);
    assert_eq!(attempts, 1, "a 4xx is not retried");
    assert!(warned.load(Ordering::SeqCst), "a permanent drop warns");
}

#[test]
fn two_drops_produce_exactly_one_warn() {
    // The AtomicBool guard emits the warn only on the first drop of the run.
    let warned = AtomicBool::new(false);
    assert!(warn_dropped(&warned), "first drop warns");
    assert!(!warn_dropped(&warned), "second drop is silent");
    assert!(!warn_dropped(&warned), "and every later drop too");

    // End-to-end: two exhausting deliveries sharing one guard warn once.
    let sink = ScriptedSink::new(vec![], PostOutcome::Transient);
    let guard = AtomicBool::new(false);
    assert!(!guard.load(Ordering::SeqCst));
    deliver(&sink, &json!({}), &guard, NO_BACKOFF);
    let after_first = guard.load(Ordering::SeqCst);
    deliver(&sink, &json!({}), &guard, NO_BACKOFF);
    assert!(after_first, "first delivery flipped the guard");
    assert_eq!(sink.call_count(), 8, "two full 4-attempt runs");
}

#[test]
fn heartbeat_carries_phase_interval_and_token_totals() {
    // A folded state: issue 7 executing, one plan + one close of token usage.
    let mut state = RunState::new("t", 3);
    state.apply(RunEvent::IssueStarted {
        number: 7,
        title: "a".into(),
    });
    state.apply(RunEvent::Executing {
        number: 7,
        budget_min: 45,
        model: "sonnet".into(),
        effort: None,
    });
    let mut tokens = Totals::default();
    tokens.add(&UsageLite {
        input: 10,
        cache_read: 20,
        cache_creation: 5,
        output: 3,
        model: None,
    });
    tokens.add(&UsageLite {
        input: 1,
        cache_read: 2,
        cache_creation: 0,
        output: 4,
        model: None,
    });

    let v = heartbeat(&test_ctx(), &state, tokens, 412);
    assert_eq!(v["type"], "dev.ralphy.run.heartbeat");
    assert_eq!(v["data"]["phase"], "executing");
    assert_eq!(v["data"]["interval_s"], 30);
    assert_eq!(v["data"]["issue"], json!({ "number": 7, "title": "a" }));
    assert_eq!(v["data"]["elapsed_s"], 412);
    assert_eq!(v["data"]["queue_total"], 3);
    assert_eq!(v["data"]["tokens_total"]["up"], 11);
    assert_eq!(v["data"]["tokens_total"]["cr"], 22);
    assert_eq!(v["data"]["tokens_total"]["cw"], 5);
    assert_eq!(v["data"]["tokens_total"]["out"], 7);
}

#[test]
fn heartbeat_queue_done_counts_a_superseded_issue() {
    // The dry-run fix reaches the wire with no per-sink code: the fold's
    // supersede makes the plan-only issue terminal, so `queue_done` advances.
    let mut state = RunState::new("t", 2);
    state.apply(RunEvent::IssueStarted {
        number: 1,
        title: "one".into(),
    });
    state.apply(RunEvent::PlanWritten {
        number: 1,
        open_steps: 3,
        usage: UsageLite::default(),
        steps: vec![],
    });
    state.apply(RunEvent::IssueStarted {
        number: 2,
        title: "two".into(),
    });
    let v = heartbeat(&test_ctx(), &state, Totals::default(), 60);
    assert_eq!(v["data"]["queue_done"], 1);
    assert_eq!(v["data"]["phase"], "planning");
}

#[test]
fn spine_pushed_event_arrives_as_cloudevents_post() {
    // A recording server on an ephemeral port: accept one connection, read the
    // request, reply 200, and hand the raw request back over a channel.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let request = read_http_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .expect("reply");
        stream.flush().expect("flush the reply");
        tx.send(request).expect("send request");
    });

    let transport = crate::events::client::UreqEventTransport::new(
        format!("http://127.0.0.1:{port}/"),
        Some("tok".to_owned()),
    );
    let queue = new_queue();
    queue.push(RunEvent::IssueClosed {
        number: 7,
        tokens: 42,
        invocations: 0,
        usage: UsageLite {
            input: 1,
            cache_read: 2,
            cache_creation: 3,
            output: 4,
            model: Some("claude-sonnet-4".into()),
        },
    });
    // shutdown already set: the worker drains once, POSTs, and returns inline.
    let shutdown = Arc::new(AtomicBool::new(true));
    // No plan file → the plan-step poll is a silent no-op for this test.
    let engine = CloudEventsEngine {
        transport,
        ctx: test_ctx(),
        state: RunState::default(),
        tokens: Totals::default(),
        start: Instant::now(),
        last_beat: Instant::now(),
        poller: StepPoller::default(),
        warned: AtomicBool::new(false),
        plan_path: std::env::temp_dir().join("ralphy-nonexistent-plan.md"),
    };
    crate::delivery::run_delivery_worker(engine, queue, shutdown);

    let raw = rx.recv().expect("recorded request");
    server.join().expect("the server thread ended");

    // Split head from body.
    let headers_end = find_subslice(&raw, b"\r\n\r\n").expect("headers end");
    let head = String::from_utf8_lossy(&raw[..headers_end]).to_string();
    let head_lc = head.to_lowercase();
    let body = &raw[headers_end + 4..];

    // Request line + headers.
    assert!(head.starts_with("POST "), "not a POST: {head}");
    assert!(
        head_lc.contains("content-type: application/cloudevents+json"),
        "missing/other content-type: {head}"
    );
    assert!(
        head_lc.contains("authorization: bearer tok"),
        "missing bearer auth: {head}"
    );

    // The JSON envelope body.
    let v: Value = serde_json::from_slice(body).expect("json body");
    assert_eq!(v["specversion"], "1.0");
    assert_eq!(v["type"], "dev.ralphy.issue.closed");
    assert_eq!(v["source"], "ralphy/o/r");
    assert_eq!(v["subject"], "issue/7");
    assert!(
        v["runid"].as_str().is_some_and(|s| !s.is_empty()),
        "runid: {v}"
    );
    assert_eq!(v["data"]["emitter"]["pid"], 4242);
}

/// A fake sink recording every delivered envelope, shared with the test thread.
struct RecordingSink(Arc<std::sync::Mutex<Vec<Value>>>);
impl EventSink for RecordingSink {
    fn post(&self, body: &Value) -> anyhow::Result<PostOutcome> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(body.clone());
        Ok(PostOutcome::Delivered)
    }
}

/// Push `events` onto a fresh ring BEFORE the worker exists, then start the
/// worker over a recording sink and shut it down — the exact ordering both
/// no-work borders (#222) rely on. Returns every envelope delivered.
fn deliver_from_pre_start_ring(events: Vec<RunEvent>) -> Vec<Value> {
    let queue = new_queue();
    for ev in events {
        queue.push(ev);
    }
    let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
    let handle = try_start_sink(
        RecordingSink(Arc::clone(&recorded)),
        test_ctx(),
        queue,
        std::env::temp_dir().join("ralphy-nonexistent-plan.md"),
    )
    .expect("the sink worker spawns");
    handle.shutdown();
    let out = recorded
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    out
}

fn envelope_types(envelopes: &[Value]) -> Vec<String> {
    envelopes
        .iter()
        .map(|e| e["type"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// The "ring buffered and discarded" pin (#222): an empty-queue run emits its
/// whole triad BEFORE the sink worker starts, so a border that returned without
/// starting (or without draining) the worker would deliver nothing.
#[test]
fn pre_start_ring_delivers_the_no_work_triad() {
    let envelopes = deliver_from_pre_start_ring(vec![
        RunEvent::QueueBuilt {
            count: 0,
            order: vec![],
            stop_before: None,
            issues: Value::Null,
            assignee_filter: None,
            scope: Some("labels [AFK]".into()),
        },
        RunEvent::RunStarted {
            repo: "o/r".into(),
            queue_labels: vec!["AFK".into()],
            agent: "claude".into(),
            plan_agent: "claude".into(),
            branch_mode: "new".into(),
            branch: "origin/main".into(),
            deadline_hours: None,
        },
        RunEvent::RunFinished {
            outcome: "no_work".into(),
            issues_done: 0,
            issues_skipped: 0,
            issues_total: 0,
            issues_blocked: 0,
            issues_hitl: 0,
            issues: serde_json::Value::Null,
            up: 0,
            cr: 0,
            cw: 0,
            out: 0,
            duration_s: 0,
        },
    ]);
    assert_eq!(
        envelope_types(&envelopes),
        vec![
            "dev.ralphy.queue.built",
            "dev.ralphy.run.started",
            "dev.ralphy.run.finished",
        ],
        "envelopes: {envelopes:?}"
    );
    let fin = envelopes.last().expect("the run.finished envelope");
    assert_eq!(fin["data"]["outcome"], "no_work");
    assert_eq!(fin["data"]["issues_total"], 0);
}

/// The same pin for the `--if-idle` deferral border (#222).
#[test]
fn pre_start_ring_delivers_run_skipped() {
    let reason = "skipped: run in progress since 2026-07-19 10:00:00, pid 4242";
    let envelopes = deliver_from_pre_start_ring(vec![RunEvent::RunSkipped {
        reason: reason.into(),
    }]);
    assert_eq!(envelope_types(&envelopes), vec!["dev.ralphy.run.skipped"]);
    assert_eq!(envelopes[0]["data"]["reason"], reason);
}
