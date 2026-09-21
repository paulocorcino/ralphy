use super::*;

const INIT: &str = r#"{"type":"system","subtype":"init","apiKeySource":"login","cwd":"C:\\Dev\\FinCal","session_id":"868f1553-01ac-4335-89c6-6c1f101d6009","model":"Auto","permissionMode":"force"}"#;

const PERMISSION_DENIED: &str = include_str!("../../fixtures/permission-denied-2026-07-20.jsonl");
const TOOL_FAILURE: &str = include_str!("../../fixtures/tool-failure-2026-07-20.jsonl");
const KILLED: &str = include_str!("../../fixtures/killed-2026-07-20.jsonl");
const KILLED_ERR: &str = include_str!("../../fixtures/killed-2026-07-20.err");
const INTERRUPTED_STREAM: &str = include_str!("../../fixtures/interrupted-2026-07-20.jsonl");
const INTERRUPTED_ERR: &str = include_str!("../../fixtures/interrupted-2026-07-20.err");
const PREFLIGHT_REJECTION: &str =
    include_str!("../../fixtures/preflight-rejection-2026-07-20.jsonl");
const PREFLIGHT_REJECTION_ERR: &str =
    include_str!("../../fixtures/preflight-rejection-2026-07-20.err");
/// The two quota refusals that blocked the live execute pass on 2026-07-21.
///
/// PROVENANCE, because it is not the same as the fixtures above: these were
/// recovered from the vendor's session store
/// (`~/.cursor/projects/<slug>/agent-transcripts/`), the run's stdout having
/// not been captured. That store serialises the conversational records
/// differently from the stream — bare `{"role","message"}` objects with no
/// `type` field — and the fold skips those either way. What both files share
/// with the stream, byte for byte, is the terminal record under test.
const USAGE_LIMIT: &str = include_str!("../../fixtures/usage-limit-2026-07-21.jsonl");
const USAGE_LIMIT_MIDTURN: &str =
    include_str!("../../fixtures/usage-limit-midturn-2026-07-21.jsonl");
/// The quota stop AS MEASURED: the vendor's bare `ActionRequiredError:` stderr
/// prose in the MERGED log, with NO terminal record at all. Captured live on
/// 2026-07-22 during the #251 capstone — the `cursor.log` of a plan run whose
/// child hit the Free-tier limit, byte-identical to the direct `agent -p` stderr.
/// The record-shape fixtures above were D13's pre-implementation guess; THIS is
/// the shape the run found, and the fold must read it the same way.
const USAGE_LIMIT_STDERR: &str = include_str!("../../fixtures/usage-limit-stderr-2026-07-22.log");

fn envelope(subtype: &str, is_error: bool, result: &str) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": subtype,
        "is_error": is_error,
        "duration_ms": 25253,
        "result": result,
        "session_id": "868f1553-01ac-4335-89c6-6c1f101d6009",
        "usage": {"inputTokens": 19264, "outputTokens": 1303,
                  "cacheReadTokens": 5248, "cacheWriteTokens": 0}
    })
    .to_string()
}

#[test]
fn clean_success_carries_the_sentinel_as_the_last_line_of_result() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("success", false, "all green\nRALPHY_DONE_EXIT")
    );
    let fold = fold_cursor_stream(&stdout);
    assert!(fold.saw_envelope);
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error);
    assert!(
        fold.final_text.ends_with("RALPHY_DONE_EXIT"),
        "{:?}",
        fold.final_text
    );
    assert_eq!(
        fold.session_id.as_deref(),
        Some("868f1553-01ac-4335-89c6-6c1f101d6009")
    );
    assert_eq!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );
}

/// D6/D3: the permission-denied fixture's envelope is a genuine `subtype:
/// "success"` stream whose text carries the spike's decoy token, not Ralphy's
/// `DONE_SENTINEL` — so committing something is not the same as buying a green
/// close.
#[test]
fn a_permission_denied_run_is_green_and_names_the_blocked_command() {
    let fold = fold_cursor_stream(PERMISSION_DENIED);
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error);
    assert_eq!(
        fold.denied_tool_calls,
        vec!["git status --short".to_string()]
    );
    let note = fold
        .degraded_note()
        .expect("a blocked command must be visible");
    assert!(note.contains("git status --short"), "{note}");
}

