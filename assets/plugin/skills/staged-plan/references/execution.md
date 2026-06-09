# Phase 1.5 (landing commit) and Phase 2 (execution)

## Phase 1.5 - Plan landing commit (mandatory, before Phase 2)

After `ExitPlanMode`, the **planner** (not a subagent) makes a single commit that lands the plan and its support artifacts. This is NOT feature work — it is plan setup, and isolating it here keeps Stage 0 and Stage 1+ scope-clean.

**Pre-check (mandatory) — `.gitignore` audit:** the landing commit assumes `<repo>/docs/plans/` is **trackable**. Inspect `.gitignore`:
- If it ignores `docs/plans/` wholesale (a `docs/plans/` line), **narrow the rule** to `docs/plans/logs/`. The plan file, `_verify.py`, and verify scripts MUST be versioned; only gate logs are excluded. Do NOT use `git add -f` to bypass — fix the rule.
- If it ignores report files specifically (e.g., `docs/plans/*-report.md`, `*-report.md`): **consistency check** — the plan must have been scaffolded with `--report-policy gitignored`. If it was scaffolded with the default (`committed`), STOP and re-scaffold with the correct flag (or remove the gitignore pattern if reports should be versioned). Mismatched policy will fail `clean-required` after the first stage.
- If report-policy is `gitignored` but `.gitignore` does NOT cover report files, **add the pattern** as part of this commit (e.g., append `docs/plans/*-report.md`).
- Otherwise, append `docs/plans/logs/` if not already present.

The landing commit contains:
1. `<repo>/docs/plans/<plan-slug>.md` — the filled plan file.
2. `<repo>/docs/plans/_verify.py` — vendored from `~/.claude/skills/staged-plan/lib/verify.py` if not already present.
3. Any `<repo>/docs/plans/<plan-slug>-verify-stage-N.py` and `<plan-slug>-verify-e2e.py` scripts the plan declares.
4. `<repo>/.gitignore` with the narrowed/appended rule from the pre-check.

Stage commits and post-stage reports come AFTER this commit; subagents don't touch these files.

Suggested subject: `chore(plans): land <plan-slug> staged plan + verify scripts`.

After the landing commit, working tree is clean and Phase 2 starts.

## Phase 2 - Execution (after Plan landing commit)

The Execution policy in the plan declares Mode, retry, working tree, and reviewer; the parent reads them and proceeds without further prompting (except for the semi-autonomous between-stage checkpoint).

### 0. Pre-execution gate (mandatory, before launching Stage 1)

The parent runs `assert_no_placeholders` against the plan file to refuse to execute a half-filled plan. This catches the failure mode where the planner forgot to replace a `<FILL: ...>` block, which would cause subagents to follow boilerplate instead of real instructions.

```bash
python3 -c "
import sys
sys.path.insert(0, 'docs/plans')
from _verify import V
V.assert_no_placeholders('docs/plans/<plan-slug>.md')
sys.exit(V.summarize())
"
```

If this exits non-zero, **abort and surface the offending lines** to the user — do NOT launch any stage. The fix is to fill or delete the flagged blocks, not to bypass the gate.

### 1. Launch each stage per `## Executor adapter` in the plan

- Claude Code: `Agent` tool, `subagent_type: general-purpose`, `model` selected per the Tier/Effort mapping in the plan's `## Executor adapter` (do NOT omit — omission inherits the parent's model and silently bypasses cost-tiering), `run_in_background` omitted, `description` = stage title, `prompt` = the Hand-off prompt (optionally appended with runtime context such as current branch state).
- Other executors: follow the adapter section.

### 2. On completion, verify green

Build passed, gates clean, commit SHA in `git log`, post-stage report written, scope respected.

- **autonomous**: green -> launch next stage immediately.
- **semi-autonomous**: green -> post a structured checkpoint and wait:
  ```
  ✓ Stage N done — {sha} "{subject}"
  Files: {path} ({+adds} {-dels}), ...
  Gates: build ✓ test ✓ ...
  Report: docs/plans/<slug>-stage-N-report.md
  Next: Stage N+1 — {title} ({k files})

  Resume? [y / edit / abort]
  ```
  - `y` -> launch Stage N+1 unchanged.
  - `edit` -> user adjusts the next stage's Hand-off (e.g., adds a callsite found in this stage's report) before launching.
  - `abort` -> stop. Committed work is preserved.
- Red (any mode) -> apply the retry rule.

### 3. Retry rule (auto-retry mode only)

- Up to **2 auto-retries** per stage.
- Each retry passes the prior run's failure excerpt back into the hand-off and narrows the instruction to "fix the reported failure only, within the same file list".
- **No retry on:** scope violations (subagent touched files outside the list), pre-commit hook rejections, or attempts to bypass hooks. These are escalated immediately.
- On exhaustion: stop, surface the failure chain to the user, wait. **Before yielding control, call `PushNotification`** (see §5).
- If the plan says "pause on first red", skip retries entirely.
- Scope violations and hook-bypass escalations also notify via `PushNotification` before yielding (§5).

### 4. After the final stage

- Run the end-to-end verification block.
- If `Reviewer: light` or `deep`, run the reviewer gate exactly as the plan describes it (reviewer -> arbiter -> fix round -> conditional re-review -> persist to `docs/plans/reports/<plan-slug>_reviewer_<seq>.md`). On `fail` / `blocked`, stop and surface the md file path; do NOT replan automatically and do NOT trigger another fix round. **Before yielding control, call `PushNotification`** (see §5).
- Emit the stage -> commit SHA -> status -> report-path table.
- List any externally-blocked items still open, with reopen criteria.
- If working-tree policy was `stash-authorized`, remind the user to `git stash pop` (or list the stash ref).

### 5. Human-interaction notifications

Whenever execution stops awaiting a human decision, the executor MUST call `PushNotification` BEFORE yielding control. The user may be away from the terminal; the notification is what brings them back.

**Notify on:**
- Reviewer verdict `fail` / `blocked` after the fix round + re-review.
- Retry rule exhausted (2 retries failed) for a stage.
- Scope violation or hook-bypass escalation (no retry path).
- Dirty-tree state encountered mid-run that requires a user choice.

**Do NOT notify on:**
- Normal between-stage transitions (autonomous mode).
- Semi-autonomous between-stage checkpoint — the user is already watching that mode by design.
- Successful end-to-end completion (no human action required).

**Message format:** `"Plan <slug>: <reason> — see <report-or-file path>"`. Keep under ~120 chars; the report file holds the detail.

Examples:
- `"Plan 008-outbound-reliability: reviewer BLOCKED (4 findings) — see docs/plans/reports/008-outbound-reliability_reviewer_001.md"`
- `"Plan migration-x: Stage 3 retry exhausted — see docs/plans/migration-x-stage-3-report.md"`
- `"Plan migration-x: Stage 2 scope violation (touched files outside list) — escalated, no retry"`

If `PushNotification` is unavailable in the executor's tool set, fall back to a clearly delimited `<<HUMAN_REQUIRED>>` block in stdout with the same message; an outer harness can grep for it.
