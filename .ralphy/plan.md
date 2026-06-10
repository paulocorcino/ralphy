# Plan for #24: OpenCode adapter — tracer: `--agent opencode` drives a headless run end-to-end

## Feasible: yes
The work is a near-clone of the existing `ralphy-agent-codex` crate with the
divergences fully specified in `docs/adr/0005-opencode-adapter.md` (D1–D5, D8a);
every step anchors to concrete code. The live end-to-end criterion needs a real
`opencode` CLI and is review-only, but the classifier and model/variant
resolution — the substance — are unit-testable, matching the Codex adapter's
test depth.

## Execution model: opus
A new workspace crate with a line-delimited-JSON event-stream classifier (with a
HEAD-diff commit downgrade and an error-event override), cross-cutting wiring
(workspace `Cargo.toml`, `main.rs` enum + arm + new `--exec-variant` arg), a new
prompt asset, and ~15 mirrored tests — cross-cutting breadth plus a classifier
whose JSON shape needs judgment, beyond a mechanical localized change.

## Done when
- `cargo test` passes, including new tests in `ralphy-agent-opencode` that prove:
  (a) a clean exit + new commit + `RALPHY_DONE_EXIT` → `Done`, and the same
  claim with no commit → `Stuck`; (b) `RALPHY_BLOCKED_EXIT <reason>` →
  `Blocked(reason)`, a non-zero exit → `Stuck`, a JSON `error` event → `Stuck`,
  and the wall timeout → `Timeout`; (c) `build_opencode_command` omits `-m` when
  no `--exec-model` and includes `-m <model>` when set; (d) `--variant` is
  present only when the operator sets it, and `--dangerously-skip-permissions`
  and `--format json` are always present.
- `cargo build --release` succeeds (the `OpenCode` `--agent` arm compiles and
  boxes `OpenCodeAgent` as `Box<dyn Agent>`).
- `cargo fmt --check` and `cargo clippy -D warnings` are clean with no new
  warnings.
- Review-only: `ralphy run --agent opencode --only-issue <N>` plans and executes
  a trivial issue end-to-end against a live `opencode run` (needs the installed
  CLI; cannot run in CI). Review-only: the diff touches only the new crate, the
  workspace `Cargo.toml`, `crates/ralphy-cli/src/main.rs`, and the new prompt
  asset — `ralphy-core`, `ralphy-agent-claude`, and `ralphy-agent-codex` source
  is untouched.

## Acceptance ledger
- [review-only] `ralphy run --agent opencode --only-issue <N>` plans and executes a trivial issue end-to-end against a live `opencode run`. — evidence: a human runs the command against the installed `opencode 1.16.2` and confirms a plan and an executed commit on the run branch
- [verified] A clean finish with a git commit + `RALPHY_DONE_EXIT` classifies as `Done`; a `Done` claim with no commit downgrades to `Stuck`. — evidence: tests `classify_done_on_clean_exit_commit_and_sentinel` (→ `Done`) and `classify_stuck_on_no_commit` (committed=false → `Stuck`) in `crates/ralphy-agent-opencode/src/lib.rs` pass
- [verified] `RALPHY_BLOCKED_EXIT <reason>` classifies as `Blocked(reason)`; a non-zero exit / JSON `error` event classifies as `Stuck`; the per-issue timeout classifies as `Timeout`. — evidence: tests `classify_blocked_on_blocked_sentinel`, `classify_stuck_on_non_zero_exit`, `classify_stuck_on_error_event`, and `classify_timeout_wins` pass
- [verified] With no `--exec-model`, the adapter passes no `-m` and OpenCode resolves its own model; `--exec-model` overrides it. — evidence: tests `build_command_omits_model_when_none` (no `-m`) and `build_command_includes_model_when_some` (`-m <model>` present) pass
- [review-only] The core crate, `ralphy-agent-claude`, and `ralphy-agent-codex` are unchanged except for the `main.rs` `--agent opencode` arm. — evidence: a reviewer confirms `git diff` touches no source under those three crates; the full suite (24+ passing) shows no regression
- [verified] Unit tests cover the classifier and the model/variant resolution, mirroring the Codex adapter's test depth. — evidence: the new `#[cfg(test)] mod tests` (17 tests) covers `classify_opencode_outcome`, `build_opencode_command` (model + variant + always-on flags + both API keys removed), `parse_opencode_events` (text parts + error flag + tolerance), the prompt-asset assertions, and the `OpenCodeAgent: Agent` binding

