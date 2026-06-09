# Plan structure (full skeleton)

The plan must be **self-describing**: anyone opening the file in a fresh context window (without this skill loaded) should be able to execute it correctly. The `## Execution model` block is mandatory.

```
# <Track name> - Staged Execution Plan

## Execution model (READ FIRST)
Staged subagent execution (prompt chaining + gate checks). Do NOT run as one linear task.

0. **Pre-execution placeholder gate** (mandatory, before launching any stage). Run:
   ```
   python3 -c "import sys; sys.path.insert(0,'docs/plans'); from _verify import V; V.assert_no_placeholders('docs/plans/<plan-slug>.md'); sys.exit(V.summarize())"
   ```
   If non-zero, abort and surface the offending lines. Fix or delete the flagged blocks; do NOT bypass.
1. Read this plan end-to-end.
2. Run Stage 0 (Pre-flight). If any gate is red on the baseline, abort.
3. For each Stage N >= 1, launch a fresh subagent (see `## Executor adapter`):
   - prompt: the verbatim Hand-off prompt block for that stage
   - description: the stage title
   - foreground, sequential, inherit model
4. On return, verify: build + gates clean, commit SHA present in `git log`,
   post-stage report written, scope respected (only declared files touched).
5. Green -> Mode handling:
   - autonomous: launch Stage N+1 immediately.
   - semi-autonomous: post the post-stage summary + `Resume? [y / edit / abort]`
     and wait. `y` -> launch Stage N+1; `edit` -> user adjusts the next
     hand-off then `y`; `abort` -> stop (committed work is preserved).
   Red -> apply the `## Execution policy` retry rule.
6. After the final stage, run `## End-to-end verification`, run the
   `## Reviewer gate` if not `none`, and emit the
   stage -> SHA -> report-path summary table.

Parent responsibilities (not delegable): launching stages in order, verifying
green between stages, running end-to-end verification, running the reviewer
gate if configured, producing the summary.

Resuming after a red stage: each hand-off prompt only assumes prior commits
exist in `git log`, not that they came from subagents. If Stage K was fixed
manually, relaunch Stage K+1 unchanged. Never re-run committed stages.

## Execution policy (fixed defaults unless user overrode)
- Mode: autonomous           # or: semi-autonomous — between-stage checkpoint posted by parent: `✓ Stage N done — {sha} "{subject}" | Files: ... | Gates: build ✓ test ✓ ... | Report: <path> | Next: Stage N+1 — {title} | Resume? [y / edit / abort]`. `y` -> next; `edit` -> adjust next hand-off; `abort` -> stop.
- Commit authorization: per-stage-direct
- On red: auto-retry-up-to-2 — cap of 2 retries; each retry passes the prior failure excerpt and narrows the instruction to the same file list. NEVER retry on scope violations, pre-commit hook rejections, or hook bypass attempts (escalate immediately). On exhaustion: stop and surface.
- Working-tree policy: clean-required   # or: stash-authorized | integrate-existing | abort-until-clean — per-state behavior is described inline in `## Stage 0`.
- Reviewer: none                        # or: light | deep — see `## Reviewer gate`
- Report policy: committed              # or: gitignored — see scaffold `--report-policy`

## Executor adapter
- **Claude Code**: use the `Agent` tool, one stage per subagent,
  `subagent_type: general-purpose`, foreground, omit `model`, omit `run_in_background`.
- **Codex / other executors**: execute each Hand-off prompt inline in a fresh context
  window, or via the executor's own delegated-agent mechanism if available.
  The plan does not depend on Claude-specific tooling beyond this section.

## Hand-off conventions (apply to every stage)
Generic rules referenced by every stage's hand-off prompt. Centralised here so
each hand-off only declares its stage-specific scope.

**Authorization:** MAY commit after green; MAY NOT push, modify out-of-scope
files, touch unrelated dirty edits, skip gates, or spawn nested subagents.

**Scope discipline:** if the stage requires files outside its declared list,
STOP and report. Pre-existing unrelated failures: STOP and report.

**Failure protocol:** in-scope gate failure → fix and re-run. Any STOP →
return to parent with reason.