/// Companion to the pin above, over the SAME fixture: a real success envelope
/// whose text never carries `DONE_SENTINEL` must not classify `Done`, even with
/// `committed = true` — commits alone never buy a green close.
#[test]
fn an_envelope_without_the_sentinel_is_not_done() {
    let fold = fold_cursor_stream(PERMISSION_DENIED);
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done,
        "commits alone never buy a green close"
    );
}

/// D3 rule 1: the discriminator is inside the tool record, and the run reports
/// success regardless. The outcome must not read it — proved by clearing the
/// tool-call vectors and reclassifying to the SAME `Outcome`.
#[test]
fn a_failed_tool_call_does_not_change_the_outcome() {
    let mut fold = fold_cursor_stream(TOOL_FAILURE);
    assert!(
        fold.failed_tool_calls[0].contains("exit 42"),
        "{:?}",
        fold.failed_tool_calls
    );
    assert_eq!(fold.subtype.as_deref(), Some("success"));
    assert!(!fold.is_error, "a failed tool call is not a failed run");
    assert!(
        fold.degraded_note().is_some(),
        "but it IS surfaced as degraded"
    );
    let before = classify_cursor_outcome(&fold, true, false, true, Some(0));
    fold.failed_tool_calls.clear();
    fold.denied_tool_calls.clear();
    let after = classify_cursor_outcome(&fold, true, false, true, Some(0));
    assert_eq!(
        before, after,
        "the outcome never reads the tool-call vectors"
    );
}

/// D3 rule 3: partial records, no envelope, and an EMPTY stderr — the one case
/// where stderr says nothing at all, so an adapter classifying on stderr alone
/// sees a silent success. Here the missing envelope is what fails it.
#[test]
fn a_truncated_stream_has_records_but_no_envelope() {
    let fold = fold_cursor_stream(KILLED);
    assert!(!fold.saw_envelope, "the run died before the envelope");
    assert!(
        !fold.saw_no_records(),
        "records DID arrive — not a preflight rejection"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Stuck
    );
    assert!(KILLED_ERR.is_empty(), "the empty-stderr half of D3 rule 3");
}

/// D3 rule 2: zero records + exit 1. Distinguishable from a truncated run (the
/// killed fixture) by the record count, which is why the fold tracks "saw
/// anything at all".
#[test]
fn a_preflight_rejection_has_zero_records() {
    let fold = fold_cursor_stream(PREFLIGHT_REJECTION);
    assert!(!fold.saw_envelope);
    assert!(fold.saw_no_records(), "no record of any kind arrived");
    assert!(
        !fold_cursor_stream(KILLED).saw_no_records(),
        "the killed fixture DID see records — that's the discriminator"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Stuck
    );
    assert!(
        PREFLIGHT_REJECTION_ERR.contains("Workspace directory does not exist"),
        "{PREFLIGHT_REJECTION_ERR}"
    );
}

/// Never reproduced, therefore handled defensively (D3): an unknown `subtype`
/// is NOT success, even alongside a sentinel and a clean exit.
#[test]
fn an_unknown_subtype_is_not_success() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("weird", false, "all green\nRALPHY_DONE_EXIT")
    );
    let fold = fold_cursor_stream(&stdout);
    assert_eq!(fold.subtype.as_deref(), Some("weird"));
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );

    // `is_error: true` — the other never-reproduced shape — fails the same way.
    let errored = format!(
        "{INIT}\n{}\n",
        envelope("success", true, "all green\nRALPHY_DONE_EXIT")
    );
    assert_ne!(
        classify_cursor_outcome(&fold_cursor_stream(&errored), true, false, true, Some(0)),
        Outcome::Done
    );
}

