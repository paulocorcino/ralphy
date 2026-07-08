---
name: staged-plan
description: Design a self-contained multi-stage plan whose markdown is the operational contract — every execution detail (Execution model, Hand-off conventions, retry rule, working-tree policy, reviewer gate, pre-execution placeholder gate) is encoded in the plan file itself. This is a PLANNING skill — it produces a plan and stops. Use when the user wants to design, scaffold, or decompose work into a staged subagent track. Typical invocations - "design a staged plan", "decompose this into stages", "scaffold a multi-stage plan", "plan in stages", "create a staged execution plan". Do NOT invoke during Phase 2 execution — the plan markdown is self-sufficient and re-invoking the skill is redundant.
---

# Staged Plan

A pattern for executing large tracks as a chain of autonomous subagents, each in its own fresh context window, with the parent gating green-to-green. The plan markdown is **the operational contract** — anyone (or any executor) opening it cold should be able to run it correctly.

## Skill scope — planning only

This skill produces a plan. It does **not** execute code changes, run stages, or modify source files. Even when invoked outside plan mode, behave as a planner: investigate the codebase (read-only), design stages, run the scaffold, fill the markdown, optionally make the Phase 1.5 landing commit (plan + verify scripts + `.gitignore` rule only — no feature work). Stop there. Phase 2 is driven by the plan markdown, not by re-invoking this skill.

If the user asks to "execute the plan" or "run Stage N" while this skill is active, the answer is: the plan file already contains the Execution model and hand-off prompts needed; the user (or another agent) follows the plan directly. Do not start launching subagents from inside this skill.

## Pattern taxonomy

This is **prompt chaining with sectioning + gate checks** in Anthropic's effective-agents taxonomy — NOT orchestrator-workers. Subtasks are pre-planned, run strictly sequentially, and each gate must pass before advancing. Choose this when the decomposition is knowable at plan time (typical for coding tracks). Cost scales roughly linearly with stage count; justified when a single session can't carry the track or contextual isolation between stages materially improves quality.

## When this applies

- User asks to **design** a staged plan, **decompose** a track into subagent stages, or **scaffold** a multi-stage execution plan
- A track is too large for one session but splits cleanly into 3-7 independent deliverables
- Each stage benefits from contextual isolation
- The user has pre-authorized commits (or wants gated commits between stages)

## When NOT to use

- Track fits comfortably in one session — execute inline
- Subtask shape is unknowable until runtime — use orchestrator-workers
- No meaningful gates between steps — the green-to-green audit is the main value
- **Phase 2 (execution) is already underway** — the plan markdown is the contract; do NOT re-invoke this skill to "remember" how to launch stages, verify green, or apply retries. Read the plan's `## Execution model` block instead.

## Workflow (overview)

| Phase | Owner | What happens |
|---|---|---|
| 1. Plan design | Planner | Read every Critical file end-to-end → run `scaffold.py` → fill `<FILL>` placeholders via `Edit` |
| 1.5. Plan landing commit | Planner | Commit plan + `_verify.py` + verify scripts + `.gitignore` rule |
| 2. Execution | Parent + subagents | Pre-execution gate, launch stages, verify green, retry, end-to-end + reviewer |

Detailed Phase 1.5 + Phase 2 mechanics: see `references/execution.md`.

## Phase 1 — Plan design

**Investigation discipline (mandatory before designing stages):** read every file that will appear in the cross-stage `## Critical files` index **end-to-end**, not just grep snippets. Plans built from excerpts produce stages with stale line numbers, missed callers, and hidden dependencies. If a file is too large to read fully, that's a signal the stage decomposition is wrong — split further.

**Symbol-fallout scan (mandatory when the plan removes, renames, or changes the signature of any exported symbol):** for each such symbol, run `grep -rn '<symbol>' <repo>` (or equivalent). Every hit outside the file declaring the symbol is a call-site that MUST appear in the scope of some stage — either patched, or explicitly deferred with a `// TODO(slice-N)` shim. If you cannot enumerate the call-sites at plan time, the decomposition is incomplete; do not scaffold yet. The backlog specifies *intent*, not *fallout* — back it with an explicit grep.

