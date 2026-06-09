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
- [verified] new crate `ralphy-agent-codex` implements `Agent`; the core still takes a single `&dyn Agent` (no per-issue routing added to the core) — evidence: the crate compiles with `impl Agent for CodexAgent`, `main.rs` boxes it as `Box<dyn Agent>` passed to the unchanged `run_queue(&cfg, &queue, agent.as_ref(), …)`, and a unit test binds `&CodexAgent as &dyn Agent`
- [verified] `execute()` classifies `exit 0 + RALPHY_DONE_EXIT` → Done, `RALPHY_BLOCKED_EXIT` → Blocked, no-commit-streak / non-zero exit → Stuck, wall timeout → Timeout (unit-tested over fixtures) — evidence: new tests drive `classify_codex_outcome` over fixtures for each of the four cases
- [verified] reasoning effort: planning runs at `high`; execution takes the plan's neutral tier via `-c model_reasoning_effort`; model defaults to the latest and is overridable by flag — evidence: tests assert the planning `Command` argv contains `-c model_reasoning_effort="high"`, the execution argv contains the plan tier, `tier_to_effort` maps `low|medium|high` (default `medium`), and the model resolves to `DEFAULT_CODEX_MODEL` or the override
- [verified] `OPENAI_API_KEY` is removed on the codex child `Command` (unit-verified) — evidence: a test inspects `Command::get_envs()` and asserts `("OPENAI_API_KEY", None)` is present
- [verified] Claude path untouched: `assets/plugin`, existing prompts, `hook.rs`, `guard.rs`, and the `ANTHROPIC_API_KEY` clearing in `main.rs` are unchanged except for the new `--agent` match; full `cargo test` is green — evidence: `git diff` over this issue's commits shows no changes to those files beyond the new `--agent` flag/match in `main.rs`, and `cargo test` passes workspace-wide

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
- [ ] In `crates/ralphy-cli/src/main.rs`: add `enum CliAgent { Claude, Codex }` (`ValueEnum`) and `#[arg(long = "agent", value_enum, default_value_t = CliAgent::Claude)] agent: CliAgent` to `RunArgs`; in `run_cmd`, build `let agent: Box<dyn Agent> = match args.agent { Claude => Box::new(ClaudeAgent…), Codex => Box::new(CodexAgent::new(args.exec_model…, run_dir).with_run_deadline(run_deadline)) }` and call `run_queue(&cfg, &queue, agent.as_ref(), &tracker, &clock)`; add `use ralphy_agent_codex::CodexAgent;` and `ralphy-agent-codex` to `crates/ralphy-cli/Cargo.toml`. Touch only the `--agent` flag/match — leave the `ANTHROPIC_API_KEY` clearing and all other wiring unchanged.
- [ ] Self-review: spawn the `reviewer` skill as an independent subagent over ONLY this issue's commits (not the whole branch). Resolve every HIGH finding; if one cannot be fixed autonomously, record it under `## Notes & decisions` and block instead of declaring done.
- [ ] Run `cargo fmt`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace`; all pass with no new warnings.
