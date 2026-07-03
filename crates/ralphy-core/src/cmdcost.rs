//! Durable command-cost knowledge for the verification-cost gate.
//!
//! The problem this solves, observed live: one slow test file in a package
//! suite script (a 170s OCR test in a 108-test suite) turned every casual
//! inner-loop "run the tests" into a ~3-minute pause, and the executor paid it
//! 5–6× per issue — roughly half the session's wall clock — even after the
//! execute charter told it not to. Prompts steer; they don't enforce.
//!
//! This module is the enforcement's memory: a small JSON scratch file at
//! `.ralphy/cmd-costs.json` recording how long each of the plan's `## Verify`
//! commands took the last time something actually ran it. Two writers feed it:
//!   - the runner's verify gate ([`record_gate_costs`]), which already runs the
//!     exact `## Verify` commands and now times them — durable, cross-issue
//!     knowledge (the suite's cost doesn't change between issues);
//!   - the Claude adapter's PostToolUse hook, which times in-session runs so
//!     even the FIRST issue of a fresh repo learns the cost after paying it
//!     once.
//!
//! One reader consumes it: the PreToolUse guard hook calls [`decide`] before
//! every Bash command. A command matching a `## Verify` line whose recorded
//! cost is expensive is denied while the plan still has real work open — with
//! a message pointing at the scoped alternative — and allowed again on the
//! plan's final step (the legitimate "one green run before done") and during
//! post-gate repairs (all steps ticked). Unknown cost always allows: the gate
//! is fail-open by construction, and the first run is the measurement.
//!
//! Everything here is best-effort scratch: the file lives under `.ralphy/`
//! (gitignored by the runner), concurrent hook processes may lose an update,
//! and any read/parse failure degrades to "no knowledge" — never to a block.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A verify command is "expensive" from this many seconds of measured runtime.
/// Below it, re-running is background noise (scoped tests, format checks);
/// above it, each repeat is a visible bite out of the session's time budget.
pub const EXPENSIVE_SECS: f64 = 60.0;

/// A `## Verify` line shorter than this never participates in matching —
/// substring containment on tiny strings (`true`, `ls`) would misfire.
const MIN_MATCH_LEN: usize = 8;

/// The on-disk state: measured costs keyed by the verbatim `## Verify` line,
/// plus in-flight start stamps written by the Pre hook for the Post hook to
/// turn into durations. `BTreeMap` for stable serialization.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct CostState {
    /// Verify line → longest observed wall-clock seconds.
    #[serde(default)]
    pub costs: BTreeMap<String, f64>,
    /// Verify line → epoch-seconds when a still-running invocation started.
    #[serde(default)]
    pub pending: BTreeMap<String, f64>,
}

/// Where the state lives for a project rooted at `project_root`.
pub fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".ralphy").join("cmd-costs.json")
}

/// Load the state, degrading to empty on any error (fail-open: no knowledge
/// means no denial).
pub fn load(project_root: &Path) -> CostState {
    let raw = match std::fs::read_to_string(state_path(project_root)) {
        Ok(raw) => raw,
        Err(_) => return CostState::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persist the state. Best-effort: a failed write only warns — cost knowledge
/// is an optimization, never worth failing a hook or a gate over.
pub fn save(project_root: &Path, state: &CostState) {
    let path = state_path(project_root);
    // `.ralphy/` is created by the runner; if it is absent this is not a
    // ralphy-managed tree and there is nothing to remember.
    if !path.parent().is_some_and(Path::exists) {
        return;
    }
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(error = %e, path = %path.display(), "could not write cmd-costs state");
            }
        }
        Err(e) => tracing::warn!(error = %e, "could not serialize cmd-costs state"),
    }
}

/// Extract the plan's `## Verify` command lines, verbatim (trimmed): the lines
/// between the `## Verify` heading and the next `## ` heading, skipping blanks,
/// code fences, and the `none` opt-out. Mirrors the format contract the plan
/// template documents; kept as raw strings (not argv) because the guard matches
/// them by containment inside the agent's shell command line.
pub fn verify_lines(plan_md: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_section = false;
    for line in plan_md.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            in_section = heading.trim().eq_ignore_ascii_case("verify");
            continue;
        }
        if !in_section || trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("none") {
            continue;
        }
        lines.push(trimmed.to_string());
    }
    lines
}

/// Count the plan's unticked `- [ ]` steps.
pub fn open_steps(plan_md: &str) -> usize {
    plan_md
        .lines()
        .filter(|l| l.trim_start().starts_with("- [ ]"))
        .count()
}

/// The verify line (if any) that `command` is an invocation of: the longest
/// verify line contained verbatim in the command string. Containment (not
/// equality) because the agent wraps the command — `cd X && pnpm run
/// test:harness 2>&1 | tail -15` still IS a `pnpm run test:harness` run.
pub fn match_verify_line<'a>(command: &str, lines: &'a [String]) -> Option<&'a str> {
    lines
        .iter()
        .filter(|l| l.len() >= MIN_MATCH_LEN && command.contains(l.as_str()))
        .max_by_key(|l| l.len())
        .map(String::as_str)
}