/// A vendor refusal that names its own cause must not arrive as a mute stop.
/// Measured: the account's allowance ran out and the vendor said so, in a
/// well-formed terminal record — not in the `ActionRequiredError` stderr prose
/// ADR-0042 anticipated. Reading `status` is the whole difference between
/// "Stuck, no idea" and the vendor's own sentence in the run log.
#[test]
fn a_turn_ended_in_error_carries_the_vendors_reason() {
    let fold = fold_cursor_stream(USAGE_LIMIT);
    assert!(fold.is_error, "status: error is an error");
    assert!(
        fold.vendor_error
            .as_deref()
            .is_some_and(|m| m.contains("usage limit")),
        "{:?}",
        fold.vendor_error
    );
    assert!(
        !fold.saw_no_records(),
        "the turn ended on a record — this is a refusal, not a preflight rejection"
    );
    assert_ne!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Done
    );
}

/// The same refusal arriving mid-turn, after the agent had already merged
/// `origin/main` through its shell tool. Nothing about the tool work makes the
/// stop any less explicit, and no `result` envelope ever comes.
#[test]
fn a_midturn_refusal_reads_the_same_as_a_bare_one() {
    let fold = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    assert!(!fold.saw_envelope, "the stream ends on `turn_ended`");
    assert_eq!(
        fold.vendor_error,
        fold_cursor_stream(USAGE_LIMIT).vendor_error,
        "one refusal, one sentence, wherever in the turn it lands"
    );
    // Committed work does not buy a green close for a run the vendor refused
    // to finish: the sentinel never arrived.
    assert_ne!(
        classify_cursor_outcome(&fold, false, false, true, Some(1)),
        Outcome::Done
    );
}

/// #266: a quota stop classifies as `Limit(None)` — no reset hint is ever
/// published, so ADR-0030's synthetic cadence schedules the resumption.
#[test]
fn a_quota_stop_is_a_limit_with_no_reset_hint() {
    let fold = fold_cursor_stream(USAGE_LIMIT);
    assert!(
        cursor_limit_note(&fold)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&fold)
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Limit(None)
    );

    let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    assert!(
        cursor_limit_note(&midturn)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&midturn)
    );
    assert_eq!(
        classify_cursor_outcome(&midturn, false, false, true, Some(1)),
        Outcome::Limit(None)
    );
}

/// #266: the note names the partial work when the call already committed,
/// and stays silent about it when nothing landed.
#[test]
fn a_midturn_quota_stop_names_the_partial_work() {
    let midturn = fold_cursor_stream(USAGE_LIMIT_MIDTURN);
    let note = limit_stop_note(&midturn, Some("abc1234..def5678"))
        .expect("a quota stop must produce a note");
    assert!(note.contains("abc1234..def5678"), "{note}");
    assert!(note.contains("kept on the branch"), "{note}");

    let bare = fold_cursor_stream(USAGE_LIMIT);
    let note = limit_stop_note(&bare, None).expect("a quota stop must produce a note");
    assert!(note.contains("usage limit"), "{note}");
    assert!(!note.contains("kept on the branch"), "{note}");
}

/// #251: the quota stop AS IT ACTUALLY ARRIVES — a bare `ActionRequiredError:`
/// stderr line, no `turn_ended`, no envelope. Before the capstone this shape
/// folded to nothing, so the plan path reported "produced no plan" and execute
/// reported `Stuck`; both buried the vendor's own sentence. It must read as
/// `Limit(None)`, and carry the SAME sentence as the record shape.
#[test]
fn the_bare_stderr_quota_shape_is_a_limit() {
    let fold = fold_cursor_stream(USAGE_LIMIT_STDERR);
    assert!(
        !fold.saw_turn_end && !fold.saw_envelope,
        "the measured shape carries NO terminal record"
    );
    assert!(
        fold.vendor_error
            .as_deref()
            .is_some_and(|m| m.contains("usage limit")),
        "the bare stderr line must carry the vendor's sentence: {:?}",
        fold.vendor_error
    );
    assert!(
        cursor_limit_note(&fold)
            .as_deref()
            .is_some_and(|m| m.contains("You've hit your usage limit")),
        "{:?}",
        cursor_limit_note(&fold)
    );
    assert_eq!(
        fold.vendor_error,
        fold_cursor_stream(USAGE_LIMIT).vendor_error,
        "one refusal, one sentence — bare-stderr and record shapes fold alike"
    );
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(1)),
        Outcome::Limit(None)
    );
}

