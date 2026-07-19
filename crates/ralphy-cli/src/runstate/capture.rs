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
        // The helper's module, not the caller's: tracing builds `Metadata` in a
        // `static` callsite, so an `emit` helper cannot forward the emitting
        // module's path (ADR-0039 §1). The decoder ignores `target`.
        assert_eq!(ev.target, "ralphy_core::emit");
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
    /// adapter that spawns a vendor CLI): `(message, repo-relative emitter file,
    /// source fragments)`.
    ///
    /// Only the nine per-adapter `planning with …` / `executing with …` strings
    /// remain: the four CLI rows this table used to carry (`queue built`,
    /// `run started`, `consolidating knowledge`, `knowledge consolidated`) moved
    /// to `ralphy_core::emit` in Fase 1a and are now proved by real round-trip
    /// tests (`super::super::roundtrip`) rather than by source-text fragments.
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

    /// The 20 messages `ralphy_core::emit` owns a helper for AND that no other
    /// pin covers (ADR-0039 §1) — the 15 emitted by the core runner plus the 5
    /// the CLI emits through the same module. The 3 shared adapter constants
    /// `emit` also owns are pinned separately (below), for 23 in total.
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
        "consolidating knowledge", // CLI — roundtrip_knowledge_consolidating
        "knowledge consolidated",  // CLI — roundtrip_knowledge_consolidated
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

    /// Every message pinned across both crates: the 9 remaining `EMITTER_SITES`
    /// rows (the per-adapter phase strings), the 3 shared adapter constants, and
    /// the 20 `ralphy_core::emit`-owned messages — 32 in all.
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

        let mut pinned: Vec<&str> = EMITTER_SITES.iter().map(|(m, _, _)| *m).collect();
        pinned.extend([API_DEGRADED_MSG, API_RECOVERED_MSG, IDLE_REAPED_MSG]);
        pinned.extend(EMIT_OWNED_MESSAGES);

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
            if let Some(site) = enclosing_info_call(&src[..end]) {
                sites.push(site);
            }
            from = end + literal.len();
        }
        sites
    }

    /// The `info!(`-to-here slice, but ONLY when `info!(` is the macro call the
    /// literal actually sits in.
    ///
    /// A bare `rfind("info!(")` would skip backwards over an intervening
    /// `warn!(`/`error!(` to an earlier, unrelated `info!(` — so flipping an
    /// emitter's level (the drift that makes the decoder collapse it to a generic
    /// `Notice`, `event.rs:173-179`) would leave the row green. Rejecting a
    /// candidate that spans a `;` or another `!(` keeps the slice inside one call.
    fn enclosing_info_call(before: &str) -> Option<&str> {
        let start = before.rfind("info!(")?;
        let body = &before[start + "info!(".len()..];
        (!body.contains(';') && !body.contains("!(")).then_some(&before[start..])
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
        ];
        assert_eq!(vocabulary.len(), 23, "every emit-owned message is scanned");

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