After investigation, decide the defaults below (slug, title, stage list, working-tree policy, reviewer recommendation, report policy). Then **stop and gate** before any file write (see *Pre-scaffold approval gate* below). On approval, run `scaffold.py` (Bash) to write the plan file at `<repo-root>/docs/plans/<slug>.md`, and use `Edit` to replace each `<FILL: ...>` and resolve each `<FILL-OR-DELETE: ...>` block. Never hand-write the plan markdown — the scaffold renders ~60% of the boilerplate deterministically and is the only path that respects the location guard.

### Pre-scaffold approval gate (mandatory, both modes)

Phase 1 investigation is **read-only by contract**. No `Write`, `Edit`, `scaffold.py`, or any file-creating Bash until the user has explicitly approved the materialization. This in-skill gate exists so the skill behaves the same whether or not the harness's plan mode is active — it does not rely on plan mode to prevent premature writes.

When investigation is complete, present a short summary (slug, title, stage list, mode, working-tree, reviewer + reason, report-policy) and request approval. **Pick exactly one gate based on context — never call more than one:**

- **Inside plan mode** (detection: the `ExitPlanMode` tool is available in your tool list): call `ExitPlanMode` with the summary. The harness's approval doubles as the gate. On approval, plan mode exits and `scaffold.py` can run. Do NOT dump a hand-written plan markdown into `ExitPlanMode` — the summary is a few lines; scaffold renders the full plan. Do NOT additionally call `AskUserQuestion` — that is double-prompting.
- **Outside plan mode, interactive** (detection: `ExitPlanMode` is NOT in your tool list AND `STAGED_PLAN_NONINTERACTIVE` is unset or not `1` — check via `Bash`: `echo "${STAGED_PLAN_NONINTERACTIVE:-0}"`): call `AskUserQuestion` with the summary and options `[scaffold / adjust / cancel]`. On `scaffold`, proceed; on `adjust`, revise and re-gate; on `cancel`, stop. Do NOT skip this step "because the plan looks obvious" — the gate is the user's only checkpoint before the landing commit.
- **Non-interactive** (detection: `STAGED_PLAN_NONINTERACTIVE=1` in env, set explicitly by the calling harness; e.g. `backlog-claude-runner` invoking the skill via `claude -p`): there is no human to prompt and `AskUserQuestion` would hang. Instead, emit the summary as a single delimited block to stdout BEFORE running `scaffold.py`, then proceed directly to scaffold + fill. The calling harness captures the block for post-hoc review. Emit exactly:

  ```
  <<PRE_SCAFFOLD_SUMMARY>>
  slug: <plan-slug>
  title: <plan title>
  stages:
    - <Stage 1 title>
    - <Stage 2 title>
    - ...
  mode: autonomous
  working-tree: <clean-required|...>
  reviewer: <none|light|deep>  reason: <reason or "—">
  report-policy: <committed|gitignored>
  output: <absolute path of plan file>
  <<END>>
  ```

  This mode trusts the upstream prompt to have pre-specified the work. It MUST NOT be entered opportunistically (e.g. "the agent thinks no human is watching") — only when the env var is set explicitly by the caller. If the env var is unset and no plan-mode tool is available, fall back to `AskUserQuestion` and do not write anything.

The rule is identical in all three cases: **no file writes in Phase 1 until either the user approves (interactive paths) or the harness has set `STAGED_PLAN_NONINTERACTIVE=1` and the summary block has been emitted (non-interactive path).**

### Fixed defaults — do NOT prompt the user for these

1. **Mode:** `autonomous` — stages run end-to-end with no pause between them.
2. **Commit authorization:** `per-stage-direct` — each subagent commits after green gates.
3. **Failure handling:** `auto-retry-up-to-2` — re-launch the stage with the failure excerpt, max 2 retries, then pause. Scope violations never auto-retry.
4. **Working-tree policy:** `clean-required` if `git status` is clean (the common case). Other states detailed in `references/working-tree.md`.
5. **Reviewer gate:** `none` unless risk signals trigger an auto-recommendation (see below).
6. **Report policy:** `committed` unless `.gitignore` already excludes report files (e.g., `*-report.md`, `docs/plans/*-report.md`), in which case use `gitignored`. Detect during investigation by inspecting `.gitignore`; do not ask the user.