/// #251: the bare-line capture is gated to the LIMIT classes. A bare
/// `ActionRequiredError:` that is NOT a quota — an entitlement refusal, which
/// `model_refusal_stop` owns — must not fold to a `vendor_error`, or it would
/// masquerade as a quota stop and schedule a pointless wait.
#[test]
fn a_bare_non_limit_action_required_is_not_a_limit() {
    let stream = format!("{INIT}\nActionRequiredError: Named models unavailable on your plan\n");
    let fold = fold_cursor_stream(&stream);
    assert_eq!(fold.vendor_error, None, "not a quota class → not captured");
    assert_eq!(cursor_limit_note(&fold), None);
}

/// #266: `vendor_error` is the only carrier — a working run whose transcript
/// merely QUOTES the sentence must not classify as a limit.
#[test]
fn a_quoted_quota_sentence_in_a_green_transcript_is_not_a_limit() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope(
            "success",
            false,
            "You've hit your usage limit\nRALPHY_DONE_EXIT"
        )
    );
    let fold = fold_cursor_stream(&stdout);
    assert_eq!(cursor_limit_note(&fold), None);
    assert_eq!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done
    );
}

/// #266: a quota stop is distinct from #245's entitlement refusal and from
/// Ralphy's own budget/idle-watchdog stop.
#[test]
fn a_quota_stop_is_not_an_entitlement_refusal_nor_a_watchdog_stop() {
    const ENTITLEMENT: &str = include_str!("../../fixtures/model-entitlement-2026-07-21.err");
    assert_eq!(cursor_limit_note(&fold_cursor_stream(ENTITLEMENT)), None);

    let fold = fold_cursor_stream(INTERRUPTED_STREAM);
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, Some(130)),
        Outcome::Timeout
    );
}

/// #266: the ADR closes D13 — pin that the rewritten section documents the
/// carrier and drops the "pending" marker. Phrases are kept short so they
/// cannot straddle the ADR's ~78-col hard wrap (`.ralphy/knowledge/issue-264.md`).
#[test]
fn the_limit_stance_is_documented() {
    let adr = include_str!("../../../../docs/adr/0042-cursor-adapter.md");
    assert!(
        !adr.contains("Limits: pending"),
        "D13 must no longer read pending"
    );
    assert!(
        adr.contains("turn_ended"),
        "D13 must name the measured carrier"
    );
    assert!(
        adr.contains("Limit(None)"),
        "D13 must name the classified outcome"
    );
}

/// A `turn_ended` that says `success` is not an error, and — since the ladder
/// keys success off the `result` envelope — it does not manufacture one either.
#[test]
fn a_successful_turn_end_is_not_an_error() {
    let stdout = r#"{"type":"turn_ended","status":"success"}"#;
    let fold = fold_cursor_stream(stdout);
    assert!(!fold.is_error);
    assert_eq!(fold.vendor_error, None);
    assert!(!fold.saw_envelope);
    assert_ne!(
        classify_cursor_outcome(&fold, true, false, true, Some(0)),
        Outcome::Done,
        "a turn end without the envelope is still a truncated stream"
    );
}

/// ADR-0038: exit 130 is what Ralphy's own budget and idle watchdogs produce.
/// Reporting that as `Stuck` would blame the agent for a stop Ralphy chose.
#[test]
fn an_interrupt_is_reported_as_interrupted() {
    let fold = fold_cursor_stream(INTERRUPTED_STREAM);
    let outcome = classify_cursor_outcome(&fold, false, false, false, Some(130));
    assert_eq!(
        outcome,
        Outcome::Timeout,
        "`Aborting operation...` is a stop, not a crash"
    );
    assert_eq!(
        INTERRUPTED_ERR.trim(),
        "Aborting operation...",
        "{INTERRUPTED_ERR:?}"
    );
    // A hard kill (no exit code at all, empty stderr) stays Stuck.
    assert_eq!(
        classify_cursor_outcome(&fold, false, false, false, None),
        Outcome::Stuck
    );
}

