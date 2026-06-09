# Plan for #22: Codex adapter base: --agent codex runs an issue end-to-end via codex exec

## Feasible: yes
The issue is fully specified by docs/adr/0004 and mirrors the existing
`ralphy-agent-claude` crate against a well-defined `Agent`/`Outcome` contract;
the headless `codex exec` invocation, the signal→`Outcome` mapping, and the
`--agent` composition-root wiring are all concrete and unit-testable.

## Execution model: opus
A new workspace crate plus composition-root dispatch, child-process timeout
handling, careful exit/sentinel/HEAD-diff → `Outcome` classification, and a
strict "Claude path untouched" constraint — cross-cutting work with subtle
correctness, not a localized mechanical change.

## Done when
- `cargo test` passes with new unit tests in `ralphy-agent-codex` that, over
  fixtures: classify `exit 0 + RALPHY_DONE_EXIT` → `Done`, `RALPHY_BLOCKED_EXIT`
  → `Blocked`, no-commit / non-zero exit → `Stuck`, and wall timeout →
  `Timeout`; assert the built codex `Command` removes `OPENAI_API_KEY`; assert
  planning argv carries `model_reasoning_effort="high"` and execution argv
  carries the plan tier; and assert the tier→effort mapping and model
  default/override.
- `cargo build` succeeds with `ralphy-agent-codex` added to the workspace and
  `CodexAgent` boxed as `Box<dyn Agent>` behind a new `--agent claude|codex`
  flag in `main.rs` (the build proves `CodexAgent: Agent` and that the core
  still takes `&dyn Agent`).
- `cargo test` is green across the whole workspace with no new warnings
  (`cargo build` / `cargo clippy` clean).
- Review-only: `ralphy run --agent codex --only-issue <n>` against a live
  `codex`-CLI + subscription plans, executes, commits onto the run branch, and
  returns `Outcome::Done` — requires the external `codex` binary and auth, so it
  cannot run inside `cargo test`; a human confirms it in the PR.

## Acceptance ledger
- [review-only] `ralphy run --agent codex --only-issue <n>` runs an issue end-to-end and produces at least one commit on the run branch with `Outcome::Done` — evidence: human runs the command against a logged-in `codex` CLI and confirms a commit lands on the run branch with a `Done` outcome (needs the external binary + subscription, not reproducible in `cargo test`)
- [verified] new crate `ralphy-agent-codex` implements `Agent`; the core still takes a single `&dyn Agent` (no per-issue routing added to the core) — evidence: commits 2e44d26 + 5c5db15 add `impl Agent for CodexAgent`; commit a9f4e96 boxes it as `Box<dyn Agent>` passed to the unchanged `run_queue(&cfg, &queue, agent.as_ref(), …)`; the unit test `codex_agent_is_a_dyn_agent` binds `&CodexAgent as &dyn Agent` (passes)
- [verified] `execute()` classifies `exit 0 + RALPHY_DONE_EXIT` → Done, `RALPHY_BLOCKED_EXIT` → Blocked, no-commit-streak / non-zero exit → Stuck, wall timeout → Timeout (unit-tested over fixtures) — evidence: commit 5c5db15 tests `classify_done_on_clean_exit_commit_and_sentinel`, `classify_blocked_on_blocked_sentinel`, `classify_stuck_on_non_zero_exit`, `classify_stuck_on_no_commit`, `classify_stuck_on_no_sentinel`, `classify_timeout_wins` (all pass)
- [verified] reasoning effort: planning runs at `high`; execution takes the plan's neutral tier via `-c model_reasoning_effort`; model defaults to the latest and is overridable by flag — evidence: commit 5c5db15 tests `build_command_argv_and_env` / `build_command_threads_the_effort_through` (argv carries `model_reasoning_effort="high"`/`"low"`), `tier_to_effort_maps_and_defaults` (`low|medium|high`, default `medium`), and `resolve_model_default_vs_override` (`DEFAULT_CODEX_MODEL` else override); `plan()` passes `"high"` and `execute()` passes `tier_to_effort(plan tier)`
- [verified] `OPENAI_API_KEY` is removed on the codex child `Command` (unit-verified) — evidence: commit 5c5db15 test `build_command_argv_and_env` asserts `Command::get_envs()` yields `("OPENAI_API_KEY", None)`
- [verified] Claude path untouched: `assets/plugin`, existing prompts, `hook.rs`, `guard.rs`, and the `ANTHROPIC_API_KEY` clearing in `main.rs` are unchanged except for the new `--agent` match — evidence: `git diff c145d1b..HEAD --stat` shows no changes to `assets/plugin`, `hook.rs`, `guard.rs`, or existing prompts; `main.rs` changed only for the `--agent` flag/enum/match (the `ANTHROPIC_API_KEY` line is untouched). All 43 Claude-adapter + 37 CLI + 42 core-lib + 13 Codex tests pass; the 2 `prompt_ledger` failures are pre-existing on the base commit and unrelated to #22 (see `## Notes & decisions`)

