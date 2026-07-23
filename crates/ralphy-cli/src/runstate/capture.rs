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
            // Built BEFORE the lock: nothing that could re-enter `tracing` may run
            // while the sink's Mutex is held.
            let captured = Captured {
                level: fields.level,
                target: event.metadata().target().to_string(),
                message: fields.message.clone(),
                fields,
            };
            sink.lock().unwrap().push(captured);
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

/// Restores the sink that was active before this capture, on every exit path.
///
/// Restore-previous rather than clear: nesting one `capture_events` inside
/// another must not silently disable the outer one for the rest of its closure.
/// `Drop` rather than a plain reset: under `--test-threads=1` every test shares
/// the main thread, so a panicking `f` would otherwise leave a stale sink behind
/// and poison the NEXT test.
struct SinkGuard(Option<Sink>);

impl Drop for SinkGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        SINK.with(|s| *s.borrow_mut() = previous);
    }
}

/// Run `f` with this thread's `tracing` events captured, returning its value and
/// the events in emission order.
pub(crate) fn capture_events<T>(f: impl FnOnce() -> T) -> (T, Vec<Captured>) {
    install();
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let _guard = SinkGuard(SINK.with(|s| s.borrow_mut().replace(sink.clone())));
    let out = f();
    drop(_guard);
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
        use ralphy_core::{IssueResult, Outcome, QueueReport, ResultStatus, Usage};

        let report = QueueReport {
            branch: "ralphy/run".into(),
            orig_branch: "main".into(),
            worked: vec![IssueResult {
                number: 7,
                outcome: Some(Outcome::Done),
                closed: true,
                blocked_by: Vec::new(),
                human_blockers: Vec::new(),
                status: ResultStatus::Done,
                skip: None,
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
            invocations: 0,
        };

        let summary = crate::run::summary::RunSummary::from_report(&report, 1);
        let run_usage = report.run_usage.clone();
        let ((), events) = capture_events(|| {
            crate::run::report::emit_run_finished(&summary, &run_usage, std::time::Instant::now())
        });
        let ev = events
            .iter()
            .find(|e| e.message == "run finished")
            .expect("a `run finished` event");

        assert_eq!(ev.level, Level::INFO);
        // The helper's module, not the caller's: tracing builds `Metadata` in a
        // `static` callsite, so an `emit` helper cannot forward the emitting
        // module's path (ADR-0039 §1). The decoder ignores `target`.
        assert_eq!(ev.target, "ralphy_core::emit");
        let f = &ev.fields;
        assert_eq!(f.outcome.as_deref(), Some("completed"));
        assert_eq!(f.issues_done, Some(1));
        assert_eq!(f.issues_skipped, Some(0));
        assert_eq!(f.issues_total, Some(1));
        // The fixture's single issue is green, so both new buckets are 0 — the
        // point of the pin is that the fields are EMITTED, with the run's own
        // rollup beside them.
        assert_eq!(f.issues_blocked, Some(0));
        assert_eq!(f.issues_hitl, Some(0));
        assert_eq!(
            f.issues_json.as_deref(),
            Some(r#"[{"number":7,"status":"done"}]"#)
        );
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
                issues_blocked,
                issues_hitl,
                issues,
                up,
                cr,
                cw,
                out,
                duration_s: _,
            }) => {
                assert_eq!(outcome, "completed");
                assert_eq!((issues_done, issues_skipped, issues_total), (1, 0, 1));
                assert_eq!((issues_blocked, issues_hitl), (0, 0));
                let rollup = issues.as_array().expect("the rollup decodes to an array");
                assert_eq!(rollup.len(), 1);
                assert_eq!(rollup[0]["status"], "done");
                assert_eq!((up, cr, cw, out), (10, 20, 30, 40));
            }
            other => panic!("expected RunFinished, got {other:?}"),
        }
    }

    /// The empty-queue border's own emitter (#222): it does NOT go through
    /// `emit_run_finished`/`outcome_of` (no `QueueReport` exists), so its shape is
    /// pinned separately — same target, same level, `no_work` and all-zero counts.
    #[test]
    fn no_work_triple_is_pinned() {
        let ((), events) = capture_events(|| {
            crate::run::report::emit_run_finished_no_work(std::time::Instant::now())
        });
        let ev = events
            .iter()
            .find(|e| e.message == "run finished")
            .expect("a `run finished` event");

        assert_eq!(ev.level, Level::INFO);
        assert_eq!(ev.target, "ralphy_core::emit");
        let f = &ev.fields;
        assert_eq!(f.outcome.as_deref(), Some("no_work"));
        assert_eq!(
            (f.issues_done, f.issues_skipped, f.issues_total),
            (Some(0), Some(0), Some(0))
        );
        assert_eq!((f.issues_blocked, f.issues_hitl), (Some(0), Some(0)));
        // No rollup on an empty run: the decoder reads the empty string back as
        // `Value::Null`, the envelope's legacy-fallback signal.
        assert_eq!(f.issues_json.as_deref(), Some(""));
        match super::super::event_to_runevent(&ev.target, &ev.message, f) {
            Some(super::super::RunEvent::RunFinished { issues, .. }) => {
                assert_eq!(issues, serde_json::Value::Null)
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

    /// The workspace root, reached from this crate's manifest dir without a
    /// hard-coded separator (Windows + Linux both run this suite).
    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    /// The 22 messages `ralphy_core::emit` owns a helper for AND that no other
    /// pin covers (ADR-0039 §1) — the 15 emitted by the core runner, the 5 the
    /// CLI emits through the same module, and the 2 the vendor adapters emit.
    /// The 3 shared adapter constants `emit` also owns are pinned separately
    /// (below), for 25 in total.
    ///
    /// Every one has a round-trip test in `super::super::roundtrip`; the
    /// 15 core ones additionally carry a characterization pin in
    /// `crates/ralphy-core/tests/queue.rs` (named in the trailing comment).
    ///
    /// Restated as literals, not as the `…_MSG` constants, on purpose: this list
    /// is the second witness. Naming the constants would make it agree with a
    /// renamed message by construction and prove nothing.
    const EMIT_OWNED_MESSAGES: &[&str] = &[
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
        "queue built",                                       // CLI — roundtrip_queue_built
        "run started",                                       // CLI — roundtrip_run_started
        "run finished",                                      // CLI — roundtrip_run_finished
        "run skipped",                                       // CLI — roundtrip_run_skipped
        "consolidating knowledge", // CLI — roundtrip_knowledge_consolidating
        "knowledge consolidated",  // CLI — roundtrip_knowledge_consolidated
        "planning",                // adapters — roundtrip_planning
        "executing",               // adapters — roundtrip_executing
    ];

    /// How many messages `event_to_runevent`'s `match` consumes, read off the
    /// decoder's source: every pattern line in the `match message {` block, which
    /// is one message per line (a multi-message arm formats as `"a"\n| "b" => …`)
    /// plus the `ralphy_*::…_MSG` constant patterns (the migrated arms match
    /// `ralphy_core::emit` constants, never literals — ADR-0039 §1).
    fn decoder_arm_messages(src: &str) -> usize {
        let body = src
            .split_once("match message {")
            .expect("the decoder's match")
            .1;
        let body = body.split_once("_ => None").expect("the fallthrough arm").0;
        body.lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with('"') || l.starts_with("| \"") || l.starts_with("ralphy_"))
            .count()
    }

    /// Every message pinned across both crates: the 3 shared adapter constants
    /// and the 23 `ralphy_core::emit`-owned messages — 26 in all. No
    /// source-fragment pins remain: every message now has a real emit helper, so
    /// `super::super::roundtrip` proves the encoding by execution.
    ///
    /// The closure this guards: each one must be genuinely CONSUMED vocabulary
    /// (`event_to_runevent` returns `Some`), and the count must match the decoder's
    /// arm count — so a NEW arm added without a pin reds here rather than shipping
    /// unpinned.
    #[test]
    fn every_decoder_arm_has_a_pin() {
        use super::super::event_to_runevent;
        // Named through the `ralphy_adapter_support` re-export on purpose: the
        // constants moved to `ralphy_core::emit` in Fase 1a, and this is what
        // proves the historical import path still resolves (ADR-0039 D4).
        use ralphy_adapter_support::{API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG};

        let mut pinned: Vec<&str> = EMIT_OWNED_MESSAGES.to_vec();
        pinned.extend([API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG]);

        // Counted off the decoder's OWN source, not restated as a second constant:
        // an arm added to `event_to_runevent` without a pin reds HERE.
        let decoder =
            std::fs::read_to_string(repo_root().join("crates/ralphy-cli/src/runstate/event.rs"))
                .expect("reading the decoder source");
        assert_eq!(
            pinned.len(),
            decoder_arm_messages(&decoder),
            "the decoder's consumed-message count drifted from the pinned set — \
             an `event_to_runevent` arm was added or removed without a pin"
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

    /// One row of [`ADAPTER_EMIT_SITES`]: `(repo-relative file, planning calls,
    /// executing calls, planning args, executing args)`.
    type AdapterEmitSite = (
        &'static str,
        usize,
        usize,
        &'static [&'static str],
        &'static [&'static str],
    );

    /// The nine adapter call sites of `emit::planning`/`emit::executing`, pinned
    /// per site.
    ///
    /// The round-trips in `super::super::roundtrip` prove the two HELPERS; they
    /// cannot prove that each caller passes the right arguments — the table below
    /// is the authoritative site count, against only 2 helpers. Two drifts live in
    /// exactly that gap
    /// and compile silently:
    ///
    /// * `model` and `effort` are ADJACENT `&str` parameters, so swapping them at
    ///   one site labels that adapter's phase with its effort. Fragments are
    ///   matched IN ORDER, which is what pins argument POSITION — the named
    ///   `model = …` / `effort = …` bindings the pre-Fase-1b `info!`s carried.
    /// * only the 2 claude sites pass a real `budget_min`; every other site passes `0`.
    ///   Editing a claude site down to `0` — it then reads exactly like the codex
    ///   line two files over — silently zeroes the per-issue countdown in the TUI
    ///   and the Telegram card (both files carry a "keep stable" comment saying so).
    ///
    /// The call counts additionally pin PRESENCE: `no_vocabulary_literal_outside_emit`
    /// is a negative scan, so an adapter that stops emitting altogether would make
    /// it *more* satisfied while the run's phase never advances.
    const ADAPTER_EMIT_SITES: &[AdapterEmitSite] = &[
        (
            "crates/ralphy-agent-claude/src/lib.rs",
            1,
            0,
            &[
                "\"claude -p --staged\"",
                "\"claude -p\"",
                "self.plan_model.as_deref().unwrap_or(\"\")",
                "self.plan_effort.as_deref().unwrap_or(\"\")",
            ],
            &[],
        ),
        (
            "crates/ralphy-agent-claude/src/interactive.rs",
            0,
            1,
            &[],
            &[
                "interactive claude over the PTY",
                "self.exec.max_minutes_per_issue",
                "&exec_model",
                "self.exec.exec_effort.as_deref().unwrap_or(\"\")",
            ],
        ),
        (
            "crates/ralphy-agent-claude/src/headless.rs",
            0,
            1,
            &[],
            &[
                "headless claude -p loop --max-calls",
                "self.exec.max_minutes_per_issue",
                "&exec_model",
                "self.exec.exec_effort.as_deref().unwrap_or(\"\")",
            ],
        ),
        (
            "crates/ralphy-agent-codex/src/lib.rs",
            1,
            1,
            &["\"codex exec\"", "&model", "DEFAULT_CODEX_EFFORT"],
            &["\"codex exec\"", "0", "&model", "effort"],
        ),
        (
            "crates/ralphy-agent-copilot/src/lib.rs",
            1,
            1,
            &[
                "\"copilot\"",
                "model.unwrap_or(\"\")",
                "effort.as_deref().unwrap_or(\"\")",
            ],
            &[
                "\"copilot\"",
                "0",
                "model.unwrap_or(\"\")",
                "effort.as_deref().unwrap_or(\"\")",
            ],
        ),
        (
            "crates/ralphy-agent-cursor/src/lib.rs",
            1,
            1,
            &["\"cursor\"", "model.unwrap_or(command::AUTO_MODEL)", "\"\""],
            &[
                "\"cursor\"",
                "0",
                "model.unwrap_or(command::AUTO_MODEL)",
                "\"\"",
            ],
        ),
        (
            "crates/ralphy-agent-gemini/src/lib.rs",
            1,
            1,
            &["\"gemini\"", "model.unwrap_or(DEFAULT_MODEL)", "\"\""],
            &["\"gemini\"", "0", "model.unwrap_or(DEFAULT_MODEL)", "\"\""],
        ),
        (
            "crates/ralphy-agent-kimi/src/lib.rs",
            1,
            1,
            &["\"kimi\"", "&model", "\"\""],
            &["\"kimi\"", "0", "&model", "\"\""],
        ),
        (
            "crates/ralphy-agent-opencode/src/lib.rs",
            1,
            1,
            &[
                "\"opencode run\"",
                "self.model.as_deref().unwrap_or(\"\")",
                "self.variant.as_deref().unwrap_or(\"\")",
            ],
            &[
                "\"opencode run\"",
                "0",
                "self.model.as_deref().unwrap_or(\"\")",
                "self.variant.as_deref().unwrap_or(\"\")",
            ],
        ),
    ];

    /// Every `call(`-to-matching-`)` slice in `src`, so a fragment match is scoped
    /// to ONE invocation. Paren-counted rather than `find(')')`: every one of these
    /// call sites nests parens (`as_deref().unwrap_or("")`, `format!(…)`).
    fn call_slices<'a>(src: &'a str, call: &str) -> Vec<&'a str> {
        let mut out = Vec::new();
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(call) {
            let open = from + rel + call.len();
            let mut depth = 1usize;
            let mut end = open;
            for (i, c) in src[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            out.push(&src[open..end]);
            from = open;
        }
        out
    }

    /// True when every fragment occurs in `slice` in the given ORDER — which is
    /// what turns a containment check into an argument-POSITION check.
    fn contains_in_order(slice: &str, fragments: &[&str]) -> bool {
        let mut from = 0usize;
        for f in fragments {
            match slice[from..].find(f) {
                Some(rel) => from += rel + f.len(),
                None => return false,
            }
        }
        true
    }

    #[test]
    fn adapter_emit_sites_pass_the_right_arguments() {
        for (file, n_plan, n_exec, plan_args, exec_args) in ADAPTER_EMIT_SITES {
            let src = std::fs::read_to_string(repo_root().join(file))
                .unwrap_or_else(|e| panic!("reading adapter emitter {file}: {e}"));
            for (helper, want, args) in [
                ("emit::planning(", *n_plan, *plan_args),
                ("emit::executing(", *n_exec, *exec_args),
            ] {
                let sites = call_slices(&src, helper);
                assert_eq!(
                    sites.len(),
                    want,
                    "{file} has {} `{helper}` call(s), expected {want} — an adapter \
                     that stops emitting leaves the run's phase stuck, and the \
                     negative literal scan cannot see it",
                    sites.len()
                );
                if want == 0 {
                    continue;
                }
                assert!(
                    sites.iter().any(|s| contains_in_order(s, args)),
                    "no `{helper}` call in {file} passes {args:?} in that ORDER — \
                     an argument was swapped, dropped, or rewritten. `model` and \
                     `effort` are adjacent `&str`s, so a swap compiles. Candidates:\n{sites:#?}"
                );
            }
        }
    }

    /// The sources that used to hold the vocabulary literals and must no longer:
    /// every migrated emitter, across all four crates.
    const MIGRATED_EMITTERS: &[&str] = &[
        "crates/ralphy-core/src/runner.rs",
        "crates/ralphy-core/src/runner/phases.rs",
        "crates/ralphy-core/src/runner/clock.rs",
        "crates/ralphy-cli/src/run.rs",
        "crates/ralphy-cli/src/run/report.rs",
        "crates/ralphy-adapter-support/src/headless.rs",
        "crates/ralphy-agent-claude/src/interactive.rs",
        // The adapter sources that owned the per-adapter phase strings until
        // Fase 1b collapsed them into `emit::planning`/`emit::executing`, plus
        // every adapter added since.
        "crates/ralphy-agent-claude/src/lib.rs",
        "crates/ralphy-agent-claude/src/headless.rs",
        "crates/ralphy-agent-codex/src/lib.rs",
        "crates/ralphy-agent-copilot/src/lib.rs",
        "crates/ralphy-agent-cursor/src/lib.rs",
        "crates/ralphy-agent-gemini/src/lib.rs",
        "crates/ralphy-agent-kimi/src/lib.rs",
        "crates/ralphy-agent-opencode/src/lib.rs",
        // The two files that USED to own the shared constants: they are now
        // `pub use ralphy_core::emit::…` re-exports, and a re-introduced literal
        // here would be the most natural way to undo ADR-0039 D4 by accident.
        "crates/ralphy-adapter-support/src/idle.rs",
        "crates/ralphy-adapter-support/src/degraded.rs",
    ];

    /// The machine proof of ADR-0039 §1's central claim: the vocabulary lives in
    /// exactly ONE place. A quoted message literal anywhere in a migrated emitter
    /// — an emit site, a leftover `info!`, or a prose doc comment naming the old
    /// string — is a second source that can drift, so it reds here.
    ///
    /// Covers the constants' text, not the constants: an emitter that spells the
    /// message out is precisely what this forbids, and only a literal comparison
    /// can see it.
    #[test]
    fn no_vocabulary_literal_outside_emit() {
        use ralphy_core::emit;

        let vocabulary: &[&str] = &[
            emit::ISSUE_STARTED_MSG,
            emit::PLAN_WRITTEN_MSG,
            emit::PLAN_OPENED_MSG,
            emit::PLAN_CLOSED_MSG,
            emit::ISSUE_CLOSED_MSG,
            emit::NEEDS_SPLIT_MSG,
            emit::BLOCKED_BY_OPEN_MSG,
            emit::BLOCKED_WAITING_HUMAN_MSG,
            emit::NON_GREEN_MSG,
            emit::DEADLINE_PASSED_MSG,
            emit::STOP_BEFORE_LABEL_MSG,
            emit::HUMAN_RETURN_LABEL_MSG,
            emit::VERIFY_GATE_FAILED_MSG,
            emit::USAGE_LIMIT_WAITING_MSG,
            emit::RESET_REACHED_MSG,
            emit::IDLE_REAPED_MSG,
            emit::API_DEGRADED_MSG,
            emit::API_RECOVERED_MSG,
            emit::QUEUE_BUILT_MSG,
            emit::RUN_STARTED_MSG,
            emit::RUN_FINISHED_MSG,
            emit::KNOWLEDGE_CONSOLIDATING_MSG,
            emit::KNOWLEDGE_CONSOLIDATED_MSG,
            emit::PLANNING_MSG,
            emit::EXECUTING_MSG,
        ];
        assert_eq!(vocabulary.len(), 25, "every emit-owned message is scanned");

        for file in MIGRATED_EMITTERS {
            let src = std::fs::read_to_string(repo_root().join(file))
                .unwrap_or_else(|e| panic!("reading migrated emitter {file}: {e}"));
            for message in vocabulary {
                assert!(
                    !src.contains(&format!("\"{message}\"")),
                    "{file} still spells out the vocabulary literal `{message}` — \
                     `ralphy_core::emit` owns it; call the helper (or name the \
                     `…_MSG` constant) instead"
                );
            }
        }
    }
}