#[test]
fn a_clean_run_has_no_degraded_note() {
    let stdout = format!(
        "{INIT}\n{}\n",
        envelope("success", false, "RALPHY_DONE_EXIT")
    );
    assert!(fold_cursor_stream(&stdout).degraded_note().is_none());
}

/// The WIRING half of D6, and the invariant the whole slice exists for: no test
/// here spawns a real child, so deleting the gate call would keep the suite
/// green and let a run spawn before the opt-out is written. Pin the call AND its
/// position — a gate that runs after the spawn has already uploaded the
/// repository. Fragments are assembled with `concat!` so the assertion cannot
/// match itself.
#[test]
fn the_gate_runs_before_any_child_is_spawned() {
    let src = include_str!("../outcome.rs");
    let gate = concat!("indexing_gate(", "work_dir, self.allow_indexing)?;");
    let seed = concat!("seed_cursor_config", "_dir(");
    let spawn = concat!("HeadlessCall::", "new(cmd,");
    let at_gate = src
        .find(gate)
        .expect("run_cursor must call the indexing gate");
    let at_seed = src.find(seed).expect("run_cursor must seed the config dir");
    let at_spawn = src.find(spawn).expect("the HeadlessCall site moved");
    assert!(
        at_gate < at_spawn,
        "D6 must write the opt-out BEFORE the child is spawned, not after"
    );
    assert!(
        at_seed < at_spawn,
        "D17's isolation must be seeded BEFORE the child is spawned"
    );
    assert_eq!(
        src.matches(spawn).count(),
        1,
        "this is the crate's single HeadlessCall site (ADR-0040 Tier 1)"
    );
}