These defaults must be recorded verbatim under `## Execution policy` in every plan. Only deviate if the user explicitly overrides in the current conversation.

### The one allowed question

When `git status` is **not clean** at plan time, the working-tree policy cannot be defaulted safely. Briefly summarize the dirty state and ask the user to choose between `stash-authorized`, `integrate-existing`, or `abort-until-clean` (see `references/working-tree.md`). Single question, not a menu of unrelated decisions.

### Per-stage Tier and Effort classification (planner job)

Every stage block carries `Tier:` and `Effort:` declarations plus a `Tier rationale:` line. The scaffold seeds them at `standard / standard`; the planner adjusts during fill based on the work the stage actually does:

| Stage shape | Default classification |
|---|---|
| 1 file, linear order-of-ops, mechanical edits (rename, move, apply listed pattern) | `mechanical / minimal` |
| 2-5 files, typical coding with light judgment inside the file list | `standard / standard` |
| Decision about scope, callsite discovered mid-work, semantic refactor, non-obvious ordering invariant | `judgment / extended` |
| Security, public/cross-repo contract, data migration, irreversible change | `critical / extended` |

The `Tier rationale:` line is mandatory — 1-2 lines naming the specific decision or risk that justifies the tier. This is what the executor reads to confirm the classification fits, and what a reviewer reads to spot over-classification. The plan **never** names a model; mapping `Tier × Effort` to platform resources is the executor's job at runtime.

### Auto-recommend reviewer gate

Decide the recommendation during Phase 1a investigation; pass it to `scaffold.py` via `--reviewer` (and `--reviewer-reason` if non-`none`). Surface the reason in the pre-scaffold approval summary (`ExitPlanMode` or `AskUserQuestion`, per Phase 1) so the user can override before materialization.

- `reviewer: deep` — recommended when ≥2 of: ≥5 stages, public/cross-repo contract change, Docker/CI changes, auth or data migration, multi-repo touch.
- `reviewer: light` — recommended when exactly 1 of those signals is present.
- `reviewer: none` — otherwise (default).

State the recommendation **with the reason** so the user can override in one edit:

```
Reviewer: deep — recommended by: 7 stages + public contract + multi-repo. Override with `Reviewer: none` to skip.
```

### Plan output location (mandatory pre-check)

Before invoking `scaffold.py`, the planner MUST run:

```
git rev-parse --show-toplevel
```

- If the command **fails** (cwd is not inside a git repo): abort and instruct the user to re-invoke the skill from inside the target repo. Do NOT scaffold to `~/.claude/plans/` for repo work — that breaks Phase 1.5 (vendoring `_verify.py`, narrowing `.gitignore`, landing commit) and forces the executor to improvise at runtime.
- If the command **succeeds**: the **canonical** output path is `<that-path>/docs/plans/<plan-slug>.md`. Post-stage reports go in the same directory. Deviate from `docs/plans/` only when a calling harness mandates a different in-repo location (e.g. `backlog-claude-runner` may direct the plan to `.agents/...`); in that case the harness MUST set `STAGED_PLAN_NONINTERACTIVE=1` and provide the absolute target path. Never invent a non-canonical location yourself.

`scaffold.py` enforces this: `--output` must resolve **inside a git repo** (exit code 4 otherwise, unless `--allow-outside-repo` is passed for the rare no-repo fallback). Any in-repo path is accepted; the scaffold derives the plan dir from `--output` and rewrites every boilerplate reference to it (verify scripts, report template, logs dir, Plan landing commit advice, reviewer-output dir). Landing the plan directly at the repo root is rejected.

**Never** write to `<repo>/.claude/plans/` — deprecated, triggers permission prompts for subagents.

### Scaffold first, then fill (mandatory)