**Return to parent:** per-file summary (with grep'd locations), gate results,
commit SHA + subject, deviations, post-stage report path.

## Context
<Why this track. Constraints. Items in scope. Items out of scope / blocked externally.>

## Alternatives considered
<FILL-OR-DELETE: 1-2 stage decompositions rejected, with reason. Planner fills
if they considered multiple decompositions; deletes this block otherwise.
Runtime surprises are covered by hand-off "STOP and report" + reviewer gate —
do not pre-invent them here.>

## Global conventions
- Build gate: <cmd>
- Lint/test gates: <cmds>
- Invariants: <e.g. no GPL in main binary, vendor-neutral i18n, tracing only, English only>
- Commit style: **one commit per stage** that includes BOTH the code changes AND the post-stage report. The report (`<plan-slug>-stage-{N}-report.md`) is staged alongside the code files in the same commit — there is no separate "report commit". Trailer: `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` (substituted by the executor at commit time, e.g. `Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>`).
- Staging: only files the stage declares, plus the stage's own report file, by explicit path; never `git add -A`

## Stage 0 - Pre-flight (mandatory, no feature work, no commit, no versioned report)
Purpose: record baseline state and apply the working-tree policy so later
failures cannot be blamed on prior repo state. **Plan support artifacts
(`_verify.py`, verify scripts, the plan file itself) are already committed
via the Plan landing commit before Phase 2 began** — they are not vendored
or committed here.

**No versioned report:** Stage 0 must NOT write `<plan-slug>-stage-0-report.md`
under `docs/plans/`. That would leave the working tree dirty and conflict with
`clean-required`. Baseline evidence goes to the gitignored logs dir, and the
human-readable summary is returned to the parent (which surfaces it inline).

1. Capture `git status` and the current HEAD SHA. Write them to
   `<repo>/docs/plans/logs/<plan-slug>-stage-0-baseline.log` (gitignored via
   the Plan landing commit) and return the same summary to the parent.
2. Apply the working-tree policy from `## Execution policy`:
   - clean-required: tree must be clean; if not, abort.
   - stash-authorized: `git stash push -u -m "staged-plan-<slug>-pre"`; record stash ref in the log + parent summary.
   - integrate-existing: leave changes in place; list them in the log + parent summary; subagents
     must not stage files they did not modify.
   - abort-until-clean: abort the plan; user resolves manually.
3. Run every gate (build, lint, tests, i18n, etc.) on the resulting baseline. `run_gate()`
   already writes its own per-command log under `docs/plans/logs/`.
4. Red -> abort. Green -> working tree must still be clean (or match the
   integrate-existing manifest); proceed to Stage 1.

<!-- BEGIN STAGE 1 -->
## Stage 1 - <title>
**Tier:** standard         <!-- mechanical | standard | judgment | critical -->
**Effort:** standard       <!-- minimal | standard | extended -->
**Tier rationale:** <1-2 lines justifying tier/effort; what the executor uses to pick the cheapest viable model on its platform>
**Items:** <atomic IDs>
**Scope:** <one sentence>
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

**Files:**
- `<path>` - <what changes and why>

**Order of operations:**
1. ...
<last>. Gates pass -> commit.

**Verification:** <per-stage commands + expected outcomes>
<Generate `docs/plans/<plan-slug>-verify-stage-1.py` per the trigger rules in
`references/verify-scripts.md`.>

**Manual verification (if any):** <user-side, deferred if agent can't execute>

**Post-stage report:** write `<repo>/docs/plans/<plan-slug>-stage-1-report.md`.
Copy `docs/plans/_report-template.md` as the starting structure; the `Commit:`
slot stays as `_filled by parent_` — the End-to-end summary table is the
canonical source for the `stage → SHA` mapping.

**Hand-off prompt for Stage 1:**
> <SELF-CONTAINED prompt — see `references/handoff-template.md`. References
> `## Hand-off conventions` instead of duplicating Authorization/Scope/Failure/
> Return blocks. Stage's authoritative spec is the block between
> `<!-- BEGIN STAGE 1 -->` and `<!-- END STAGE 1 -->`.>

<!-- END STAGE 1 -->
---

<!-- BEGIN STAGE 2 -->
## Stage 2 - ...
<!-- END STAGE 2 -->

---

## Reviewer gate (only if Reviewer != none)
After the final stage commits green, the reviewer runs and emits a verdict
plus a findings list. If findings exist, an **arbiter** classifies them
(`must-fix` / `nice-to-have` / `human-judgment`) using a fixed decision
tree, a **fix-subagent** corrects only the `must-fix` items (max 1 round),
gates are re-run, and (conditionally) a re-review runs. The full outcome
is persisted to `docs/plans/reports/<plan-slug>_reviewer_<seq>.md` --
immutable per sequence, never overwritten.

Final verdict is one of:
`pass | pass-with-notes | pass-with-fixes | pass-with-pending | fail | blocked`

Neither the reviewer nor the arbiter edits code or replans. On
`fail`/`blocked`, the parent stops and surfaces the md file path. The
full sequence (arbiter prompt, fix-round rules, re-review trigger,
output file structure) is rendered into every plan whose
`Reviewer != none` -- see the `## Reviewer gate` block in the generated
plan for the operational contract.

## Critical files (cross-stage index)
<table of file -> stages that touch it>

## End-to-end verification (after final stage)
<commands + manual smoke. If >3 commands OR invariants to grep, generate
`docs/plans/<plan-slug>-verify-e2e.py` (Python 3 stdlib, importing _verify).>
```

## Stage sizing

- One logical deliverable per stage, 1-3 backlog items
- Order by blast radius: internal/pure-refactor first, public-API/UI last, external-blocked last
- Isolate externally-blocked work in its own trailing stage
- Target stage duration: 3-15 minutes of subagent wall time; if longer, split
- **Scope discipline over numeric budgets.** Token/tool-call counters are not exposed to the subagent at runtime, so numeric budgets cannot be mechanically enforced. Instead, rely on an **explicit file list per stage** and the anti-scope-expansion rule: the subagent must STOP and report if the stage appears to require files outside the list.