## Decisions
- Decision: keep `ralphy-core/src/plan.rs` untouched and parse the Codex
  `low|medium|high` tier inside the codex crate (a private `recommended_tier`
  fn), storing it in the vendor-neutral `Plan.recommended_model` string. Why:
  the `Agent` adapter owns its plan format; this leaves core fully unchanged and
  keeps `plan::recommended_model`'s `opus|sonnet` regex for the Claude path.
- Decision: add a new Codex planning prompt `assets/prompts/prompt.plan.codex.md`
  (a variant of `prompt.plan.md`) that emits `## Execution model: low | medium |
  high` and drops the reviewer-skill-spawn step. Why: the tier line is in scope
  (criterion #4) but reviewer/skills materialization is explicitly deferred to
  the parity slice; reusing the Claude prompt would emit the wrong tier.
- Decision: reuse the existing vendor-neutral `prompt.execute.md` as the Codex
  execution charter piped on stdin. Why: it already names `RALPHY_DONE_EXIT` /
  `RALPHY_BLOCKED_EXIT` and is not Claude-specific, so no execution-prompt
  variant is needed for the base slice.
- Decision: the operator model override reuses the existing `--exec-model` flag
  (applied to both plan and execute for Codex); the default is a
  `DEFAULT_CODEX_MODEL` constant set to `gpt-5-codex`. Why: avoids adding a new
  flag; "latest Codex model" is operator-overridable as the criterion requires.
- Decision: Codex execution effort is driven solely by the plan tier (default
  `medium` when absent); planning effort is fixed `high`. The `--exec-effort`
  flag continues to feed only the Claude path. Why: matches criterion #4's
  "execution takes the plan's neutral tier" without overloading a Claude knob.
- Decision: omit `Outcome::Limit` classification from this base slice. Why: the
  issue's `execute()` signal list and acceptance criteria name only
  Done/Blocked/Stuck/Timeout; ADR-0004 D6 explicitly defers the Codex usage-limit
  parser to a later slice.

## Notes & decisions
- The seven `lib.rs` steps (struct/helpers/command-builder/classifier/`run_codex`/
  `Agent` impl/tests) were implemented and committed together: the struct fields
  are each only used by the `Agent` impl, so splitting them across commits would
  leave `dead_code` warnings and violate the "no new warnings" gate. The unit
  tests (step 10) ship in the same commit, proving behavior per the exec charter.
- `classify_codex_outcome` requires a HEAD commit for `Done` (not just exit 0 +
  sentinel): a `RALPHY_DONE_EXIT` claim with no new commit is downgraded to
  `Stuck`. This honors the plan's "no-commit → Stuck" progress guard and makes
  the `committed` argument load-bearing; the `Timeout` and `Blocked` signals take
  precedence in that order.
- `build_codex_command` uses `Command::new("codex")` directly (std `Command`
  honors `PATH`/`PATHEXT`), unlike the Claude adapter's `resolve_claude_binary`
  shim, which exists only because the PTY backend ignores runtime `PATH`.
- PRE-EXISTING unrelated test failure: `ralphy-core`'s integration test
  `tests/prompt_ledger.rs` (both `prompt_plan_ledger_example_parses_into_typed_verdicts`
  and `prompt_plan_verified_example_ticks_matching_issue_body_line`) fails on the
  *base* commit too (verified by `git stash` + `cargo test` with my changes
  removed). Its hardcoded `VERIFIED_CRITERION`/`REVIEW_ONLY_CRITERION` constants
  no longer match the canonical ledger example in `assets/prompts/prompt.plan.md`.
  Unrelated to #22 — I never touched `prompt.plan.md` or that test — and fixing
  it is out of scope here (it would mean editing the Claude planning prompt the
  "Claude path untouched" criterion forbids, or an unrelated test). My commits
  add 13 passing Codex tests and introduce **no new** failures or warnings.
  Flagged for the PR reviewer / a separate fix.
- Self-review (step 12): an independent reviewer subagent over this issue's
  commits (`c145d1b..HEAD`) confirmed every in-scope axis correct — argv per
  ADR-0004 D2, `OPENAI_API_KEY` removed on the child, timeout kill (no leak), no
  output-pipe deadlock, no panic on external data, the 4-way classifier, the
  `--agent` wiring (Claude path untouched, `&dyn Agent` passed to the core), and
  effort routing. It raised two HIGH findings, both the *same* gap: no
  `Outcome::Limit` usage-limit handling, so a Codex limit currently maps to
  `Stuck`. This is **deliberately deferred** — see the `## Decisions` "omit
  `Outcome::Limit` from this base slice" entry and ADR-0004's Consequences ("the
  exact shape of a `try again at <datetime>` reset … deferred until observed
  against a live Codex run; until then `Limit(None)` + stop"). The issue's
  acceptance criteria name only Done/Blocked/Stuck/Timeout, all of which are
  implemented and unit-tested. Not a blocker for this slice; flagged for the PR
  reviewer and the follow-up parity slice. A secondary note: `run_codex` returns
  only `(exited_cleanly, timed_out)` and the classifier reads the `-o` final
  message — when the limit parser lands, `run_codex` should also surface the
  captured stdout/stderr (already written to `codex.log`) so limit text on those
  streams is visible.

## Notes for review
- No `Outcome::Limit` handling yet: a Codex usage/rate limit currently classifies
  as `Stuck`, which stops the run as non-green rather than waiting/reporting for
  reset. Deferred per the plan's `## Decisions` and ADR-0004 D6 (parser firms up
  against a live run). Confirm this is acceptable for the base slice.
