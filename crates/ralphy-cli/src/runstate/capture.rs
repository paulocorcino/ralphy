//! Test-only capture harness for the `tracing` event vocabulary (ADR-0039 §2).
//!
//! [`capture_events`] runs a closure with every `tracing` event emitted on the
//! calling thread recorded as a [`Captured`] — the `(level, target, message,
//! fields)` triple, with `fields` built by the SAME [`EventFields`] `Visit` impl
//! the production decoder consumes. That shared extractor is the point: a pin
//! written here fails when an emitter drifts away from what
//! [`super::event_to_runevent`] reads, not merely when a string changes.
//!
//! This module is permanent infrastructure — the ADR-0039 §2 round-trip gate is
//! built on `capture_events` in Fase 1 — so it returns the unfiltered
//! `Vec<Captured>` rather than an asserted view.
//!
//! Nothing here is compiled into the shipped binary (`#[cfg(test)]` at the
//! `mod` declaration in `runstate.rs`); the crate's public surface is unchanged.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::Level;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use super::EventFields;

/// One captured `tracing` event: the metadata triple plus the decoder's own
/// field extraction.
pub(crate) struct Captured {
    pub level: Level,
    pub target: String,
    pub message: String,
    pub fields: EventFields,
}

type Sink = Arc<Mutex<Vec<Captured>>>;

std::thread_local! {
    /// The active sink for THIS thread, if any. A `#[test]` runs on its own
    /// thread, so a capturing test never sees a sibling test's events.
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

/// Routes every event to the calling thread's [`SINK`] (a no-op when none is
/// set).
struct CaptureLayer;

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        SINK.with(|s| {
            let Some(sink) = s.borrow().as_ref().cloned() else {
                return;
            };
            // The `Visit` impl never sets `level`; the decoder's "level wins"
            // short-circuit reads `fields.level`, so seed it from the metadata
            // BEFORE recording (a visitor could never supply it).
            let mut fields = EventFields {
                level: *event.metadata().level(),
                ..Default::default()
            };
            event.record(&mut fields);
            sink.lock().unwrap().push(Captured {
                level: fields.level,
                target: event.metadata().target().to_string(),
                message: fields.message.clone(),
                fields,
            });
        });
    }
}

/// Install [`CaptureLayer`] as the process-global default exactly once.
///
/// Global, not `with_default`: a callsite first registered on another thread
/// caches its interest as *disabled* under a thread-local dispatcher, and the
/// event then silently never arrives (the trap already paid for in
/// `crates/ralphy-core/tests/queue.rs`). A default set elsewhere is a harmless
/// no-op — the capture simply records nothing.
fn install() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(CaptureLayer),
        );
    });
}

