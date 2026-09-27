# Issue #485 — fix the lying tests, then cut the duplicates - Staged Execution Plan

<!-- scaffolded 2026-09-27 via staged-plan/lib/scaffold.py -->

## Execution model (READ FIRST)
Staged subagent execution (prompt chaining + gate checks). Do NOT run as one linear task.

0. **Pre-execution placeholder gate** (mandatory, before launching any stage). Run:
   ```
   python3 -c "import sys; sys.path.insert(0,'docs/plans'); from _verify import V; V.assert_no_placeholders('docs/plans/issue-485-test-suite-cleanup.md'); sys.exit(V.summarize())"
   ```
   If non-zero, abort and surface the offending lines. Fix or delete the flagged blocks; do NOT bypass.
1. **Parent** reads this plan end-to-end (orchestration needs the full picture).
   **Subagents** read only the sections their hand-off prompt names — never
   other stages' blocks. This split is a deliberate token optimization.
2. Run Stage 0 (Pre-flight). If any gate is red on the baseline, abort.
3. For each Stage N >= 1, launch a fresh subagent (see `## Executor adapter`):
   - prompt: the verbatim Hand-off prompt block for that stage
   - description: the stage title
   - foreground, sequential, `model` selected per Tier/Effort (see `## Executor adapter` mapping table)
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

### Resource selection vocabulary (read before launching each stage)

Each stage declares `Tier:` (cognitive load) and `Effort:` (reasoning budget).
The executor at runtime maps these to the cheapest viable resource on its
platform that meets BOTH dimensions. The plan does NOT name models — that is
the executor's responsibility (it knows its own lineup and pricing).

**Tier:**
- `mechanical` — literal execution of a well-specified hand-off (rename, move,
  apply pattern from list). Smallest model that can follow the instruction.
- `standard` — typical coding within the declared file list, light judgment.
- `judgment` — scope decisions, semantic synthesis, non-obvious refactors.
- `critical` — security, public contract, data migration, irreversible changes.

**Effort:**
- `minimal` — no extended reasoning; cheapest setting.
- `standard` — default reasoning budget.
- `extended` — maximum reasoning budget the executor offers.

**Selection rule:** pick the cheapest model × reasoning combo on your platform
that meets or exceeds the declared Tier and Effort. Do NOT auto-promote on
retry — if a `mechanical` stage fails twice, the classification was wrong;
STOP and replan rather than silently escalating to a bigger model.

**Role defaults** (apply when not overridden by a stage block):
- Parent / orchestrator: `standard / standard`
- Stage 0 (pre-flight gates): `mechanical / minimal`
- Reviewer gate: `critical / extended`
- Stage N >= 1: declared per stage; absence defaults to `standard / standard`

## Execution policy (fixed defaults unless user overrode)
- Mode: autonomous
- Commit authorization: per-stage-direct
- On red: auto-retry-up-to-2 — cap of 2 retries; each retry passes the prior failure excerpt and narrows the instruction to the same file list. NEVER retry on scope violations, pre-commit hook rejections, or hook bypass attempts (escalate immediately). On exhaustion: stop and surface.
- Working-tree policy: clean-required — per-state behavior is described inline in `## Stage 0`.
- Reviewer: deep  # 12 stages + security-guarantee tests + CI test layout changes
- Human-interaction stops: whenever execution halts awaiting a human decision (reviewer verdict `fail`/`blocked` after fix round; retry exhausted; scope violation / hook-bypass escalation; dirty-tree mid-run), the executor MUST call `PushNotification` BEFORE yielding control. Message format: `"Plan <slug>: <reason> — see <report-or-file path>"`. Do NOT notify on normal between-stage transitions or successful completion.

## Plan landing commit (mandatory before Phase 2)
Before launching Stage 1, the planner (NOT a subagent) makes a single commit
that lands this plan and its support artifacts. This is plan setup, not
feature work — isolating it here keeps Stage 0 and Stage 1+ scope-clean.

**Pre-check (mandatory):** before staging anything, inspect `C:\Dev\ralphy/.gitignore`.
The Plan landing commit assumes `docs/plans/` is **trackable**. Two cases:

- If `.gitignore` ignores `docs/plans/` wholesale (e.g. a `docs/plans/` line),
  **narrow the rule to ignore only logs**: replace that line with
  `docs/plans/logs/`. The plan file, `_verify.py`, and verify scripts MUST be
  versioned; only gate logs are excluded. Do NOT use `git add -f` to bypass —
  the rule itself needs fixing.
- If `.gitignore` does not ignore `docs/plans/`, just append `docs/plans/logs/`
  if not already present.

The landing commit MUST contain:
1. `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup.md` — this plan file.
2. `C:\Dev\ralphy/docs/plans/_verify.py` — vendored verify primitives; the planner
   copies this from the staged-plan skill source as part of Phase 1.5 if not
   already present in the repo. Stage scripts import it via
   `sys.path.insert(0, 'docs/plans'); from _verify import V`.
3. `C:\Dev\ralphy/docs/plans/_report-template.md` — scaffolded alongside the plan;
   subagents copy it as the starting structure for post-stage reports.
4. Any `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-verify-stage-N.py` and
   `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-verify-e2e.py` scripts the plan declares.
5. `C:\Dev\ralphy/.gitignore` with the narrowed/added rule from the pre-check above
   (plus the report-ignoring pattern when report-policy = `gitignored`).

Suggested subject:
`chore(plans): land issue-485-test-suite-cleanup staged plan + verify scripts`

After this commit, working tree is clean and Phase 2 starts.

## Logs policy
Gate execution logs are written to `C:\Dev\ralphy/docs/plans/logs/<prefix>-<ts>.log`
on every `run_gate()` call. They are **local evidence artifacts, not
versioned**: `docs/plans/logs/` is gitignored via the Plan landing commit.
Reports (committed alongside each stage) capture the deviations and
judgments needed for PR review; raw logs are kept locally for forensics.

## Executor adapter

Each stage runs in a fresh context window via whatever delegated-agent
mechanism the executor provides (Claude Code: `Agent` tool with
`subagent_type: general-purpose`, foreground, sequential; Codex / others: the
equivalent fresh-window mechanism, or inline in a clean session if no delegate
mechanism exists).

**Model & effort selection:** the plan declares `Tier:` and `Effort:` per stage
(see `## Execution model` § Resource selection vocabulary). The executor maps
those to its own model lineup, picking the cheapest viable combo. The plan
itself names no model — only the executor knows what's available and what it
costs.

**Mapping for Claude Code** (pass as `model` argument to the `Agent` tool):

| Tier / Effort | `model` |
|---|---|
| mechanical / minimal | `haiku` |
| mechanical / standard | `haiku` |
| standard / minimal | `sonnet` |
| standard / standard | `sonnet` |
| standard / extended | `sonnet` or `opus` |
| judgment / standard | `opus` |
| judgment / extended | `opus` |
| critical / * | `opus` |