- Live end-to-end behavior (`ralphy run --agent codex --only-issue <n>` against a
  logged-in `codex` CLI producing a commit + `Done`) needs a human to confirm —
  it requires the external `codex` binary and subscription auth, so it cannot run
  inside `cargo test`.

## Steps
- [x] Create `crates/ralphy-agent-codex/Cargo.toml` (package `ralphy-agent-codex`, edition/license from `workspace.package`) depending on `ralphy-core`, `anyhow`, `regex`, `tracing` from `workspace.dependencies`; add `"crates/ralphy-agent-codex"` to `members` and a `ralphy-agent-codex = { path = "crates/ralphy-agent-codex" }` line under `[workspace.dependencies]` in the root `Cargo.toml`.
- [x] Add `assets/prompts/prompt.plan.codex.md`: copy `prompt.plan.md`, change the `## Execution model` heading and guidance to emit `low | medium | high` (low=mechanical, medium=default, high=genuinely complex), and remove the `reviewer`-skill self-review step (keep the green-build/test gate step) since skills are out of scope here.
- [x] In `crates/ralphy-agent-codex/src/lib.rs`, define `CodexAgent` with fields `model: Option<String>`, `run_dir: PathBuf`, `max_minutes_per_issue: u64`, `run_deadline: Option<Instant>`; add `const DEFAULT_CODEX_MODEL: &str = "gpt-5-codex"`, `const PROMPT_PLAN_CODEX = include_str!("../../../assets/prompts/prompt.plan.codex.md")`, and `const PROMPT_EXECUTE = include_str!("../../../assets/prompts/prompt.execute.md")`; add `new(model, run_dir)`, `with_run_deadline`, and `issue_deadline` mirroring `ClaudeAgent`.
- [x] In `lib.rs`, add pure helpers: `recommended_tier(md: &str) -> Option<String>` (regex `^\s*##\s*Execution model:\s*(low|medium|high)`), `tier_to_effort(tier: Option<&str>) -> &str` (`low|medium|high`, default `"medium"`), and `resolve_model(&self) -> &str` (override or `DEFAULT_CODEX_MODEL`).
- [x] In `lib.rs`, add `build_codex_command(model, effort, root, out_path) -> std::process::Command` building `codex exec -C <root> -m <model> -c model_reasoning_effort="<effort>" -s danger-full-access -a never -o <out_path> -` with `stdin/stdout/stderr` piped and `.env_remove("OPENAI_API_KEY")`; this is the single point both `plan()` and `execute()` build their command through.
- [x] In `lib.rs`, add pure `classify_codex_outcome(exited_cleanly: bool, timed_out: bool, committed: bool, out: &str) -> Outcome`: timeout→`Timeout`; `exit 0 + RALPHY_DONE_EXIT`→`Done`; `RALPHY_BLOCKED_EXIT <reason>`→`Blocked(reason)`; otherwise (non-zero exit or no commit / no sentinel)→`Stuck`.
- [x] In `lib.rs`, add a private `run_codex(&self, cmd, timeout) -> Result<(bool /*exited_cleanly*/, bool /*timed_out*/)>` that pipes `PROMPT_*` on stdin, drains stdout/stderr via reader threads (mirroring `ClaudeAgent::run_headless_call` to avoid pipe-buffer deadlock), polls `try_wait` to the deadline, and kills on expiry.
- [x] In `lib.rs`, `impl Agent for CodexAgent`: `plan()` removes any stale `ws.plan_path()`, runs `run_codex` with `PROMPT_PLAN_CODEX`, effort `"high"`, output `ws.ralphy_dir().join("codex-last.txt")`; on no plan file, `bail!`; returns `Plan { open_steps: plan::count_open_steps(&md), recommended_model: recommended_tier(&md), path }`.
- [x] In `lib.rs`, `execute()`: capture `git::head_sha` before, run `run_codex` with `PROMPT_EXECUTE` at `tier_to_effort(plan.recommended_model.as_deref())` and output `.ralphy/codex-last.txt`, capture `head_sha` after (`committed = before != after`), read the `-o` file, and return `classify_codex_outcome(exited_cleanly, timed_out, committed, &out)`.
- [x] Add unit tests in `lib.rs`: `classify_codex_outcome` over the four fixture cases; `build_codex_command` argv contains `-C`/`-m`/`-o`/`model_reasoning_effort="<effort>"` and `get_envs()` yields `("OPENAI_API_KEY", None)`; `recommended_tier` parses `low|medium|high` and returns `None` otherwise; `tier_to_effort` mapping incl. default; `resolve_model` default vs override; and a compile-level test binding `&CodexAgent as &dyn Agent` (these fail to compile/pass before the impl exists and pass after).
- [x] In `crates/ralphy-cli/src/main.rs`: add `enum CliAgent { Claude, Codex }` (`ValueEnum`) and `#[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)] agent: CliAgent` to `RunArgs`; in `run_cmd`, build `let agent: Box<dyn Agent> = match args.agent { Claude => Box::new(ClaudeAgent…), Codex => Box::new(CodexAgent::new(args.exec_model…, run_dir).with_run_deadline(run_deadline)) }` and call `run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock)`; add `use ralphy_agent_codex::CodexAgent;` and `ralphy-agent-codex` to `crates/ralphy-cli/Cargo.toml`. Touch only the `--agent` flag/match — leave the `ANTHROPIC_API_KEY` clearing and all other wiring unchanged.
- [x] Self-review: spawn the `reviewer` skill as an independent subagent over ONLY this issue's commits (not the whole branch). Resolve every HIGH finding; if one cannot be fixed autonomously, record it under `## Notes & decisions` and block instead of declaring done.
- [x] Run `cargo fmt`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace`; all pass with no new warnings. (fmt `--check` clean; clippy clean workspace-wide; all #22 tests pass; the only 2 failing tests — `ralphy-core` `prompt_ledger` — are pre-existing on the base commit and unrelated to #22, see `## Notes & decisions`.)