/// What [`decide`] concluded about one Bash command.
#[derive(Debug, Clone, PartialEq)]
pub enum CostDecision {
    /// Not a verify command, cost unknown/cheap, or the plan is on its final
    /// stretch — run it.
    Allow,
    /// A known-expensive verify command mid-plan: deny with this steering
    /// message (the agent reads it as hook feedback and adjusts).
    Deny(String),
}

/// The pure policy: deny a Bash `command` iff it invokes a `## Verify` line
/// whose measured cost is `>= EXPENSIVE_SECS` while the plan still has more
/// than one step open. One open step (or none) is the legitimate final green
/// run — and post-gate repair sessions (all steps ticked) are never blocked.
pub fn decide(command: &str, plan_md: &str, state: &CostState) -> CostDecision {
    let lines = verify_lines(plan_md);
    let Some(line) = match_verify_line(command, &lines) else {
        return CostDecision::Allow;
    };
    let Some(&cost) = state.costs.get(line) else {
        return CostDecision::Allow; // first run measures
    };
    if cost < EXPENSIVE_SECS {
        return CostDecision::Allow;
    }
    let open = open_steps(plan_md);
    if open <= 1 {
        return CostDecision::Allow; // final stretch / repair
    }
    CostDecision::Deny(format!(
        "`{line}` last measured ~{cost:.0}s, and the runner's verify gate re-runs it \
         after you finish anyway — with {open} plan steps still open, re-paying it now \
         buys nothing. Run the NARROWEST test covering the code you just touched \
         (the plan's cheap `## Verify` line, or `node --test <file>` / the scoped \
         equivalent for this ecosystem). This command unlocks on the plan's final \
         open step."
    ))
}

/// Epoch seconds now, as f64 (sub-second precision is irrelevant here).
fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Pre-hook bookkeeping for an ALLOWED Bash command: if it invokes a verify
/// line, stamp its start so the Post hook can turn it into a duration.
pub fn note_start(project_root: &Path, command: &str, plan_md: &str) {
    let lines = verify_lines(plan_md);
    let Some(line) = match_verify_line(command, &lines) else {
        return;
    };
    let mut state = load(project_root);
    state.pending.insert(line.to_string(), now_epoch());
    save(project_root, &state);
}

/// Post-hook bookkeeping: if this Bash command invokes a verify line with a
/// pending start stamp, record the elapsed wall clock as its cost (keeping the
/// longest observation — a `tail`-truncated rerun must not shrink the record).
pub fn note_finish(project_root: &Path, command: &str, plan_md: &str) {
    let lines = verify_lines(plan_md);
    let Some(line) = match_verify_line(command, &lines) else {
        return;
    };
    let mut state = load(project_root);
    let Some(t0) = state.pending.remove(line) else {
        return;
    };
    let elapsed = (now_epoch() - t0).max(0.0);
    let entry = state.costs.entry(line.to_string()).or_insert(0.0);
    if elapsed > *entry {
        *entry = elapsed;
    }
    save(project_root, &state);
}