## Decisions
- Decision: scope this tracer to `Done`/`Stuck`/`Blocked`/`Timeout` only — no
  `Limit`, auth-error, or skills-materialization handling. Why: the issue's
  acceptance criteria name only these four outcomes; usage-limit (D9), auth (D6),
  and `skills.paths` (D7) are explicitly deferred-until-live in the ADR and are
  not asked for here, so adding them would invent scope.
- Decision: still `env_remove` both `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` on
  the child `Command` (ADR-0005 D6). Why: OpenCode auto-detects either key and
  silently switches the run to metered API billing — a cheap, defensive part of
  cloning `run_codex`'s command builder that a reviewer would otherwise flag.
- Decision: classify from a tolerant line-delimited-JSON parse — concatenate
  `text` parts for the sentinel scan and flag any `error` event — rather than a
  full typed event model. Why: the exact event JSON is "deferred until live" in
  the ADR; a tolerant parser keyed on the documented `text`/`error` shapes is
  unit-testable now and refinable when observed against a live run.
- Decision: leave `Plan.recommended_model` `None` and emit no `## Execution
  model` line in the OpenCode plan prompt (ADR-0005 D3/D8a). Why: OpenCode drops
  complexity routing, so there is no tier to parse or thread into execution.

## Steps
- [x] Scaffold the crate: create `crates/ralphy-agent-opencode/Cargo.toml`
      (name `ralphy-agent-opencode`, deps `anyhow`, `tracing`, `serde_json`,
      `ralphy-core`, all `.workspace = true`), mirroring
      `crates/ralphy-agent-codex/Cargo.toml`.
- [x] Register the crate in the workspace root `Cargo.toml`: add
      `"crates/ralphy-agent-opencode"` to `members` and
      `ralphy-agent-opencode = { path = "crates/ralphy-agent-opencode" }` to
      `[workspace.dependencies]`.
- [x] Add the new prompt asset `assets/prompts/prompt.plan.opencode.md` — a copy
      of `assets/prompts/prompt.plan.codex.md` with the entire `## Execution
      model: ...` block removed (D3/D8a) and the reviewer step rephrased from the
      Codex-subagent wording to OpenCode-neutral dispatch (no "independent
      subagent"/Claude Task-tool idiom; keep the "ONLY the commits you made"
      scoping).
- [x] Create `crates/ralphy-agent-opencode/src/lib.rs` with the `OpenCodeAgent`
      struct (`model: Option<String>`, `variant: Option<String>`,
      `run_dir: PathBuf`, `max_minutes_per_issue: u64`,
      `run_deadline: Option<Instant>`), `new(model, run_dir)`,
      `with_variant(variant)`, `with_run_deadline(...)`, and `issue_deadline()`
      — cloned from `CodexAgent`; embed `PROMPT_EXECUTE` (verbatim, reused) and
      `PROMPT_PLAN_OPENCODE` via `include_str!` to the two assets.
- [x] In `lib.rs`, add `build_opencode_command(model: Option<&str>, variant:
      Option<&str>, root: &Path) -> Command`: `Command::new("opencode")` with
      `run`, `--format json`, `--dangerously-skip-permissions` (always),
      conditional `-m <model>` (only when `model.is_some()`), conditional
      `--variant <variant>` (only when `variant.is_some()`), piped
      stdin/stdout/stderr, and `.env_remove("ANTHROPIC_API_KEY")` +
      `.env_remove("OPENAI_API_KEY")` (D5/D6).
- [x] In `lib.rs`, add `run_opencode(&self, cmd, prompt, timeout) -> Result<(bool,
      bool, String)>` — a clone of `run_codex`: reader threads before stdin
      write, `try_wait` poll loop, kill-on-timeout, combined log written to
      `run_dir/opencode.log`, returning `(exited_cleanly, timed_out, stdout_text)`.