/// Run `f` with this thread's `tracing` events captured, returning its value and
/// the events in emission order. A panicking `f` unwinds without clearing the
/// sink, which fails the test anyway.
pub(crate) fn capture_events<T>(f: impl FnOnce() -> T) -> (T, Vec<Captured>) {
    install();
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    SINK.with(|s| *s.borrow_mut() = Some(sink.clone()));
    let out = f();
    SINK.with(|s| *s.borrow_mut() = None);
    let events = std::mem::take(&mut *sink.lock().unwrap());
    (out, events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_layer_records_level_target_and_message() {
        let ((), events) = capture_events(|| tracing::warn!(count = 3u64, "smoke"));
        assert_eq!(events.len(), 1, "exactly one event captured");
        assert_eq!(events[0].message, "smoke");
        assert_eq!(events[0].level, Level::WARN);
        assert_eq!(
            events[0].fields.level,
            Level::WARN,
            "level seeded on fields"
        );
        assert_eq!(events[0].fields.count, Some(3));
        // The bin crate's own module path — `ralphy`, not `ralphy_cli`.
        assert_eq!(events[0].target, "ralphy::runstate::capture::tests");
    }

    #[test]
    fn capture_is_scoped_to_the_closure() {
        let ((), events) = capture_events(|| tracing::info!("inside"));
        tracing::info!("outside");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message, "inside");
    }

    /// An `EventFields` at INFO carrying nothing but the caller's tweaks — the
    /// decoder input for a message-only pin.
    fn info_fields(tweak: impl FnOnce(&mut EventFields)) -> EventFields {
        let mut f = EventFields::default();
        tweak(&mut f);
        f
    }

    #[test]
    fn run_finished_triple_is_pinned() {
        use ralphy_core::{IssueResult, Outcome, QueueReport, Usage};

        let report = QueueReport {
            branch: "ralphy/run".into(),
            orig_branch: "main".into(),
            worked: vec![IssueResult {
                number: 7,
                outcome: Some(Outcome::Done),
                closed: true,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
            }],
            stop: None,
            commits: 1,
            undo_tag: None,
            oneline: Vec::new(),
            run_usage: Usage {
                input: 10,
                cache_read: 20,
                cache_creation: 30,
                output: 40,
                model: None,
            },
            run_usage_by_model: Default::default(),
        };

        let ((), events) = capture_events(|| {
            crate::run::report::emit_run_finished(&report, 1, std::time::Instant::now())
        });
        let ev = events
            .iter()
            .find(|e| e.message == "run finished")
            .expect("a `run finished` event");

        assert_eq!(ev.level, Level::INFO);
        assert_eq!(ev.target, "ralphy::run::report");
        let f = &ev.fields;
        assert_eq!(f.outcome.as_deref(), Some("completed"));
        assert_eq!(f.issues_done, Some(1));
        assert_eq!(f.issues_skipped, Some(0));
        assert_eq!(f.issues_total, Some(1));
        assert_eq!(
            (f.up, f.cr, f.cw, f.out),
            (Some(10), Some(20), Some(30), Some(40))
        );
        assert!(f.duration_s.is_some(), "duration_s present");

        // The decoder must read the same triple back into the typed event.
        match super::super::event_to_runevent(&ev.target, &ev.message, f) {
            Some(super::super::RunEvent::RunFinished {
                outcome,
                issues_done,
                issues_skipped,
                issues_total,
                up,
                cr,
                cw,
                out,
                duration_s: _,
            }) => {
                assert_eq!(outcome, "completed");
                assert_eq!((issues_done, issues_skipped, issues_total), (1, 0, 1));
                assert_eq!((up, cr, cw, out), (10, 20, 30, 40));
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    #[test]
    fn shared_vocabulary_constants_are_pinned() {
        use super::super::{event_to_runevent, RunEvent};
        use ralphy_adapter_support::{API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG};

        assert_eq!(API_DEGRADED_MSG, "api degraded — child retrying");
        assert_eq!(API_RECOVERED_MSG, "api recovered — child resuming");
        assert_eq!(
            IDLE_REAPED_MSG,
            "idle watchdog — no progress, reaping the child"
        );

        assert!(matches!(
            event_to_runevent("t", API_DEGRADED_MSG, &info_fields(|_| {})),
            Some(RunEvent::ApiDegraded)
        ));
        assert!(matches!(
            event_to_runevent("t", API_RECOVERED_MSG, &info_fields(|_| {})),
            Some(RunEvent::ApiRecovered)
        ));
        assert!(matches!(
            event_to_runevent(
                "t",
                IDLE_REAPED_MSG,
                &info_fields(|f| f.idle_minutes = Some(7))
            ),
            Some(RunEvent::IdleReaped { idle_minutes: 7 })
        ));
    }

    /// The consumed messages whose emitter cannot be driven from a unit test (an
    /// adapter that spawns a vendor CLI, `run started` inline in `run_cmd`,
    /// `queue built` needing `gh`): `(message, repo-relative emitter file, source
    /// fragments)`.
    ///
    /// Each row asserts on SHORT fragments so `cargo fmt` rewrapping an `info!`
    /// cannot red it — but a changed message, a dropped field, or a flipped
    /// `%`-vs-`?` sigil does. That sigil split is exactly the drift class
    /// ADR-0039 §2 names: `%model` reaches the decoder as `x`, `?model` as
    /// `Some("x")`.
    ///
    /// Deliberately absent: `crates/ralphy-agent-claude/src/interactive.rs`'s
    /// `api degraded past kill — re-spawning child once against plan.md` has NO
    /// decoder arm, so it is not consumed vocabulary and this issue does not pin
    /// it.
    ///
    /// The nine `planning with …` / `executing with …` strings are pinned as they
    /// stand today; ADR-0039 Decision 3 rewrites them to `planning`/`executing` +
    /// a `cmd` field in Fase 1 **on purpose** — a red here after that lands is the
    /// intended signal, not a regression.
    const EMITTER_SITES: &[(&str, &str, &[&str])] = &[
        (
            "queue built",
            "crates/ralphy-cli/src/run.rs",
            &[
                "count = queue.len()",
                "order = %order.join(\" -> \")",
                "stop_before,",
                "issues_json = %issues_json",
                "assignee_filter = %assignee_filter",
            ],
        ),
        (
            "run started",
            "crates/ralphy-cli/src/run.rs",
            &[
                "repo = %events_slug",
                "queue_labels = %",
                "agent = args.agent.cli_name()",
                "plan_agent = plan_agent.cli_name()",
                "branch_mode = branch_mode_str",
                "base = %base_branch",
                "deadline_hours =",
            ],
        ),
        (
            "consolidating knowledge",
            "crates/ralphy-cli/src/run/report.rs",
            &["info!(count = notes.len() as u64,"],
        ),
        (
            "knowledge consolidated",
            "crates/ralphy-cli/src/run/report.rs",
            &["info!(count = archived as u64,"],
        ),
        (
            "planning with claude -p",
            "crates/ralphy-agent-claude/src/lib.rs",
            &[
                "model = self.plan_model.as_deref().unwrap_or(\"\")",
                "effort = self.plan_effort.as_deref().unwrap_or(\"medium\")",
                "staged,",
            ],
        ),
        (
            "planning with codex exec",
            "crates/ralphy-agent-codex/src/lib.rs",
            &["model = %model", "effort = DEFAULT_CODEX_EFFORT"],
        ),
        (
            "planning with opencode run",
            "crates/ralphy-agent-opencode/src/lib.rs",
            &["model = ?self.model", "variant = ?self.variant"],
        ),
        (
            "planning with kimi --print",
            "crates/ralphy-agent-kimi/src/lib.rs",
            &["model = %model"],
        ),
        (
            "executing with interactive claude over the PTY",
            "crates/ralphy-agent-claude/src/interactive.rs",
            &[
                "model = %exec_model",
                "effort = self.exec.exec_effort.as_deref().unwrap_or(\"medium\")",
                "budget_min = self.exec.max_minutes_per_issue",
            ],
        ),
        (
            "executing with headless claude -p loop",
            "crates/ralphy-agent-claude/src/headless.rs",
            &[
                "model = %exec_model",
                "effort = self.exec.exec_effort.as_deref().unwrap_or(\"medium\")",
                "budget_min = self.exec.max_minutes_per_issue",
            ],
        ),
        (
            "executing with codex exec",
            "crates/ralphy-agent-codex/src/lib.rs",
            &["model = %model", "effort,"],
        ),
        (
            "executing with opencode run",
            "crates/ralphy-agent-opencode/src/lib.rs",
            &["model = ?self.model", "variant = ?self.variant"],
        ),
        (
            "executing with kimi --print",
            "crates/ralphy-agent-kimi/src/lib.rs",
            &["model = %model"],
        ),
    ];

    /// The workspace root, reached from this crate's manifest dir without a
    /// hard-coded separator (Windows + Linux both run this suite).
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    /// The 16 core-emitted messages, each pinned by a named test in
    /// `crates/ralphy-core/tests/queue.rs` (that crate cannot see this module, so
    /// the coverage closure below restates them as literals).
    const CORE_PINNED_MESSAGES: &[&str] = &[
        "issue started",                                     // pins_green_run_vocabulary
        "plan written",                                      // pins_green_run_vocabulary
        "plan opened",                                       // pins_green_run_vocabulary
        "plan closed",                                       // pins_green_run_vocabulary
        "green — issue closed",                              // pins_green_run_vocabulary
        "non-green — stopping run",                          // pins_skip_and_stop_vocabulary
        "deadline passed — not starting issue",              // pins_skip_and_stop_vocabulary
        "stop-before label — halting run before this issue", // pins_skip_and_stop_vocabulary
        "human-return label — skipping issue",               // pins_skip_and_stop_vocabulary
        "verify gate failed — skipping issue",               // pins_skip_and_stop_vocabulary
        "blocked by open issue(s) — skipping",               // pins_blocked_and_split_vocabulary
        "blocked — waiting on human",                        // pins_blocked_and_split_vocabulary
        "bundle plan — needs split",                         // pins_blocked_and_split_vocabulary
        "usage limit — waiting for reset",                   // pins_usage_limit_vocabulary
        "reset reached — resuming",                          // pins_usage_limit_vocabulary
    ];

    /// Every message this issue pins, across both crates: the 13 `EMITTER_SITES`
    /// rows, the 3 shared constants, `run finished`, and the 15 core messages.
    ///
    /// The closure this guards: each one must be genuinely CONSUMED vocabulary
    /// (`event_to_runevent` returns `Some`), and the count must match the decoder's
    /// arm count — so a NEW arm added without a pin reds here rather than shipping
    /// unpinned.
    #[test]
    fn every_decoder_arm_has_a_pin() {
        use super::super::event_to_runevent;
        use ralphy_adapter_support::{API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG};

        let mut pinned: Vec<&str> = EMITTER_SITES.iter().map(|(m, _, _)| *m).collect();
        pinned.extend([API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG]);
        pinned.push("run finished");
        pinned.extend(CORE_PINNED_MESSAGES);

        // 32 = the arm count of `event_to_runevent` (event.rs), counting each
        // message of the two multi-message arms (4 `planning with …`, 5
        // `executing with …`) separately.
        assert_eq!(
            pinned.len(),
            32,
            "pin count drifted from the decoder's arms"
        );

        let unique: std::collections::BTreeSet<&str> = pinned.iter().copied().collect();
        assert_eq!(unique.len(), pinned.len(), "a message is pinned twice");

        for message in &pinned {
            assert!(
                event_to_runevent("t", message, &info_fields(|_| {})).is_some(),
                "`{message}` is pinned but the decoder ignores it — \
                 either the pin or the decoder arm is stale"
            );
        }
    }

    /// The source text of the `info!(…)` invocation that emits `message` — from
    /// the nearest preceding `info!(` to the message literal.
    ///
    /// Scoping to the invocation, not the whole file, is what makes a fragment pin
    /// discriminate: two emitters in one file (codex's `planning with codex exec`
    /// and `executing with codex exec`) share the `model = %model` fragment, so a
    /// file-wide `contains` would let either one satisfy both rows.
    /// Every candidate site: the message literal occurs in prose comments too
    /// (`run.rs` names `info!("queue built")` twice in doc text), so a row matches
    /// when ANY candidate carries all its fragments. A comment's candidate is an
    /// empty/short slice that carries none, so this never launders a real drift.
    fn emit_sites<'a>(src: &'a str, message: &str) -> Vec<&'a str> {
        let literal = format!("\"{message}\"");
        let mut sites = Vec::new();
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(&literal) {
            let end = from + rel;
            if let Some(start) = src[..end].rfind("info!(") {
                sites.push(&src[start..end]);
            }
            from = end + literal.len();
        }
        sites
    }

    #[test]
    fn emitter_sites_are_pinned() {
        for (message, file, fragments) in EMITTER_SITES {
            let path = repo_root().join(file);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("reading emitter {file} for `{message}`: {e}"));
            let sites = emit_sites(&src, message);
            assert!(
                !sites.is_empty(),
                "`{message}` must be an `info!` message literal in {file}"
            );
            assert!(
                sites
                    .iter()
                    .any(|site| fragments.iter().all(|f| site.contains(f))),
                "no `info!` site in {file} emits `{message}` with all of {fragments:?} — \
                 an emitter field or its %/? encoding drifted. Candidates:\n{sites:#?}"
            );
        }
    }
}