/// Gate-side recording: fold the verify gate's measured command durations into
/// the durable costs (keyed by the argv rejoined with spaces, which for the
/// plain single-token-safe lines the template mandates is the verbatim
/// `## Verify` line the hooks match on). Called by the runner after each gate
/// run — pass or fail, the cost information is equally real.
pub fn record_gate_costs(project_root: &Path, measured: &[(Vec<String>, f64)]) {
    if measured.is_empty() {
        return;
    }
    let mut state = load(project_root);
    for (argv, secs) in measured {
        let line = argv.join(" ");
        if line.len() < MIN_MATCH_LEN {
            continue;
        }
        let entry = state.costs.entry(line).or_insert(0.0);
        if *secs > *entry {
            *entry = *secs;
        }
    }
    save(project_root, &state);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_OPEN: &str = "\
# Plan for #45

## Done when
- stuff

## Verify
node --test harness/p0-matrix/tests/scale-typ.test.mjs
pnpm run test:harness

## Steps
- [x] first step
- [ ] second step
- [ ] third step
- [ ] Run `pnpm run test:harness` — green
";

    const PLAN_FINAL: &str = "\
## Verify
pnpm run test:harness

## Steps
- [x] first step
- [x] second step
- [ ] Run `pnpm run test:harness` — green
";

    fn costs(entries: &[(&str, f64)]) -> CostState {
        CostState {
            costs: entries.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            pending: BTreeMap::new(),
        }
    }

    #[test]
    fn verify_lines_extracts_commands_skipping_fences_and_none() {
        let lines = verify_lines(PLAN_OPEN);
        assert_eq!(
            lines,
            vec![
                "node --test harness/p0-matrix/tests/scale-typ.test.mjs",
                "pnpm run test:harness"
            ]
        );
        assert!(verify_lines("## Verify\nnone\n\n## Steps\n").is_empty());
        assert!(verify_lines("## Verify\n```\ncargo test\n```\n## X\n") == vec!["cargo test"]);
    }

    #[test]
    fn open_steps_counts_unticked_only() {
        assert_eq!(open_steps(PLAN_OPEN), 3);
        assert_eq!(open_steps(PLAN_FINAL), 1);
    }

    #[test]
    fn match_prefers_longest_contained_line_and_ignores_short_ones() {
        let lines = vec!["cargo".to_string(), "cargo test -p ralphy-core".to_string()];
        // The wrapped invocation matches the long line, not the 5-char one.
        let got = match_verify_line("cd /x && cargo test -p ralphy-core 2>&1 | tail -5", &lines);
        assert_eq!(got, Some("cargo test -p ralphy-core"));
        // A command containing only the short line matches nothing.
        assert_eq!(match_verify_line("cargo build", &lines), None);
    }

    #[test]
    fn unknown_cost_allows_first_run_measures() {
        let got = decide(
            "cd /x && pnpm run test:harness | tail -3",
            PLAN_OPEN,
            &CostState::default(),
        );
        assert_eq!(got, CostDecision::Allow);
    }

    #[test]
    fn cheap_command_never_denied() {
        let state = costs(&[(
            "node --test harness/p0-matrix/tests/scale-typ.test.mjs",
            0.2,
        )]);
        let got = decide(
            "node --test harness/p0-matrix/tests/scale-typ.test.mjs",
            PLAN_OPEN,
            &state,
        );
        assert_eq!(got, CostDecision::Allow);
    }

    #[test]
    fn expensive_command_denied_mid_plan_with_steering_message() {
        let state = costs(&[("pnpm run test:harness", 167.0)]);
        let got = decide(
            "cd C:/Dev/x && pnpm run test:harness 2>&1 | tail -15",
            PLAN_OPEN,
            &state,
        );
        match got {
            CostDecision::Deny(msg) => {
                assert!(
                    msg.contains("pnpm run test:harness"),
                    "names the command: {msg}"
                );
                assert!(msg.contains("~167s"), "names the cost: {msg}");
                assert!(msg.contains("NARROWEST"), "steers to scoped tests: {msg}");
            }
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[test]
    fn expensive_command_allowed_on_final_step_and_when_all_ticked() {
        let state = costs(&[("pnpm run test:harness", 167.0)]);
        // One open step left: the legitimate final green run.
        assert_eq!(
            decide("pnpm run test:harness", PLAN_FINAL, &state),
            CostDecision::Allow
        );
        // All ticked (post-gate repair session): never blocked.
        let repaired = PLAN_FINAL.replace("- [ ]", "- [x]");
        assert_eq!(
            decide("pnpm run test:harness", &repaired, &state),
            CostDecision::Allow
        );
    }

    #[test]
    fn non_verify_command_always_allowed() {
        let state = costs(&[("pnpm run test:harness", 167.0)]);
        assert_eq!(
            decide("pnpm run lint", PLAN_OPEN, &state),
            CostDecision::Allow
        );
    }

    #[test]
    fn note_start_then_finish_records_a_duration_and_keeps_the_max() {
        let dir = std::env::temp_dir().join(format!("ralphy-cmdcost-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".ralphy")).unwrap();
        let cmd = "cd /x && pnpm run test:harness | tail -3";

        note_start(&dir, cmd, PLAN_OPEN);
        let mid = load(&dir);
        assert!(mid.pending.contains_key("pnpm run test:harness"));

        note_finish(&dir, cmd, PLAN_OPEN);
        let done = load(&dir);
        assert!(done.pending.is_empty(), "pending cleared");
        assert!(done.costs.contains_key("pnpm run test:harness"));

        // A pre-existing larger cost is never shrunk by a fast rerun.
        let mut state = load(&dir);
        state.costs.insert("pnpm run test:harness".into(), 500.0);
        save(&dir, &state);
        note_start(&dir, cmd, PLAN_OPEN);
        note_finish(&dir, cmd, PLAN_OPEN);
        assert_eq!(load(&dir).costs["pnpm run test:harness"], 500.0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_gate_costs_keys_by_rejoined_argv_and_keeps_max() {
        let dir = std::env::temp_dir().join(format!("ralphy-gatecost-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".ralphy")).unwrap();
        let argv = vec![
            "pnpm".to_string(),
            "run".to_string(),
            "test:harness".to_string(),
        ];

        record_gate_costs(&dir, &[(argv.clone(), 167.4)]);
        assert_eq!(load(&dir).costs["pnpm run test:harness"], 167.4);
        // A faster later gate never shrinks the record.
        record_gate_costs(&dir, &[(argv, 90.0)]);
        assert_eq!(load(&dir).costs["pnpm run test:harness"], 167.4);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_ralphy_dir_is_a_silent_noop() {
        let dir = std::env::temp_dir().join(format!("ralphy-nodir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap(); // no .ralphy inside
        note_start(&dir, "pnpm run test:harness", PLAN_OPEN);
        assert!(!state_path(&dir).exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