- [x] In `lib.rs`, add `parse_opencode_events(stdout: &str) -> (String, bool)`:
      parse each non-blank line as `serde_json::Value`, concatenate assistant
      `text` parts into the returned string, and set the bool when a line is an
      `error` event; tolerant of unparseable lines (skip them).
- [x] In `lib.rs`, add `classify_opencode_outcome(exited_cleanly, timed_out,
      committed, text, saw_error) -> Outcome` (ADR-0005 D2, no `Limit` branch):
      `Timeout` if `timed_out`; `Blocked(reason)` if `text` carries
      `RALPHY_BLOCKED_EXIT`; `Done` only if `exited_cleanly && committed &&
      !saw_error && text.contains("RALPHY_DONE_EXIT")`; else `Stuck`.
- [x] In `lib.rs`, implement `Agent for OpenCodeAgent`: `plan` runs
      `build_opencode_command(self.model, self.variant, repo_root)` with
      `PROMPT_PLAN_OPENCODE`, then reads `plan.md` into a `Plan` with
      `open_steps = plan::count_open_steps(md)` and `recommended_model: None`;
      `execute` records `head_sha` before/after, runs `PROMPT_EXECUTE`, calls
      `parse_opencode_events` then `classify_opencode_outcome`, and returns the
      `Outcome` (mirrors `CodexAgent::plan`/`execute`, minus the limit/auth paths).
- [x] In `crates/ralphy-cli/src/main.rs`, add `OpenCode` to the `CliAgent` enum
      (line ~142) and a `--exec-variant` `Option<String>` field to `RunArgs`
      (near `exec_model`, line ~99).
- [x] In `crates/ralphy-cli/src/main.rs`, add the `CliAgent::OpenCode` arm to the
      `match args.agent` adapter selection (line ~230): box
      `OpenCodeAgent::new(non_empty(args.exec_model...), run_dir)
      .with_variant(non_empty(args.exec_variant...)).with_run_deadline(...)` as
      `Box<dyn Agent>`; add `use ralphy_agent_opencode::OpenCodeAgent;`.
- [x] Add `#[cfg(test)] mod tests` to `lib.rs` mirroring the Codex adapter's
      depth — these FAIL before the impls exist and PASS after:
      `classify_opencode_outcome` (Done; Stuck-on-no-commit; Blocked;
      Stuck-on-non-zero-exit; Stuck-on-error-event; Stuck-on-no-sentinel;
      Timeout-wins); `build_opencode_command` (no `-m` when `model=None`; `-m`
      present when `Some`; `--variant` only when `Some`;
      `--dangerously-skip-permissions` + `--format json` always; both API keys
      `env_remove`d); `parse_opencode_events` (extracts text parts, flags an
      `error` event); `prompt.plan.opencode` has no `## Execution model` line and
      keeps the reviewer step; and `OpenCodeAgent: Agent` (a `&dyn Agent` bind).
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over
      ONLY the commits made for this issue (this run's branch may carry earlier
      issues — review just these commits). Resolve every HIGH finding before
      finishing; if one cannot be fixed autonomously, record it under `## Notes &
      decisions` and block instead of declaring done.
- [x] Run `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test` — all
      pass with no new warnings.

## Notes & decisions
- `cargo fmt --check` reports pre-existing formatting drift in
  `crates/ralphy-agent-claude/src/lib.rs` (lines 325, 1280, 1287) and
  `crates/ralphy-agent-codex/src/lib.rs` (line 217) that is present in committed
  `HEAD` before this issue. Those files are out of scope (the "untouched"
  criterion), so they were left as-is rather than reformatted. The new crate,
  the new prompt asset, and `main.rs` are all fmt-clean — this issue introduces
  no new fmt/clippy warnings. Reviewer to confirm the pre-existing drift is not a
  regression from this work.
- Self-review (general-purpose reviewer over commits 5024304..44c8446): no HIGH
  or MEDIUM defects. One deferred-D9 item surfaced for the follow-up — `main.rs`
  `effective_stop_on_limit` is not yet forced for `CliAgent::OpenCode` (ADR-0005
  D9 wants it forced, alongside the Limit detection this tracer scopes out). It
  is correctly absent here (no Limit handling in the tracer); the PR reviewer
  should track it for the D9 slice so an OpenCode limit never attempts the
  ADR-0003 auto-resume hang.