Do NOT omit `model` (omission inherits the parent's model and defeats the
cost-tiering — every stage would silently run on the parent's model). For
Codex / other executors, apply the same vocabulary against their lineup.

Roles when no stage-level override is present:
- Parent / orchestrator: `standard / standard`
- Stage 0: `mechanical / minimal`
- Reviewer gate: `critical / extended`
- Stage N >= 1: as declared; default `standard / standard`

## Hand-off conventions (apply to every stage)

**Authorization:**
- MAY commit directly after all verifications pass.
- MAY NOT push.
- MAY NOT modify files outside the stage's declared file list.
- MAY NOT touch pre-existing unrelated working-tree edits.
- MAY NOT skip gates or use --no-verify / bypass hooks.
- MAY NOT spawn nested subagents (no Agent calls inside this stage).

**Scope discipline:**
- If the stage appears to require files outside the declared list, STOP and
  report. Do NOT silently expand scope.
- If pre-existing test/build failure is unrelated to this stage, STOP and
  report. Do NOT fix it.

**Failure protocol:**
- Gate fails within declared scope -> fix within scope and re-run the gate.
- Any STOP condition above -> return to parent with a clear reason.

**Return to parent:**
- Per-file summary with actual grep-found locations.
- Gate results (pass/fail + snippets).
- Commit SHA + subject.
- Deviations from the plan, if any.
- Path to the post-stage report written to disk.

## Context
Issue #485 carries out the test audit in `docs/audit-tests-2026-09-27.md` (commit 31d2f8e0). About 90
tests pass when the behavior they name is broken. 15 of them are P0, and several of the P0 tests
guard a security property. About 500 more tests repeat another test, test a library, or pin
incidental text. The report gives every finding an ID (P0-n, A-n, D-n, C-An, C-Bn, K-n, S-n, J-n,
G-n) with `file:line`, the evidence, and the production mutation that the test does not catch.
The issue sets the order and the method. This plan turns them into stages.

**Re-checked at HEAD `78628fe4` (2026-09-27).** Every P0 line number still holds (±1). Two facts
are new since the report:
- **P0-2 is wider than the report says.** Besides the 3 named tests, the same
  `split("#[cfg(test)]")` cut appears in about 15 more places: cursor `auth.rs`, `command/tests.rs`
  ×2, `model.rs`, `outcome/tests.rs` ×2; gemini `auth.rs`, `command/tests.rs` ×2,
  `outcome/tests.rs`, `revocation.rs` ×2, `root.rs`, `tasks.rs` ×3; copilot `outcome.rs`. Stage 3
  fixes all of them.
- **Some P0 tests cannot call production without a production change**, and this issue forbids
  production changes. The maintainer decided (2026-09-27):
  - P0-11 and P0-13 become pins that match the **call**, inside a slice of the calling function's
    body.
  - The lying tests of P0-10, P0-14 and P0-15 are deleted.
  - The doc comment of P0-3 is reworded to say what the test proves.
  - K-3 uses `#[ignore]` + `--ignored`. It cannot move to `src/bin`, because the child uses
    private functions and a `#[cfg(test)]`-only marker.
  - P0-8 and P0-9 stay Rust pins that match the exact call. The node harness has no recording DOM
    and no fake xterm.
  - Every production seam these tests need goes into one follow-up issue draft,
    `docs/plans/issue-485-followup-draft.md`. The maintainer publishes it.

**In scope:** issue phases 0–5 and 7, as stages 1–12.
**Out of scope:** issue phase 6 (re-audit of the areas the report did not read) is a new audit,
not an executable change. It goes into the follow-up draft. The issue's own "Out of scope" list
also stays out: the skills-dance refactor, the `run_hook` seam, retiring `xtask asset-pins`, the
policy on source-grep lints, structural findings, the pty tree kill, the Playwright scripts, and
any new framework or mutation tool.
**Hard constraint:** no production code changes, except test children under `src/bin/`. A test
module inside a production file (`#[cfg(test)] mod tests { … }`) is test code. So are `tests.rs`
files, `tests/`, `ui-tests/`, and `ralphy-daemon/tests/*.js`.

## Global conventions
- Build gate + lint/test gates: `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py` runs the CI gate from AGENTS.md:
  `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo nextest run --workspace`, `cargo test --workspace --doc`, and
  `cargo run -q -p xtask -- changelog --check`. Add `--ui` when the stage touches
  `crates/ralphy-daemon/assets/ui/`, `ui-tests/`, or `ralphy-daemon/tests/*.js`. It adds
  `node --test crates/ralphy-daemon/ui-tests` and `cargo run -q -p xtask -- ui-copy --check`. Add
  `--ids ID1,ID2,…` to check that each ID has a row in §7 of the report.
- **Method (the rules of issue #485; they are not optional):**
  1. *Fix a lying test (LIES-FP).* Apply the report's mutation to production. Run the test. It
     must PASS, which proves the lie. If it FAILS, the finding is wrong: mark the ID `rejected` in
     §7 with the reason, and do not change the test. Change only the test until it FAILS against
     the mutation. Revert the mutation. The test PASSES.
  2. *Delete a test.* Name the covering test. If the deleted test aimed at a mutation, apply that
     mutation and show that the covering test FAILS. A test of std, serde, clap, uuid, a
     constant, or prose needs no mutation; name its category instead.
  3. *Merge into a table.* Cases before = rows after. Each row names its case in the assert
     message. Apply one mutation per merged group and show that the table FAILS.
  4. *Loosen (LIES-FN).* Keep the behavior assertion and drop the incidental text. Show that the
     harmless edit named in the report now PASSES, and that one real mutation still FAILS.
  5. *Add a test for a gap (G-n).* Show that it FAILS against a mutation of the existing behavior.
     Then revert the mutation.
  6. *Merge integration binaries.* No test body changes, and the per-test assertion count stays
     the same. Use the existing patterns: identical env values
     (`ralphy-daemon/tests/changes_nudge.rs:20-22`) or `static Once`
     (`ralphy-daemon/tests/session_single_writer.rs:106-115`).
- **Evidence record.** For every rule 1/3/4/5 action, write one line in the commit message body
  **and** in the stage report:
  `<ID>: mutation <file:line, what changed> | before fix: PASS | after fix: FAIL | reverted: PASS`.
  Target a single test with `cargo nextest run -p <crate> -E 'test(<name>)'` or
  `node --test <file>`. Never run the full gate with a mutation applied.
- **Mutation hygiene.** Revert every mutation before the gate runs. Before commit,
  `git diff --stat` shows only the declared test files. A production file may appear only when
  the change is inside its `#[cfg(test)]` module, and the report must say so.
- **§7 of the report is the status ledger.** Each stage appends one row per ID it handles to the
  `| ID | Status | Commit | Note |` table in `docs/audit-tests-2026-09-27.md` §7. Status is one of: fixed, deleted,
  merged, loosened, rejected, kept, follow-up. Commit = `stage N`; the parent replaces it with
  the SHA at the end. A finding without an ID (a DELETE or MERGE row) gets a row keyed by section
  and test name, for example `§3.1 dyn_agent ×7`.
- **Invariants:** the public crate API is unchanged. Tests stay next to their code. An inline
  `#[cfg(test)] mod tests` block that grows past 500 lines moves to `foo/tests.rs` in the same
  stage (ADR-0022; `xtask` enforces it). Comments state invariants, not history. User text cites
  no ADR. English only, plain words, no idioms.
- **Commits:** one per stage, except Stages 6 and 7, which make one commit per crate (the issue
  asks for it). There, the stage report goes into the last commit. Subject style:
  `test(<crate>): <what> (#485)`. No branch, no push. Stages 8 to 10 may also commit per crate.
  So the `git log --oneline -K` counts in the hand-off prompts are not exact. Use
  `git log --oneline 78628fe4..HEAD` to see the prior work, and the stage reports to tell which
  commits belong to which stage.
- Commit style: ONE commit per stage that includes BOTH the code changes AND
  the post-stage report file. The report is staged alongside code; there is
  no separate "report commit". Trailer:
  `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL`
  (substituted by the executor at commit time, e.g.
  `Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>`).
- Report content: do NOT include the stage's own commit SHA in the report
  body (impossible: the file is part of the commit). The parent emits the
  canonical stage->SHA mapping in the End-to-end summary table.
- Staging: only files the stage declares PLUS the stage's own
  `issue-485-test-suite-cleanup-stage-{N}-report.md`, by explicit path; never `git add -A`.

## Stage 0 - Pre-flight (mandatory, no feature work, no commit, no versioned report)
**Tier:** mechanical
**Effort:** minimal
Purpose: record baseline state and apply the working-tree policy so later
failures cannot be blamed on prior repo state. Plan support artifacts
(`_verify.py`, verify scripts, the plan file) are already committed via the
Plan landing commit before Phase 2 began.

**No versioned report:** Stage 0 must NOT write `issue-485-test-suite-cleanup-stage-0-report.md`
under `docs/plans/` — that would leave the working tree dirty and conflict
with `clean-required`. Baseline evidence goes to the gitignored logs dir;
the human-readable summary is returned to the parent.

1. Capture `git status` and the current HEAD SHA. Write them to
   `C:\Dev\ralphy/docs/plans/logs/issue-485-test-suite-cleanup-stage-0-baseline.log` (gitignored) and
   return the same summary to the parent.
2. Apply the working-tree policy from `## Execution policy`:
   - clean-required: tree must be clean; if not, abort.
   - stash-authorized: `git stash push -u -m "staged-plan-issue-485-test-suite-cleanup-pre"`; record stash ref in the log + parent summary.
   - integrate-existing: leave changes in place; list them in the log + parent summary.
   - abort-until-clean: abort the plan; user resolves manually.
3. Run every gate (build, lint, tests, etc.) on the resulting baseline.
   `run_gate()` already writes its own per-command log under `docs/plans/logs/`.
4. Red -> abort. Green -> working tree must still be clean (or match the
   integrate-existing manifest); proceed to Stage 1.

<!-- BEGIN STAGE 1 -->
## Stage 1 - Baseline measurements
<!-- STAGE 1: tier-effort -->
**Tier:** standard
**Effort:** minimal
<!-- STAGE 1: tier-rationale -->
**Tier rationale:** Runs measurements and writes numbers into one table. There is no design choice, but reading nextest and node output needs care.
<!-- STAGE 1: items -->
**Items:** Phase 0 baseline
<!-- STAGE 1: scope -->
**Scope:** Record the suite's size and speed before any test changes, in §7 of the audit report.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 1: files -->
**Files:**
- `docs/audit-tests-2026-09-27.md` - fill the **Before** column of the §7 measurement table.

<!-- STAGE 1: order -->
**Order of operations:**
1. Build once so compile time is not measured: `cargo nextest run --workspace --no-run`.
2. Run `cargo nextest run --workspace` 3 times and record the wall time of each run (nextest prints it on the summary line). Record the median.
3. Rust test count: `cargo nextest list --workspace --message-format oneline | wc -l`.
4. Rust test binaries: `cargo nextest list --workspace --list-type binaries-only --message-format oneline | wc -l`.
5. `node --test crates/ralphy-daemon/ui-tests` — record the pass count and the duration from its summary.
6. Write the four values into the Before column. Add one sentence under the table: the host, the date, and the commit measured (HEAD).
7. Run `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py`.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 1: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py` passes. `git diff --stat HEAD~1` shows only the report file and the stage report.

<!-- STAGE 1: manual -->
**Manual verification (if any):** none

<!-- STAGE 1: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-1-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 1: handoff -->
**Hand-off prompt for Stage 1:**
> You are executing Stage 1 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 1 -->` and `<!-- END STAGE 1 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: this is the first feature stage; no prior stage commits exist beyond Stage 0 baseline.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 1 only - Baseline measurements. Items: Phase 0 baseline. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 1 -->
---

<!-- BEGIN STAGE 2 -->
## Stage 2 - P0 security and watch tests
<!-- STAGE 2: tier-effort -->
**Tier:** critical
**Effort:** extended
<!-- STAGE 2: tier-rationale -->
**Tier rationale:** These tests guard path confinement, the token strip, and the operator's own opt-out file. A wrong fix gives false security. Each fix needs a red run against the named mutation.
<!-- STAGE 2: items -->
**Items:** P0-1, P0-3, P0-4, P0-5, P0-6
<!-- STAGE 2: scope -->
**Scope:** Make the P0 security and watch tests able to fail, following report §2 rows P0-1 and P0-3 to P0-6.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 2: files -->
**Files:**
- `crates/ralphy-daemon/src/tree.rs` (test module only) - P0-1: `read_masks_escape_as_not_found` and `read_image_masks_escape_as_not_found`. Build `outer = tempdir()` and `root = outer/root`. Write a real `outer/secret` (text) and a real `outer/secret.png` (valid bytes from the module's `magic(ImageType::Png)`). Read `../secret` and `../secret.png` from `root`. Expect `NotFound`.
- `crates/ralphy-daemon/src/note.rs` (test module only) - P0-1: `read_masks_a_missing_or_escaping_target_as_a_miss`. Put a valid note (the module's `encode`) at `outer/outside.note`.
- `crates/ralphy-daemon/tests/observe_read.rs` - P0-1: `serve_repo` registers the tempdir itself as the root, and its parent is the shared system temp directory. Add a variant (or change `serve_repo` for every caller) that registers `outer/repo`, writes `outer/secret` and `outer/secret.png`, and returns the root. `image_read_masks_traversal_as_not_found` and `file_read_masks_traversal_as_not_found` use it.
- `crates/ralphy-daemon/tests/child_env_hygiene.rs` - P0-3: reword the doc comment. The test proves that a child dispatched after `strip_token_from_env()` does not inherit `RALPHY_DAEMON_TOKEN`. It does **not** prove that the daemon's boot calls the strip. Status `follow-up`: the boot seam goes into the draft in Stage 5.
- `crates/ralphy-proc-util/src/cursor.rs` (test module only) - P0-4: `the_gate_writes_nothing_when_already_protected`, and the "inner untouched" assert in `indexing_gate_creates_the_optout_in_every_enclosing_repository`. Seed an operator body that differs from the gate's `"*\n"` (for example `"# operator\nsecrets/\n"`) and assert the bytes are unchanged afterwards.
- `crates/xtask/tests/user_text_cites_no_adr.rs` - P0-5: add a unit test of `production()` in both directions. (a) A text with an item-level `#[cfg(test)] fn helper` **before** the production code, and a `#[cfg(test)]\nmod tests {` at the end: the cut keeps the code after the helper and drops the test module. (b) A text with no test module: the result is the whole text.
- `crates/ralphy-daemon/tests/runs_watch.rs` - P0-6: after the negative window in `runs_unwatch_stops_the_pushes`, add a positive control on the same socket. Re-send `runs.watch`, write a third snapshot, and assert that `recv_verb` returns `Some(..)`. Copy the pattern from `tree_watch.rs` `malformed_checkout_on_watch_holds_nothing` (the "POSITIVE CONTROL" block).
- `docs/audit-tests-2026-09-27.md` - §7 rows for P0-1, P0-3, P0-4, P0-5, P0-6.

<!-- STAGE 2: order -->
**Order of operations:**
1. P0-1 red runs: in `tree.rs`, replace `confine::confine(root, rel)` with `Ok(root.join(rel))` at both sites (`read` and `read_image`; grep `confine::confine`). The current tests must PASS. Fix them. They must FAIL. Revert. Do the same for `note.rs` (its `read` calls `confine::confine`), then for the two `observe_read.rs` tests (they go through the same `tree.rs` sites).
2. P0-4 red run: remove `.filter(|r| !r.join(OPT_OUT_FILE).exists())` in `cursor.rs` `indexing_gate`. The current tests PASS. Fix them. They FAIL. Revert.
3. P0-5 red run: first replace the body of `production()` with `""`, then with "cut at the first `#[cfg(test)]` line". The new test must FAIL for both. Revert.
4. P0-6 red run: find the `runs.unwatch` handler in `crates/ralphy-daemon/src` (grep `runs.unwatch`). Make it close the socket or return an error instead of releasing. Before the fix the test PASSES; after the fix it FAILS. Revert.
5. P0-3: reword only. No mutation, because the test cannot reach the boot path without a production change. The §7 note says so.
6. Write the §7 rows. Run `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-1,P0-3,P0-4,P0-5,P0-6`.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 2: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-1,P0-3,P0-4,P0-5,P0-6` passes. The commit body has an evidence line for P0-1 (tree, image, note, observe_read), P0-4, P0-5 (both mutations) and P0-6. `git diff --stat HEAD~1 -- crates/` lists only the files above.

<!-- STAGE 2: manual -->
**Manual verification (if any):** none

<!-- STAGE 2: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-2-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 2: handoff -->
**Hand-off prompt for Stage 2:**
> You are executing Stage 2 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 2 -->` and `<!-- END STAGE 2 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..1 committed (confirm with `git log --oneline -1`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -1` and `git diff HEAD~1 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 2 only - P0 security and watch tests. Items: P0-1, P0-3, P0-4, P0-5, P0-6. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 2 -->
---

<!-- BEGIN STAGE 3 -->
## Stage 3 - P0 source-cut scans
<!-- STAGE 3: tier-effort -->
**Tier:** judgment
**Effort:** extended
<!-- STAGE 3: tier-rationale -->
**Tier rationale:** Every source-text scan in three adapter crates changes, and the executor decides per site whether it was blind. Each ban or pin must keep its meaning after the new cut.
<!-- STAGE 3: items -->
**Items:** P0-2, plus the P0-2 sibling sites
<!-- STAGE 3: scope -->
**Scope:** Make every `split("#[cfg(test)]")` source scan in the gemini, cursor and copilot crates cut at the test module, not at the first `#[cfg(test)]` item.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 3: files -->
**Files:**
- `crates/ralphy-agent-gemini/src/tests.rs`, `crates/ralphy-agent-cursor/src/tests.rs`, `crates/ralphy-agent-copilot/src/tests.rs` - add one `pub(crate) fn production_text(src: &str) -> &str` per crate. It returns the text before the first `#[cfg(test)]` line whose next non-empty line starts with `mod `. This is the rule of `crates/xtask/tests/user_text_cites_no_adr.rs` `production()`. Give it one unit test in each direction. These files are already the crate's test module (`#[cfg(test)] mod tests;` in `lib.rs`), so other test modules call `crate::tests::production_text`.
- gemini: `src/auth.rs` (test `the_auth_probe_reads_no_credential`, and the second cut at about :202), `src/command/tests.rs` (`autonomy_argv_is_never_downgraded`, and cuts at about :248 and :409), `src/outcome/tests.rs` (:148), `src/revocation.rs` (:492, :529), `src/root.rs` (:486), `src/tasks.rs` (:525, :579, :662) - replace each `.split("#[cfg(test)]").next()` with `crate::tests::production_text(..)`.
- cursor: `src/tests.rs` (`no_adapter_side_retry_of_a_quota_stop`), `src/auth.rs` (:201), `src/command/tests.rs` (:226, :520), `src/model.rs` (:166), `src/outcome/tests.rs` (:493, :593) - the same replacement.
- copilot: `src/outcome.rs` (:321) - the same replacement.
- `docs/audit-tests-2026-09-27.md` - §7 row for P0-2, and one row `P0-2 siblings` listing every site, with a "was blind: yes/no" note.

<!-- STAGE 3: order -->
**Order of operations:**
1. Grep the three crates for `split("#[cfg(test)]")` and `#[cfg(test)]`. For each file that a scan reads, record whether it has an item-level `#[cfg(test)]` **before** its test module. Those scans are blind today. gemini `lib.rs` (`fn issue_deadline`, about :163) and cursor `lib.rs` (about :182) are the known cases.
2. Red runs on today's code, for the named P0-2 tests:
   - gemini `plan`: add `let _creds = std::path::Path::new("oauth_creds.json");` inside `fn plan` in `lib.rs`. `the_auth_probe_reads_no_credential` PASSES (the lie).
   - gemini `execute`: add the argv words `--approval-mode`, `auto_edit` inside `fn execute`. `autonomy_argv_is_never_downgraded` PASSES.
   - cursor: wrap the quota-stop handling in `lib.rs` (the file that calls `cursor_limit_note(`) in a `loop { … break; }`. `no_adapter_side_retry_of_a_quota_stop` PASSES.
3. Add `production_text` and its unit tests, and replace every cut site.
4. Re-apply the three mutations. Each named test now FAILS. Revert.
5. For each sibling site whose file was blind (step 1), apply one mutation after the early `#[cfg(test)]` item that the scan bans. Show that it FAILS now. For a site that was not blind, write "not blind; cut hardened" in §7. It needs no mutation.
6. Confirm that every existing scan still PASSES on the clean tree. A scan that now fails has found a real hit in text it never saw before. STOP and report it; do not weaken the scan.
7. §7 rows. Run the gate.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 3: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-2` passes. The commit body has the three P0-2 evidence lines, plus one per blind sibling site. `grep -rn 'split("#\[cfg(test)\]")' crates/ralphy-agent-gemini crates/ralphy-agent-cursor crates/ralphy-agent-copilot` returns no match.

<!-- STAGE 3: manual -->
**Manual verification (if any):** none

<!-- STAGE 3: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-3-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 3: handoff -->
**Hand-off prompt for Stage 3:**
> You are executing Stage 3 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 3 -->` and `<!-- END STAGE 3 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..2 committed (confirm with `git log --oneline -2`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -2` and `git diff HEAD~2 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 3 only - P0 source-cut scans. Items: P0-2, plus the P0-2 sibling sites. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 3 -->
---

<!-- BEGIN STAGE 4 -->
## Stage 4 - P0 UI wiring pins
<!-- STAGE 4: tier-effort -->
**Tier:** judgment
**Effort:** standard
<!-- STAGE 4: tier-rationale -->
**Tier rationale:** The executor chooses, per P0-7 pin, between a behavior test through `loadShell()` and a pin that matches the call. Both P0-8 and P0-9 need a pin that fails on the exact mutation without matching the definition.
<!-- STAGE 4: items -->
**Items:** P0-7, P0-8, P0-9
<!-- STAGE 4: scope -->
**Scope:** Make the UI wiring tests fail when the call is removed, not only when the definition is removed.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 4: files -->
**Files:**
- `crates/ralphy-daemon/ui-tests/app.test.mjs` - P0-7 behavior tests through `loadShell()` from `harness.mjs`:
  (a) `toggle(ref, row)` on the open slug calls `wakePeerFor(ref)` (the call is about `app.js:3461`). Record it with a stub `state.wakePeerFor`.
  (b) `toggle` mounts the changes subscription: set `state.$nextTick = fn => fn()` and record `mountChangesSub` (the call is about `app.js:3484`).
  (c) `emitCreate(node, kind)` emits `path: this.createDir(node)` — spy on `WB.emit`.
  (d) the `create` workbench action calls `askPrompt` — capture the `workbench:action` listener through `opts.document.addEventListener`, as `wb-encoding.test.mjs` does.
  If one case cannot run under the harness stubs, pin the exact call text instead (`this.wakePeerFor(ref)`, `this.mountChangesSub();`, `path: this.createDir(node)`, `await c.askPrompt({`), and give the reason in the report.
- `crates/ralphy-daemon/src/tests.rs` - P0-7: in `the_peer_wake_is_wired…`, `the_run_completion_nudge_is_wired…`, `the_explorer_can_create…` and `naming_a_new_entry…`, remove the definition needles (`wakePeerFor(ref) {`, `createDir(node) {`, `askPrompt(opts = {}) {`, `function X(`) that the node tests now cover. Keep the markup, CSS and index.html needles. P0-8: in `the_destructive_console_clicks_confirm_first`, pin each confirm site's action on the answer: `if (ok) arrangeFence(f.id)`, `if (ok) removeFence(f.id)`, and a `if (!ok) return;` that follows each `askConfirm({` with the titles `Restart in`, `Restart session?`, and both `Close this console?`. A site-local slice (from the title to the next `askConfirm` or the end of the function) is better than a whole-file `contains`. P0-9: in `the_console_clipboard_is_write_only…`, replace the `"replaying = false;"` needle with a whitespace-normalized pin of the write callback (`replaying ? () => { replaying = false; } : undefined`). The declaration `let replaying = false;` must not satisfy it.
- `docs/audit-tests-2026-09-27.md` - §7 rows for P0-7, P0-8, P0-9.

<!-- STAGE 4: order -->
**Order of operations:**
1. P0-7 red: delete the `wakePeerFor` call in `app.js` `toggle`. The current Rust test PASSES. Write test (a); it FAILS. Revert. Repeat with the `destroyChangesSub(); mountChangesSub();` line for (b), the `path: this.createDir(node)` argument for (c) (replace it with `path: ""`), and the `askPrompt` call for (d).
2. P0-8 red: `if (ok) arrangeFence(f.id)` → `arrangeFence(f.id)` in `wb-console.js`. The current pin PASSES; the new pin FAILS. Revert. Do one more for a close-button `if (!ok) return;`.
3. P0-9 red: the report's mutation — the write callback becomes `term.write(a.subarray(9)); replaying = false;`. The current pin PASSES; the new pin FAILS. Revert.
4. Trim the Rust needles as listed.
5. §7 rows. Run the gate with `--ui`.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 4: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids P0-7,P0-8,P0-9` passes. The commit body has the evidence lines. `git diff --stat HEAD~1 -- crates/ralphy-daemon/assets` is empty.

<!-- STAGE 4: manual -->
**Manual verification (if any):** none

<!-- STAGE 4: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-4-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 4: handoff -->
**Hand-off prompt for Stage 4:**
> You are executing Stage 4 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 4 -->` and `<!-- END STAGE 4 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..3 committed (confirm with `git log --oneline -3`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -3` and `git diff HEAD~3 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 4 only - P0 UI wiring pins. Items: P0-7, P0-8, P0-9. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 4 -->
---

<!-- BEGIN STAGE 5 -->
## Stage 5 - P0 Rust wiring tests and K-3
<!-- STAGE 5: tier-effort -->
**Tier:** judgment
**Effort:** extended
<!-- STAGE 5: tier-rationale -->
**Tier rationale:** For each wiring test, the executor decides between delete, a body-slice call pin, and an integration test. The executor also writes the follow-up draft that the maintainer will publish.
<!-- STAGE 5: items -->
**Items:** P0-10, P0-11, P0-12, P0-13, P0-14, P0-15, K-3
<!-- STAGE 5: scope -->
**Scope:** Stop the Rust wiring tests from testing a copy of the wiring, as the maintainer decided (pin the call, delete, or integration-test), and draft the follow-up issue for the production seams.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 5: files -->
**Files:**
- `crates/ralphy-agent-claude/src/headless.rs` (test module) - P0-10: delete `loop_exhaustion_yields_maxcalls` and `maxcalls_outcome_is_stuck`. Also delete `run_headless_steps` if nothing else uses it. If another test uses the pure `headless_reason_to_outcome(HeadlessReason::MaxCalls)` mapping, keep that one as the Stuck mapping test. Status `deleted` + `follow-up` (the loop has no injectable step source).
- `crates/ralphy-agent-copilot/src/tests.rs` - P0-11: add a body-slice pin. Slice `lib.rs` (`include_str!`) into the body of `fn plan(` and the body of `fn execute(`. Assert that the plan body contains `self.phase_model(Phase::Plan)` and `self.phase_effort(Phase::Plan)` and no `Phase::Execute`, and the reverse for execute. Keep the five argv tests. Rename them where the name claims the phase wiring (for example `plan_phase_uses_plan_model_in_argv` → `the_builder_puts_the_given_model_in_argv`), because they test `build_copilot_command`.
- `crates/ralphy-cli/tests/mutate.rs` - P0-12: add `config_set_refuses_under_held_lock` and `config_unset_refuses_under_held_lock`. They use the existing pattern: spawn `CARGO_BIN_EXE_runlock_test_child`, write `.ralphy/run.lock` with its pid, run `CARGO_BIN_EXE_ralphy config set <key> <value> --repo <tmp git repo>`. Assert that it fails and that the settings file is unchanged. Grep `hold_run_lock` in `tests/` for the helper.
- `crates/ralphy-cli/src/config/tests.rs` - P0-12: delete the unit test that calls `guard_run_lock` directly.
- `crates/ralphy-cli/src/run/wiring/tests.rs` - P0-13: add a body-slice pin of `fn preflight_agents` in `wiring.rs` that requires `CliAgent::Cursor => ralphy_agent_cursor::locate_cursor()`. Rename the existing test to what it proves (`check_agents_present_uses_the_given_locator`).
- `crates/ralphy-cli/src/schedule/spec.rs` (test module) - P0-14: delete `timer_spec_run_with_triage_chains_triage_first`. If `triage_prelude()`'s own text is not tested elsewhere, keep an assertion on `triage_prelude()` only, named for what it proves.
- `crates/ralphy-cli/src/init/run.rs` (test module) - P0-15: delete `init_git_safety_branch_and_scaffold_end_to_end`. If it is the only test of `write_scaffold` or of the `commit_decision`/`branch_decision` pair, keep those asserts as small unit tests without the "end to end" claim.
- `crates/ralphy-core/src/model_recovery.rs` (test module) - K-3: mark `locked_merge_child` `#[ignore]` and add `"--ignored"` to the parent's spawn args in `separate_process_transactions_are_serialized`.
- `docs/plans/issue-485-followup-draft.md` - new. Title, context, and one section per production seam: the boot token strip (P0-3), the headless loop step source (P0-10), the copilot phase args (P0-11), the cursor locator in `preflight_agents` (P0-13), the schedule `install` host call (P0-14), the init git stage (P0-15), and the wb-console confirm and OSC gate testability (P0-8, P0-9). Each section names the test that becomes possible. Leave a heading `## Also out of scope in #485` for Stage 12 to fill. English, plain words.
- `docs/audit-tests-2026-09-27.md` - §7 rows.

<!-- STAGE 5: order -->
**Order of operations:**
1. P0-11 red: `lib.rs` `self.phase_model(Phase::Plan)` → `Phase::Execute` in `fn plan`. The current tests PASS. Add the pin; it FAILS. Revert.
2. P0-12 red: delete the `guard_run_lock` call for `"config set"` in `config.rs` `run()`. The old unit test PASSES; the new integration test FAILS. Do the same for `"config unset"`. Revert. Delete the unit test.
3. P0-13 red: change the Cursor arm in `preflight_agents` to use the same probe as the other agents. The old test PASSES; the new pin FAILS. Revert.
4. P0-10, P0-14, P0-15: deletions with the category "tests a copy of production". No mutation, because the covering test does not exist; the follow-up draft owns it.
5. K-3: run `cargo nextest run -p ralphy-core -E 'test(model_recovery)'`. The parent PASSES and the child is listed as skipped. Then break the child (make it return early always). The parent must FAIL, which proves that `--ignored` really runs it. Revert.
6. Write the follow-up draft and the §7 rows. Run the gate.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 5: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids P0-10,P0-11,P0-12,P0-13,P0-14,P0-15,K-3` passes. `docs/plans/issue-485-followup-draft.md` exists. The commit body has evidence lines for P0-11, P0-12, P0-13 and K-3.

<!-- STAGE 5: manual -->
**Manual verification (if any):** none

<!-- STAGE 5: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-5-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 5: handoff -->
**Hand-off prompt for Stage 5:**
> You are executing Stage 5 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 5 -->` and `<!-- END STAGE 5 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..4 committed (confirm with `git log --oneline -4`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -4` and `git diff HEAD~4 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 5 only - P0 Rust wiring tests and K-3. Items: P0-10, P0-11, P0-12, P0-13, P0-14, P0-15, K-3. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 5 -->
---

<!-- BEGIN STAGE 6 -->
## Stage 6 - Deletions in the adapters
<!-- STAGE 6: tier-effort -->
**Tier:** standard
**Effort:** standard
<!-- STAGE 6: tier-rationale -->
**Tier rationale:** Mostly deletions from a list, each with a named covering test. The one judgment is which classify cases are vendor-specific and stay.
<!-- STAGE 6: items -->
**Items:** §3.1 DELETE rows, cross-adapter duplicates (§3.1 table rows 1–3 and `plan_pointer`), A-2, A-3, A-4, A-6
<!-- STAGE 6: scope -->
**Scope:** Delete the adapter tests that the report lists as duplicates or tests of a library, a constant, or prose, and the cross-adapter copies of shared behavior.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 6: files -->
**Files:**
- `crates/ralphy-adapter-support/src/**`, `crates/ralphy-agent-{claude,codex,kimi,opencode,gemini,cursor,copilot}/src/**` test code - the §3.1 **DELETE** list (dyn_agent ×7, mint_session_id ×3, the charter-size tests, the ADR-prose tests, serde/Default, constants, copilot `usage.rs` duplicates, and the named single tests). Also the LIES-FP rows marked delete: A-2 (×3), A-3, A-4 (×3), A-6.
- Cross-adapter table: delete the 13 `*_honours_max_minutes_per_issue` / `*_zero_minutes_disables…` tests (covered by `ralphy-adapter-support/src/budget.rs` tests). Delete the pass-through `classify_*` tests (covered by `classify.rs` tests), but **keep** the vendor-specific cases: non-zero exit → Stuck, vendor limits, kimi exit 75, opencode error event. Delete the 8 `prompt_plan_*_carries_finalize_trailer` asserts if `ralphy-core/tests/prompt_assembly.rs` checks the trailer in every artifact; otherwise keep exactly one. Delete the `plan_pointer…` / `PLAN_CHARTER.len()*50` copies (covered by `sentinel.rs` `len() < 512`).
- `docs/audit-tests-2026-09-27.md` - §7 rows.

<!-- STAGE 6: order -->
**Order of operations:**
1. Per crate, grep each test name (line numbers have drifted) and confirm that the covering test named by the report exists and runs.
2. Rule 2 red runs, one per group:
   - `budget.rs` `deadline` returns `None` always → the budget tests FAIL.
   - `classify.rs` drops `&& !s.errored` or swaps Done and Blocked → the ladder tests FAIL (if `errored` has no case, record G-1 for Stage 12).
   - Delete the trailer from `assets/prompts/plan/template.md` → `prompt_assembly.rs` FAILS.
   - For A-2, the covering test is Stage 12 G-2. Record that in §7.
3. Delete. Commit once per crate (`test(<crate>): delete duplicate tests (#485)`), with the category or covering test listed per deleted test in the body.
4. §7 rows (grouped rows are fine). The gate runs before each commit.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 6: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ids A-2,A-3,A-4,A-6` passes before each commit. `cargo nextest list --workspace --message-format oneline | wc -l` is lower than the Stage 1 count; record the delta in the report.

<!-- STAGE 6: manual -->
**Manual verification (if any):** none

<!-- STAGE 6: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-6-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 6: handoff -->
**Hand-off prompt for Stage 6:**
> You are executing Stage 6 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 6 -->` and `<!-- END STAGE 6 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..5 committed (confirm with `git log --oneline -5`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -5` and `git diff HEAD~5 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 6 only - Deletions in the adapters. Items: §3.1 DELETE rows, cross-adapter duplicates (§3.1 table rows 1–3 and `plan_pointer`), A-2, A-3, A-4, A-6. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 6 -->
---

<!-- BEGIN STAGE 7 -->
## Stage 7 - Deletions in daemon, cli, core, small crates and JS
<!-- STAGE 7: tier-effort -->
**Tier:** standard
**Effort:** extended
<!-- STAGE 7: tier-rationale -->
**Tier rationale:** A long list across many crates, but each deletion has a named reason. Extended effort, because the executor must confirm per test that the covering test really exists before it deletes.
<!-- STAGE 7: items -->
**Items:** §3.2–§3.9 DELETE rows; C-A1, C-A2, C-A3, C-A5, C-A8, C-A9, C-A10, C-A11, C-A13; C-B1, C-B2, C-B3, C-B4, C-B5, C-B11; K-1, K-2; S-4; J-2, J-3, J-7, J-8
<!-- STAGE 7: scope -->
**Scope:** Delete the tests outside the adapters that the report marks DELETE, or marks LIES-FP with the action delete.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 7: files -->
**Files:**
- `crates/ralphy-daemon/src/tests.rs` - the §3.2 DELETE list.
- `crates/ralphy-daemon/src/**` test code - the §3.3 DELETE list.
- `crates/ralphy-daemon/tests/observe_read.rs` (:236), `workspace_write.rs` (:449, :469, :492), `tree_watch.rs` (:115) - the §3.4 single-test deletions. File deletions that need new argv legs (`command_changes.rs`, `command_blob.rs` …) belong to Stage 10, not here.
- `crates/ralphy-cli/src/**` test code - the §3.5 and §3.6 DELETE lists, plus C-A1, C-A2, C-A3, C-A5, C-A8, C-A9, C-A10, C-A11, C-A13, C-B1 to C-B5, C-B11 (delete `live_animate_card`; mark `:123` as a manual probe with `#[ignore]` and a reason, or delete it).
- `crates/ralphy-core/src/**` test code and `crates/ralphy-core/tests/release_profile.rs` - the §3.7 DELETE list, K-1 (delete the file, which is one binary fewer), K-2.
- small crates test code - the §3.8 DELETE list and S-4.
- `crates/ralphy-daemon/ui-tests/*.test.mjs` - the §3.9 DELETE list, J-2, J-3, J-7 (rename to what it checks, or delete), J-8.
- `docs/audit-tests-2026-09-27.md` - §7 rows.

<!-- STAGE 7: order -->
**Order of operations:**
1. Per crate: grep each test, confirm the covering test, and apply rule 2 (a mutation where the deleted test aimed at one; a category otherwise).
2. `github/issues/tests.rs:137` (core): the report asks to confirm first that `parse_issue` goes through `From<GhIssue>`. If it does not, keep the test and mark it `kept` with the reason.
3. Commit once per crate. Run the gate with `--ui` for the JS commit.
4. §7 rows.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 7: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids C-A1,C-A2,C-A3,C-A5,C-A8,C-A9,C-A10,C-A11,C-A13,C-B1,C-B2,C-B3,C-B4,C-B5,C-B11,K-1,K-2,S-4,J-2,J-3,J-7,J-8` passes before the last commit. `crates/ralphy-core/tests/release_profile.rs` is absent.

<!-- STAGE 7: manual -->
**Manual verification (if any):** none

<!-- STAGE 7: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-7-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 7: handoff -->
**Hand-off prompt for Stage 7:**
> You are executing Stage 7 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 7 -->` and `<!-- END STAGE 7 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..6 committed (confirm with `git log --oneline -6`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -6` and `git diff HEAD~6 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 7 only - Deletions in daemon, cli, core, small crates and JS. Items: §3.2–§3.9 DELETE rows; C-A1, C-A2, C-A3, C-A5, C-A8, C-A9, C-A10, C-A11, C-A13; C-B1, C-B2, C-B3, C-B4, C-B5, C-B11; K-1, K-2; S-4; J-2, J-3, J-7, J-8. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 7 -->
---

<!-- BEGIN STAGE 8 -->
## Stage 8 - Fix the remaining LIES-FP tests
<!-- STAGE 8: tier-effort -->
**Tier:** judgment
**Effort:** extended
<!-- STAGE 8: tier-rationale -->
**Tier rationale:** Each fix is small, but each needs a red run against a reasoned mutation that may be wrong. The executor must decide `rejected` honestly, and must send to follow-up any fix that needs production code.
<!-- STAGE 8: items -->
**Items:** A-1, A-5, A-7, A-8, D-1, D-2, D-3, D-4, D-5, D-6, D-7, C-A4, C-A6, C-A7, C-A12, C-A14, C-B6, C-B7, C-B8, C-B9, C-B10, S-1, S-2, S-3, S-5, S-6, S-7, S-8, S-9, S-10, S-11, J-1, J-4, J-5, J-6, J-9
<!-- STAGE 8: scope -->
**Scope:** Fix the remaining LIES-FP tests by rule 1, so that each one fails against the mutation that the report names.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 8: files -->
**Files:**
- Test code only, at the locations in report §3 for each ID (grep the test name). The report states the fix for each: for example D-1 asserts `"'/usr/local/bin/ralphy' daemon"`, D-4 compares with the literal `GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ`, S-2 adds a dead pid, S-10 shuffles the input, and J-9 keeps one exact table.
- A-1 (codex skills copy) and A-5 (gemini triage attachments): if the fix needs a production change, mark them `follow-up` and add them to `docs/plans/issue-485-followup-draft.md`.
- D-5 and C-B6: the report says keep. Mark them `kept` with the caveat; no change.
- D-3: pin the probe argv, or delete if the probe is not reachable without starting WSL.
- `docs/plans/issue-485-followup-draft.md` - only if A-1 or A-5 goes to follow-up.
- `docs/audit-tests-2026-09-27.md` - §7 rows.

<!-- STAGE 8: order -->
**Order of operations:**
1. For each ID in the order of the report: apply the mutation, show PASS, fix the test, show FAIL, revert, show PASS. Write the evidence line.
2. If a mutation does not survive (the test already FAILS), mark the ID `rejected` with the reason and leave the test alone.
3. Commit when all the IDs of one crate are done, or once at the end if that is simpler; either way the evidence lines go in the body.
4. Run the gate with `--ui` (J-* rows touch ui-tests).
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 8: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids A-1,A-5,A-7,A-8,D-1,D-2,D-3,D-4,D-5,D-6,D-7,C-A4,C-A6,C-A7,C-A12,C-A14,C-B6,C-B7,C-B8,C-B9,C-B10,S-1,S-2,S-3,S-5,S-6,S-7,S-8,S-9,S-10,S-11,J-1,J-4,J-5,J-6,J-9` passes. Every `fixed` ID has an evidence line.

<!-- STAGE 8: manual -->
**Manual verification (if any):** none

<!-- STAGE 8: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-8-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 8: handoff -->
**Hand-off prompt for Stage 8:**
> You are executing Stage 8 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 8 -->` and `<!-- END STAGE 8 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..7 committed (confirm with `git log --oneline -7`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -7` and `git diff HEAD~7 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 8 only - Fix the remaining LIES-FP tests. Items: A-1, A-5, A-7, A-8, D-1, D-2, D-3, D-4, D-5, D-6, D-7, C-A4, C-A6, C-A7, C-A12, C-A14, C-B6, C-B7, C-B8, C-B9, C-B10, S-1, S-2, S-3, S-5, S-6, S-7, S-8, S-9, S-10, S-11, J-1, J-4, J-5, J-6, J-9. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 8 -->
---

<!-- BEGIN STAGE 9 -->
## Stage 9 - Merges and LIES-FN loosening
<!-- STAGE 9: tier-effort -->
**Tier:** standard
**Effort:** extended
<!-- STAGE 9: tier-rationale -->
**Tier rationale:** Table-driving is a known pattern, but the case count must be kept and brittle pins must be loosened without losing the behavior check. The volume is large, hence extended effort.
<!-- STAGE 9: items -->
**Items:** §3 MERGE rows (all slices); §3 LIES-FN rows (loosen); the §3.8 rename of `pty/tests/pty.rs:168`
<!-- STAGE 9: scope -->
**Scope:** Merge near-identical tests into named-row tables, and loosen the brittle LIES-FN pins, keeping every behavior case.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 9: files -->
**Files:**
- Test code at the MERGE and LIES-FN locations of report §3.1 (within a crate), §3.2 (the non-JS-fold merges; JS-fold pins are Stage 11), §3.3, §3.5, §3.6 (including the `guard/tests.rs` tables, with the cheap extra rows the report lists), §3.7, §3.8 and §3.9. The `include_str!` pins listed as LIES-FN in §3.1 are loosened to absence lints or trimmed to the behavior; the `run_hook` seam that would replace them is out of scope.
- `crates/ralphy-pty/tests/pty.rs` - rename `kills_and_waits_the_process_tree` → `kills_and_waits_the_child`.
- A test block that a merge pushes past 500 lines moves to `foo/tests.rs` (ADR-0022).
- `docs/audit-tests-2026-09-27.md` - §7 rows (one per merge group; one per loosened test).

<!-- STAGE 9: order -->
**Order of operations:**
1. One crate at a time. Count the cases before a merge. The table has the same number of rows, and each assert message names its row.
2. One mutation per merged group: show that the table FAILS. Evidence line.
3. For each LIES-FN row: apply the harmless edit named in the report and show PASS. Then apply one real mutation and show FAIL. Evidence line.
4. Commit per crate or per slice. Run the gate with `--ui` when JS changed.
5. Record the new test count (`cargo nextest list --workspace --message-format oneline | wc -l`) in the stage report.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 9: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui` passes. `cargo test -p xtask` passes (the inline-test-module size lint). §7 has a row per merge group.

<!-- STAGE 9: manual -->
**Manual verification (if any):** none

<!-- STAGE 9: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-9-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 9: handoff -->
**Hand-off prompt for Stage 9:**
> You are executing Stage 9 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 9 -->` and `<!-- END STAGE 9 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..8 committed (confirm with `git log --oneline -8`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -8` and `git diff HEAD~8 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 9 only - Merges and LIES-FN loosening. Items: §3 MERGE rows (all slices); §3 LIES-FN rows (loosen); the §3.8 rename of `pty/tests/pty.rs:168`. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 9 -->
---

<!-- BEGIN STAGE 10 -->
## Stage 10 - Integration binaries and re-measure
<!-- STAGE 10: tier-effort -->
**Tier:** judgment
**Effort:** extended
<!-- STAGE 10: tier-rationale -->
**Tier rationale:** Merging integration binaries changes how env vars and statics are shared. A wrong grouping makes tests racy. The executor must also report honestly when the merge gives no speed gain.
<!-- STAGE 10: items -->
**Items:** §3.4 binary consolidation, §3.6 integration binaries, §3.7 binaries, Phase 3 re-measure
<!-- STAGE 10: scope -->
**Scope:** Reduce the integration test binaries (daemon 55 → about 26, cli 9 → 1, core −3) without changing any test body, then measure again.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 10: files -->
**Files:**
- `crates/ralphy-daemon/tests/*.rs` - follow §3.4 exactly:
  - Add one argv assert per leg in `command_checkout_cwd.rs` for blob, branch, config and worktree, plus a `board.list` leg. Then delete `command_blob.rs`, `command_branch.rs`, `command_config.rs`, `command_worktree.rs`, `command_board.rs` and `command_changes.rs`.
  - Move `command_config_mutate.rs` → a leg in `command_mutate_git.rs`, and `command_mutate_ok_message.rs` → a leg in `command_changes_mutate.rs`. Put `command_run_params.rs` + `command_ws.rs` in one file.
  - Put the session/console group in one binary with `static Once`. Move `fleet_console.rs` into `fleet_session.rs`. Put `codec_transport_free.rs` and `session_transport_free.rs` in one file.
  - Put the env-free files in three binaries: observe, watch, ws/peer. Each is a `tests/<group>/main.rs` with `mod` files, or a single file.
  - The files the report lists under "Keep separate" stay separate.
- `crates/ralphy-cli/tests/*.rs` → `crates/ralphy-cli/tests/verbs/main.rs` + one `mod` per old file, with the shared `init_repo`/`run_git`/`hold_run_lock` helpers in one module. The ~10 lock-refusal tests plus `config set`/`config unset` (Stage 5) become one table with one lock-holder child. Keep in mind the §3.6 note on `tests/mutate.rs:152`, which switches to a branch that does not exist.
- `crates/ralphy-core/tests/effort.rs` → `crates/ralphy-core/src/effort.rs` test module. `tests/prompt_ledger.rs` → merged into `tests/prompt_assembly.rs`. `release_profile.rs` was deleted in Stage 7. `tests/stop.rs` stays separate (process-global flag).
- `docs/audit-tests-2026-09-27.md` - §7 rows, and the **After** column of the measurement table.

<!-- STAGE 10: order -->
**Order of operations:**
1. Before moving anything, run `cargo nextest list --workspace --message-format oneline` and save the list of test names (in the gitignored `docs/plans/logs/`).
2. Move the files group by group. After each group, run the moved tests alone, then check that the list of test names is the same apart from the binary prefix and the tests deleted on purpose (the `command_*` files replaced by legs).
3. Run the full gate.
4. Measure exactly as in Stage 1 (3 runs, median; test count; binary count; node time). Fill the After column. If the suite is not faster, or is slower, say so in §7 and in the report. Do not hide it.
5. Commit per crate.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 10: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py` passes. `cargo nextest list --workspace --list-type binaries-only --message-format oneline | wc -l` is lower than the Stage 1 count. The before/after name lists match, apart from the listed deletions.

<!-- STAGE 10: manual -->
**Manual verification (if any):** none

<!-- STAGE 10: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-10-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 10: handoff -->
**Hand-off prompt for Stage 10:**
> You are executing Stage 10 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 10 -->` and `<!-- END STAGE 10 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..9 committed (confirm with `git log --oneline -9`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -9` and `git diff HEAD~9 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 10 only - Integration binaries and re-measure. Items: §3.4 binary consolidation, §3.6 integration binaries, §3.7 binaries, Phase 3 re-measure. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 10 -->
---

<!-- BEGIN STAGE 11 -->
## Stage 11 - Daemon asset pins
<!-- STAGE 11: tier-effort -->
**Tier:** judgment
**Effort:** standard
<!-- STAGE 11: tier-rationale -->
**Tier rationale:** The executor must tell which lines of each Rust asset pin duplicate a node test, and which guard markup, CSS or wiring that no node test sees. It must also move a spawned JS test into the node runner without losing a case.
<!-- STAGE 11: items -->
**Items:** Issue phase 4: §3.2 LIES-FP (`:3564`, `:5122`, `:6328`, `:6900`), the §3.9 list of Rust pins made redundant, G-8 (moved logic), the two defects inside KEEP tests (`:1125`, `:2112`), `wb_session_owner_351.js`, `wb_fleet_label.js`, the stale comments
<!-- STAGE 11: scope -->
**Scope:** Trim the Rust asset pins in `ralphy-daemon/src/tests.rs` to what no node test covers, and move pure UI logic and the spawned JS test into `ui-tests`.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 11: files -->
**Files:**
- `crates/ralphy-daemon/src/tests.rs` - remove the `function X(` lines (and JS-fold lines) that duplicate a `node --test` test, at the pins the report lists at the end of §3.9 and in §3.2 LIES-FN. Keep the markup, CSS and call-order lines, and every test under "Keep as they are". Fix `:1125` `security_state_reflects_the_stores` (re-read the state after setting the flags) and `:2112` `api_agents_serves_the_roster` (build the expected set from `session::Agent::ALL`). Update the comments that call a pin "the only gate in CI" (CI runs `node --test`, `ci.yml`). Remove the node spawn of `wb_session_owner_351.js` (about :6833-6846).
- `crates/ralphy-daemon/ui-tests/app.test.mjs` - behavior tests through `loadShell()` for `isFolder`, `_changesRefused`, `isNoteInNotesDir`, `syncBusy` and `planProseIsCurrent` (G-8). Stage 12 handles `modeFor` in `wb-mode.js` if it is not covered here.
- `crates/ralphy-daemon/ui-tests/wb-session-route.test.mjs` - receives the 5 tests of `tests/wb_session_owner_351.js` (the file is already imported by `index.mjs`).
- `crates/ralphy-daemon/tests/wb_session_owner_351.js`, `crates/ralphy-daemon/tests/wb_fleet_label.js` - delete (covered by `wb-fleet.test.mjs:233`).
- `docs/audit-tests-2026-09-27.md` - §7 rows.

<!-- STAGE 11: order -->
**Order of operations:**
1. For each Rust pin to trim, name the node test that covers the removed lines. Then apply one mutation to the JS fold: the node test FAILS. Evidence line.
2. For each §3.2 LIES-FP row (`:3564`, `:5122`, `:6328`, `:6900`), apply rule 1 with the report's mutation.
3. Move `wb_session_owner_351.js` case by case. The 5 cases are present, and one mutation makes the moved test FAIL.
4. For each G-8 function, write the node test and show that it FAILS against a mutation of the function. Then remove the matching Rust substring pin.
5. `:1125` and `:2112`: rule 1 (the stale snapshot passes when the state is wrong; the hard-coded list passes when a vendor is missing from `ALL`).
6. Run the gate with `--ui`.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 11: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui` passes. `crates/ralphy-daemon/tests/wb_session_owner_351.js` and `wb_fleet_label.js` are absent. `grep -n 'wb_session_owner_351' crates/ralphy-daemon/src/tests.rs` returns nothing. `grep -rn 'only gate in CI' crates/ralphy-daemon/src` returns nothing.

<!-- STAGE 11: manual -->
**Manual verification (if any):** none

<!-- STAGE 11: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-11-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 11: handoff -->
**Hand-off prompt for Stage 11:**
> You are executing Stage 11 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 11 -->` and `<!-- END STAGE 11 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..10 committed (confirm with `git log --oneline -10`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -10` and `git diff HEAD~10 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 11 only - Daemon asset pins. Items: Issue phase 4: §3.2 LIES-FP (`:3564`, `:5122`, `:6328`, `:6900`), the §3.9 list of Rust pins made redundant, G-8 (moved logic), the two defects inside KEEP tests (`:1125`, `:2112`), `wb_session_owner_351.js`, `wb_fleet_label.js`, the stale comments. The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 11 -->
---

<!-- BEGIN STAGE 12 -->
## Stage 12 - Gaps, flakes and closure
<!-- STAGE 12: tier-effort -->
**Tier:** judgment
**Effort:** extended
<!-- STAGE 12: tier-rationale -->
**Tier rationale:** Each gap test must show red against a mutation of existing behavior. The flake changes apply only with evidence. The closure must leave no ID without a status.
<!-- STAGE 12: items -->
**Items:** G-1, G-2, G-3, G-4, G-5, G-6, G-7, G-8, G-9, G-10, G-11, G-12, G-13; issue phase 7 flakes; closure (§7 complete, follow-up draft, changelog)
<!-- STAGE 12: scope -->
**Scope:** Add one test per behavior gap, act on the flake risks only where the issue allows it, and close the §7 ledger.
**Scope discipline:** stay within the declared file list; if the stage requires
touching files outside it, STOP and report instead of silently expanding.

<!-- STAGE 12: files -->
**Files:**
- Test code at the gap locations in report §4:
  - G-1 `adapter-support` `classify` `errored: true` case.
  - G-2 scaffold charter write and stale-plan removal.
  - G-3 codex limit text on a clean exit.
  - G-4 the `since` boundary in the usage-scan scanners, including kimi.
  - G-5 release HTTP 429 retry.
  - G-6: the Stage 9 rename is the whole action. Mark it `follow-up` (a tree kill is a production change).
  - G-7 OSC 52 gating: mark it `follow-up` if the node harness cannot reach it without production changes (see the Stage 5 draft).
  - G-8 `modeFor`, if Stage 11 left it.
  - G-9 `spendView`/`floorNote` in a `wb-spend.test.mjs` test.
  - G-10 the WSL probe argv.
  - G-11: check the `internal_commands…` argv against what the daemon spawns.
  - G-12: covered by Stage 4 pins or by follow-up.
  - G-13: grep each adapter's tests for `emit::planning`/`emit::executing` argument checks, and record whether `runstate/capture/tests.rs:509` can go. Do not delete it here; that is a policy call (out of scope).
- Flakes (phase 7), only these without flake evidence, because they remove real network or real waiting from a unit test:
  - `ralphy-cli/src/events/emitter.rs` tests that call `detect()` build `Emitter` as a struct literal.
  - `peer/nudge/tests.rs` `nudge_never_waits` uses `ping -n 8` with a `> 5s` bound, if that keeps the claim.
  - Everything else in phase 7 gets a §7 row `kept (no flake evidence)`.
- `docs/plans/issue-485-followup-draft.md` - fill `## Also out of scope in #485`: the issue's own out-of-scope list, the phase 6 re-audit list (report §5), and every §7 row with status `follow-up`.
- `changelog.d/485.md` - `kind: internal`, one plain sentence (format: `changelog.d/README.md`).
- `docs/audit-tests-2026-09-27.md` - §7 rows. Check that every ID in §2–§4 has a row.

<!-- STAGE 12: order -->
**Order of operations:**
1. One gap at a time, by rule 5: write the test, apply a mutation of the existing behavior, and show FAIL. Revert and show PASS. Evidence line.
2. The two flake changes: show that the test still FAILS against a mutation of the behavior it checks.
3. Run `python docs/plans/issue-485-test-suite-cleanup-verify-e2e.py --ledger-only` and add the missing rows until it passes.
4. Follow-up draft, changelog fragment, `cargo run -q -p xtask -- changelog --check`.
5. Run the gate with `--ui`.
N. Gates pass -> write the post-stage report -> stage code files AND the
   report file together -> commit. (One commit per stage; report is committed
   alongside the code.)

<!-- STAGE 12: verification -->
**Verification:** `python docs/plans/issue-485-test-suite-cleanup-verify-gate.py --ui --ids G-1,G-2,G-3,G-4,G-5,G-6,G-7,G-8,G-9,G-10,G-11,G-12,G-13` passes. `python docs/plans/issue-485-test-suite-cleanup-verify-e2e.py --ledger-only` passes. `changelog.d/485.md` exists.

<!-- STAGE 12: manual -->
**Manual verification (if any):** none

<!-- STAGE 12: report -->
**Post-stage report:** write `C:\Dev\ralphy/docs/plans/issue-485-test-suite-cleanup-stage-12-report.md`. Copy `docs/plans/_report-template.md` as the starting structure; leave the `Commit:` slot as `_filled by parent_` — the End-to-end summary table is the canonical source for that mapping.

<!-- STAGE 12: handoff -->
**Hand-off prompt for Stage 12:**
> You are executing Stage 12 of Issue #485 — fix the lying tests, then cut the duplicates at C:/Dev/ralphy/docs/plans/issue-485-test-suite-cleanup.md.
> From that plan file, read ONLY: (a) `## Execution model`, (b) `## Execution policy`,
> (c) `## Hand-off conventions`, (d) `## Global conventions`, (e) `## Critical files`,
> and (f) your own stage block between `<!-- BEGIN STAGE 12 -->` and `<!-- END STAGE 12 -->`.
> Do NOT read other stages' blocks — they are not your context. Then read
> C:\Dev\ralphy/CLAUDE.md for repo-wide rules. Your authoritative spec is the stage block.
>
> Repo root: C:/Dev/ralphy
> Branch: test/485-suite-audit (commit here; never create a branch, never push)
> Platform: Windows 11 (Git Bash available; CI also runs Linux and macOS)  (Windows: use bash syntax, forward slashes)
>
> Status: Stages 1..11 committed (confirm with `git log --oneline -11`).
> Prior stages' work is reflected in: (1) the actual code state — run
> `git log --oneline -11` and `git diff HEAD~11 HEAD --stat` if you need
> to see what changed; (2) `## Critical files` in the plan (cross-stage index);
> (3) prior stage reports under `docs/plans/<slug>-stage-K-report.md` if you
> need detail on a specific surprise or deviation. Do NOT read other stages'
> BEGIN/END blocks for prior context — git is the source of truth.
>
> Line-number hints in the plan may be stale after prior stages; grep for symbols.
>
> Your scope: Stage 12 only - Gaps, flakes and closure. Items: G-1, G-2, G-3, G-4, G-5, G-6, G-7, G-8, G-9, G-10, G-11, G-12, G-13; issue phase 7 flakes; closure (§7 complete, follow-up draft, changelog). The source of every finding is `docs/audit-tests-2026-09-27.md`; read the report sections your items name, plus §7.
>
> Spec is your stage block (Files, Order of operations, Verification, Report
> path). Gates/invariants/commit style: `## Global conventions`. Working-tree
> policy: `## Execution policy`. Authorization/scope/failure/return-to-parent:
> `## Hand-off conventions`.
>
> Commit step: after gates pass, copy `docs/plans/_report-template.md` to the
> report path declared in your stage block (leave the `Commit:` slot as
> `_filled by parent_`), then stage code files AND the report together by
> explicit path and commit with the
> `Co-Authored-By: $EXECUTOR_NAME $EXECUTOR_EMAIL` trailer. One commit per stage.
>
> Begin now.

<!-- END STAGE 12 -->
---

## Reviewer gate (only if Reviewer != none)
**Tier:** critical
**Effort:** extended
After the final stage commits green:
- reviewer: light -> small subagent validates scope, diff vs. plan, gate
  results, post-stage reports, and obvious risk. Does NOT replan.
- reviewer: deep -> same plus security/perf/maintainability lens for
  stack-relevant best practices.

Reviewer returns a verdict in
`pass | pass-with-notes | fail | blocked`
plus a findings list (each: `file:line`, severity, description).
Reviewer never edits code and never replans.
If a `reviewer` skill is available in the executor, prefer it; otherwise use
an inline QA prompt that takes the plan + diff range as input.

### Arbiter (run only if findings list is non-empty)
**Tier:** critical / **Effort:** focused
Reads the reviewer verdict + findings + diff range. For each finding,
applies this decision tree verbatim:

1. Is this a real defect? (correctness, security, contract, data integrity)
   - No  -> classify `nice-to-have`.
   - Yes -> step 2.
2. Is the fix mechanical (one obvious right answer, no design choice)
   AND fully inside the plan's declared file list?
   - Yes -> classify `must-fix`.
   - No  -> classify `human-judgment`.

Arbiter records both answers per finding in the output md (auditable).
Arbiter does NOT edit code and does NOT replan.

### Fix round (run only if any `must-fix` exists; HARD MAX = 1 round)
**Tier:** standard / **Effort:** focused
Fix-subagent receives only the `must-fix` items + their target files.
- Applies fixes within those files only.
- For each item: writes a `fix note` -- what changed, line(s), and one
  sentence linking the change to the original finding.
- If a `must-fix` proves to require out-of-scope work or a design choice,
  reclassify it as `human-judgment` and skip it. Do NOT block sibling fixes.
After the round: re-run every declared gate (build/lint/test/etc.). Gate
failure does NOT trigger another fix round -- the failure goes into the
pending list and the verdict degrades.

### Re-review (conditional)
Run a second reviewer pass (same level: light/deep) only if EITHER:
- The plan's Tier was `critical`, OR
- The fix round modified files outside the declared scope of the
  originating `must-fix` finding (scope-creep signal).

Any *new* finding from re-review goes straight to the pending list of the
next sequence file. Re-review does NOT trigger another fix round.

### Output file (always written when this gate runs)
Path: `docs/plans/reports/issue-485-test-suite-cleanup_reviewer_<seq>.md`
- `<seq>` is a zero-padded 3-digit counter starting at `001`, incremented
  for each run of this gate against the same plan.
- Each sequence file is **immutable** once written. Re-runs produce a
  new file, never overwrite.

Top of file: final verdict in
`pass | pass-with-notes | pass-with-fixes | pass-with-pending | fail | blocked`

Body sections (in order):
- `## Reviewer verdict` -- raw reviewer output, verbatim.
- `## Arbiter classification` -- table per finding: `file:line`, severity,
  class (must-fix / nice-to-have / human-judgment), decision-tree answers
  (defect? yes/no -- mechanical+in-scope? yes/no), 1-line reason.
- `## Fixes applied` -- one entry per `must-fix` corrected, with the fix
  note (what changed, lines, link to finding).
- `## Pending` -- every `human-judgment` finding + every `must-fix`
  reclassified to `human-judgment` during the fix round + any new finding
  from re-review. Each entry:
  - `file:line`
  - reviewer's original finding (short quote)
  - arbiter's reason for human classification
  - suggested action (may be "decide whether to address")

Verdict mapping:
- `pass` / `pass-with-notes` -- reviewer's original verdict, no findings
  needed fixing.
- `pass-with-fixes` -- all `must-fix` corrected, no pending items.
- `pass-with-pending` -- corrections completed (or none needed), but
  `Pending` section is non-empty.
- `fail` / `blocked` -- reviewer's verdict was `fail`/`blocked`, OR gates
  failed after the fix round. Parent stops the plan and surfaces the md
  file path to the user.

## Critical files (cross-stage index)
| File | Stages |
|---|---|
| `docs/audit-tests-2026-09-27.md` (§7 ledger) | 1–12 |
| `docs/plans/issue-485-followup-draft.md` | 5, 12 |
| `crates/ralphy-daemon/src/tests.rs` | 4, 7, 9, 11 |
| `crates/ralphy-daemon/src/tree.rs`, `note.rs` (test modules) | 2 |
| `crates/ralphy-daemon/tests/*.rs` | 2 (`observe_read`, `runs_watch`, `child_env_hygiene`), 7, 10 |
| `crates/ralphy-daemon/ui-tests/*.test.mjs`, `index.mjs` | 4, 7, 8, 9, 11, 12 |
| `crates/ralphy-daemon/tests/wb_session_owner_351.js`, `wb_fleet_label.js` | 11 |
| `crates/ralphy-proc-util/src/cursor.rs` (tests) | 2, 7, 8 |
| `crates/xtask/tests/user_text_cites_no_adr.rs` | 2 |
| `crates/ralphy-agent-{gemini,cursor,copilot}/src/**` test code | 3, 5, 6, 8, 9 |
| `crates/ralphy-agent-*/src/**`, `ralphy-adapter-support/src/**` test code | 6, 8, 9, 12 |
| `crates/ralphy-cli/src/**` test code | 5, 7, 8, 9, 12 |
| `crates/ralphy-cli/tests/*.rs` | 5, 10 |
| `crates/ralphy-core/src/model_recovery.rs` (tests), `crates/ralphy-core/tests/*` | 5, 7, 9, 10 |
| small crates (`usage-scan`, `release`, `pricing`, `pty`, `run-snapshot`, `xtask`) test code | 7, 8, 9, 12 |
| `changelog.d/485.md` | 12 |

## End-to-end verification (after final stage)
Run `python docs/plans/issue-485-test-suite-cleanup-verify-e2e.py`. It runs the full CI gate
with `--ui`, then checks two things. First, every finding ID in §2–§4 of `docs/audit-tests-2026-09-27.md` has a row in
§7. Second, no `stage N` placeholder is left in the Commit column; the parent fills in the SHAs
first. Then, by hand:
- `git diff 78628fe4 --stat -- crates/` — for every production file listed, confirm that the diff
  is inside its `#[cfg(test)]` module, or under `src/bin/`.
- §7 measurements: both the Before and the After columns are filled.
- `docs/plans/issue-485-followup-draft.md` is ready to publish. Ask the maintainer before
  `gh issue create`; publishing is outward-facing.

## End-to-end summary (parent fills after final stage)
| Stage | Title | Tier | Effort | Model used | Commit SHA | Status | Report |
|-------|-------|------|--------|------------|------------|--------|--------|
<!-- one row per stage. `Model used` is what the executor actually selected
on its platform for the declared Tier/Effort (the executor fills this — the
plan never prescribes model names). Used post-hoc to audit whether the
platform mapping is well-calibrated. If <40% of rows are mechanical/standard,
the decomposition is suspect — too many stages classified as judgment/critical
defeats the cost-savings purpose. -->