Once you have decided slug, title, and the list of stage titles, do NOT hand-write the markdown. Run the scaffold script — it deterministically renders ~60% of the plan (Execution model, Execution policy, Executor adapter, Stage 0, hand-off template per stage, End-to-end block, Reviewer gate when applicable) so you only edit the cognitive parts (per-stage scope, files, order of operations, hand-off specifics, Context, Alternatives, Open questions).

```bash
python3 ~/.claude/skills/staged-plan/lib/scaffold.py \
  --slug <plan-slug> \
  --title "<Plan Title>" \
  --stage "<Stage 1 title>" \
  --stage "<Stage 2 title>" \
  ... \
  --output <repo>/docs/plans/<plan-slug>.md \
  --mode autonomous \
  --working-tree clean-required \
  --reviewer none \
  --report-policy committed
```

**Safety:** `--output` is required. The script refuses to overwrite an existing file (exit 3) unless `--force` is passed — this protects filled plans from accidental rescaffold.

Flags:
- `--mode`: `autonomous` (default) | `semi-autonomous`.
- `--working-tree`: `clean-required` (default) | `stash-authorized` | `integrate-existing` | `abort-until-clean`.
- `--reviewer`: `none` (default) | `light` | `deep`. If non-`none`, also pass `--reviewer-reason "<short reason>"`.
- `--report-policy`: `committed` (default) | `gitignored`. Decide during investigation by inspecting `.gitignore`; surface alongside reviewer in the recommendation block.

### After scaffolding — fill rules

1. Every `<FILL: ...>` placeholder must be replaced with real content before the Phase 1.5 landing commit. No `<FILL>` survives in the final plan — the pre-execution gate (`assert_no_placeholders`) will refuse to launch Stage 1 otherwise.
2. `<FILL-OR-DELETE: ...>` blocks — fill if you have content; delete the entire block if you don't. The planner decides, not the user:
   - `## Alternatives considered`: fill if you genuinely considered >1 stage decomposition; delete otherwise.
3. **`<repo>` is already substituted** by `scaffold.py` to the absolute repo root (auto-detected from `--output`, or explicit via `--repo-root`). It is NOT a `<FILL>` to resolve. If you see literal `<repo>` survive in the rendered plan, you ran with `--allow-outside-repo` and must substitute manually before the landing commit.
4. **Edit anchors per stage:** each FILL-bearing block in a stage carries an HTML comment like `<!-- STAGE 3: files -->` immediately above it. Use that marker (plus the FILL line) as the `old_string` in `Edit` calls — it makes the match unique without needing large surrounding context, and it scales when you have 6+ stages with structurally identical blocks.

The scaffold is a starting point — modify freely. Do NOT re-run scaffold after editing; it will overwrite your work.

### End of Phase 1

Always print (so the IDE renders a clickable link):

```
Plan file: [<plan-slug>.md](/absolute/path/to/docs/plans/<plan-slug>.md#L1)
```

### Plan structure, hand-off prompt, verify scripts

- Full plan markdown skeleton + Stage 0 template + stage block layout: see `references/plan-structure.md`.
- Per-stage hand-off prompt template: see `references/handoff-template.md`.
- When to generate a verify script and what it looks like: see `references/verify-scripts.md`.
- A fully-filled minimal example: see `examples/migration-x.md`.

## Phase 1.5 — Plan landing commit

After the pre-scaffold approval gate clears (either `ExitPlanMode` or `AskUserQuestion`, per Phase 1), the **planner** (not a subagent) makes a single commit landing the plan + `_verify.py` + any verify scripts + the `.gitignore` rule for `docs/plans/logs/` (and `docs/plans/*-report.md` if report-policy is `gitignored`). Pre-check `.gitignore` first — full procedure in `references/execution.md`.

After the landing commit, working tree is clean and Phase 2 starts.

## Phase 2 — Execution

The Execution policy in the plan declares Mode, retry, working tree, and reviewer; the parent reads them and proceeds without further prompting (except the semi-autonomous between-stage checkpoint).