/// The pin above counts spawns in ONE file, which is structurally blind to a
/// child spawned from another module — and one exists (`auth::probe_cursor_login`).
/// So enumerate every spawn site in the crate and assert each one is accounted
/// for: either it is gated (the run path) or it runs in a throwaway cwd and
/// config dir, where D6 has nothing to refuse and D17 nothing to protect.
/// Recursive, so an ADR-0022 `foo.rs` + `foo/` split cannot silently drop a file.
#[test]
fn every_spawn_site_in_the_crate_is_gated_or_neutralized() {
    fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.file_stem().and_then(|s| s.to_str()) == Some("tests") {
                // A sibling test module (ADR-0022 §3) carries no `#[cfg(test)]`
                // marker of its own; it is test code, not production.
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let body = std::fs::read_to_string(&path).expect("read source");
                let production = body.split("#[cfg(test)]").next().unwrap_or("").to_string();
                out.push((path.display().to_string(), production));
            }
        }
    }
    let mut files = Vec::new();
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut files,
    );

    let ctor = concat!("Command::", "new(");
    let spawners = [
        concat!("HeadlessCall::", "new("),
        concat!("run_", "headless("),
        // The one-shots spawn through the shared harness, not `HeadlessCall`
        // directly — without these the whole of `tasks.rs` is invisible here.
        // `run_json_session` is the harness' third public entrypoint (the one
        // the Claude adapter uses); it is listed so a future file reaching for
        // it cannot slip past this scan.
        concat!("run_init_", "session("),
        concat!("run_text_", "session("),
        concat!("run_json_", "session("),
    ];
    let mut sites: Vec<String> = Vec::new();
    for (name, body) in &files {
        if body.contains(ctor) || spawners.iter().any(|s| body.contains(s)) {
            sites.push(name.clone());
        }
    }
    sites.sort();
    let short: Vec<&str> = sites
        .iter()
        .map(|s| s.rsplit(['\\', '/']).next().unwrap_or(s))
        .collect();
    assert_eq!(
        short,
        vec!["auth.rs", "command.rs", "outcome.rs", "tasks.rs"],
        "a NEW child-spawning file appeared — decide its D6/D17/D18 stance and \
             extend this test; a spawn that skips them is the failure this slice exists to prevent"
    );

    // D6/D17 must cover EVERY one-shot spawn — stated as a RELATION, not a
    // literal count: a legitimate fifth verb must not red this, and a preflight
    // moved BELOW its spawn must. Pairs each spawn with the preflight that
    // precedes it, the same positional shape as the run path's pin above.
    let tasks = &files
        .iter()
        .find(|(n, _)| n.ends_with("tasks.rs"))
        .expect("tasks.rs")
        .1;
    let preflight = concat!("one_shot_", "preflight(");
    // Skip the fn's own definition; what remains are the call sites.
    let calls: Vec<usize> = tasks
        .match_indices(preflight)
        .map(|(i, _)| i)
        .skip(1)
        .collect();
    let mut spawns: Vec<usize> = spawners
        .iter()
        .flat_map(|s| tasks.match_indices(s).map(|(i, _)| i))
        .collect();
    spawns.sort_unstable();
    assert!(!spawns.is_empty(), "tasks.rs must hold the one-shot spawns");
    assert_eq!(
        calls.len(),
        spawns.len(),
        "every one-shot spawn needs its own gate+seed call"
    );
    for (call, spawn) in calls.iter().zip(&spawns) {
        assert!(
            call < spawn,
            "a one-shot spawns at byte {spawn} before its preflight at {call}"
        );
    }

    // `command.rs` only BUILDS the command; the run path's gate is pinned above.
    let auth = &files
        .iter()
        .find(|(n, _)| n.ends_with("auth.rs"))
        .expect("auth.rs")
        .1;
    assert!(
        auth.contains(concat!("current_", "dir(scratch.path())")),
        "the login probe must run OUTSIDE the operator's repository (D6)"
    );
    for key in ["CURSOR_CONFIG_DIR", "CURSOR_AGENT_DISABLE_DEBUG_LOG"] {
        assert!(
            auth.contains(key),
            "the login probe must set {key} like every other invocation (D17/D18)"
        );
    }
}

/// `shellToolCall.result.success` carries no file-change data at all, so a
/// progress number read from the stream would report zero for shell work. The
/// production half must never name those fields.
#[test]
fn no_progress_read_from_the_stream() {
    let production = include_str!("../outcome.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    for banned in ["linesAdded", "linesRemoved", "diffString"] {
        assert!(
            !production.contains(banned),
            "progress comes from the HEAD-diff, never the stream; found {banned}"
        );
    }
}

/// Every committed fixture must be read by a test via `include_str!`, not
/// re-inlined as a literal record — the fixture rule this whole slice exists
/// to enforce. Recursive over `src/` so a future ADR-0022 split cannot hide a
/// fixture from the check.
#[test]
fn every_fixture_is_read_by_a_test() {
    fn sources(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("readable src dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(std::fs::read_to_string(&path).expect("read source"));
            }
        }
    }
    let mut sources_text = Vec::new();
    sources(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")),
        &mut sources_text,
    );

    let fixtures_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"));
    for entry in std::fs::read_dir(fixtures_dir).expect("readable fixtures dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("fixture file name");
        // Read from `src/` or from a sibling test module one level down, so the
        // needle is the file name and the macro, not the depth.
        let tail = format!("fixtures/{name}\")");
        let read = sources_text.iter().any(|src| {
            src.lines()
                .any(|l| l.contains(concat!("include_str!(", "\"")) && l.contains(&tail))
        });
        assert!(
            read,
            "fixture {name} is committed but no test reads it via include_str!"
        );
    }

    let result_literal = concat!("\"type\"", ": \"result\"");
    // The tests are this file (ADR-0022 §3), so the count is over it directly.
    let test_half = include_str!("tests.rs");
    assert!(
        test_half.matches(result_literal).count() <= 2,
        "a new record shape must be captured as a fixture, not inlined — \
             the two never-reproduced synthetic tests are the only exception"
    );
}