1. **Pre-execution gate:** run `assert_no_placeholders` against the plan file. If non-zero, abort and surface the offending lines — do NOT launch any stage.
2. **Launch each stage** per `## Executor adapter` in the plan.
3. **On completion, verify green** — autonomous: launch next; semi-autonomous: post checkpoint and wait `[y / edit / abort]`; red: apply retry rule.
4. **Retry:** up to 2 auto-retries; never on scope violations or hook bypasses.
5. **After final stage:** end-to-end verification, reviewer gate (if configured), summary table.
6. **Human-interaction stops** (reviewer BLOCKED, retry exhausted, scope violation, dirty-tree mid-run): the executor calls `PushNotification` before yielding control. The scaffold renders this rule into every plan's `## Execution policy`.

Full mechanics, retry exclusions, semi-autonomous checkpoint format, notification spec: see `references/execution.md`.

## Subagent trace / auditability

Each subagent leaves these durable traces:
- Git commits — one per stage; `git log` / `git diff` between commits
- Disk changes — file modifications persist
- Returned summary to the parent — surfaced to the user between stages
- Backlog status flips — versioned in `docs/backlog.md` or equivalent
- **Post-stage report** at `<repo>/docs/plans/<plan-slug>-stage-{N}-report.md` — **mandatory**. The only trace of surprises, deviations, and judgment calls that git alone does not capture; required so an autonomous run remains reviewable after the fact. Both plan and reports live under `docs/plans/` so they are versioned and reviewable in PRs.

**Report structure:** subagents copy `docs/plans/_report-template.md` (landed by the Plan landing commit) as a starting point. The template has a `Commit: _filled by parent_` slot — subagents leave it as-is. The parent fills the canonical `stage → SHA` mapping in the End-to-end summary table.

## Optional hardening (per-plan, not baked in)

- **Hooks for gate enforcement** (`.claude/settings.json` PostToolUse / PreCommit). Prompt-level gates can be ignored by a confused subagent; hooks cannot. Configure via `update-config`.
- **Reviewer gate** — auto-recommended when risk signals trigger; otherwise opt-in.
- **Accumulated run log**: a `Stop` hook appending each subagent's summary into one `<repo>/docs/plans/<plan-slug>-run.md`.

## Anti-patterns

- **Do NOT** ask the subagent to plan — pass a fully-formed, executable stage description.
- **Do NOT** let hand-off prompts reference "the previous conversation" or prior-stage internals — they must stand alone.
- **Do NOT** batch multiple stages into one subagent — contextual isolation is the entire point.
- **Do NOT** retry a red stage **unboundedly or with the same prompt** — retries are capped by the Execution policy and each retry must narrow the instruction.
- **Do NOT** use `git add -A` / `git add .` in hand-off prompts — always explicit paths.
- **Do NOT** rely on literal line numbers from the plan when writing stages N>=2 — instruct "grep for symbols, line numbers have drifted".
- **Do NOT name models in the plan markdown.** The plan declares `Tier:` (`mechanical | standard | judgment | critical`) and `Effort:` (`minimal | standard | extended`) per stage; the executor at runtime maps those to its own platform's models, picking the cheapest viable combo. Keeps the plan portable across Claude Code, Codex, and future executors without edits.
- **Do NOT classify every stage as `critical / extended` "to be safe".** Promotion has cost; demotion is free. The End-to-end summary table should show >40% of stages at `mechanical` or `standard` — if not, the decomposition is suspect.
- **Do NOT auto-promote a stage's model on retry.** If a `mechanical` stage fails twice, the classification was wrong — STOP and replan, do not silently escalate to a bigger model.
- **Do NOT** allow stages to spawn their own subagents — nested `Agent` calls defeat contextual isolation and the green-to-green audit.
- **Do NOT** prompt the user for a menu of execution policy choices — defaults are fixed; the only allowed planning question is the working-tree policy when `git status` is dirty.
- **Do NOT** let the reviewer gate replan or edit code — it returns a verdict only.
- **Do NOT** ship a plan with open questions for the user. If a question can't be resolved from the code, ask it once before scaffolding (single question, like the working-tree gate) and then scaffold with the answer baked in. The plan markdown is an executable contract, not a discussion document.
- **Do NOT** trust the backlog to enumerate call-sites of removed/renamed symbols — back the backlog with an explicit grep at plan time. Symbol-fallout discovered mid-stage means Phase 1 was incomplete.
