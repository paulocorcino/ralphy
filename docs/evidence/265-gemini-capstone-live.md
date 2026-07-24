# Live capstone — `ralphy run --agent gemini` end-to-end (#265)

Raw-evidence companion to the accepted note
[docs/adr/0043-gemini-validation.md](../adr/0043-gemini-validation.md). Where that
file records the verdict per phase, this one carries the numbers, commands and log
lines behind it. The runbook that drove it is
[265-gemini-capstone-runbook.md](./265-gemini-capstone-runbook.md).

Host: Windows 11 (10.0.26200). `gemini --version` → **0.51.0**, node at
`C:\nodejs\node.exe`. Auth `security.auth.selectedType = "gemini-api-key"`
(credential in the Windows credential store; never read). Lab repo:
`C:\Dev\FinCal` (`paulocorcino/FinCal`), base `master` @ `f15623d5`
(the Cursor-indexing opt-out; pre-existing, harmless to Gemini). Binary:
`./target/release/ralphy.exe` from `feat/copilot` (rebuilt this session). Date:
2026-07-22. Model strategy: cheap flash pinned per phase (kills the D8 router tax);
`pro` deliberately unused.

Standard invocation:
```bash
./target/release/ralphy.exe run --repo C:/Dev/FinCal --issues <n> --agent gemini \
  --base-branch master --branch-mode new \
  --plan-model <flash-id> --exec-model <flash-id> --max-minutes-per-issue <n> --verbose
```
Note `--issues <n>` (direct fetch), not `--only-issue <n>` — the latter filters the
label queue and a labelless throwaway issue falls out as `no_work`.

## Phase 0 — baseline (feeds AC5)

SHA-256 manifest of every file under `C:/Users/PICHAU/.gemini` → **9 278 files**
(grew from the note's 9 264 via the operator's own interactive use). FinCal at
`master`@`f15623d5`, worktree clean, **8** pre-existing `afk/run-*` branches, and a
D4 owned root already present at `.ralphy/gemini-home/` from the #253–#264 slices —
all recorded as *pre-#265*, so AC5 means restoring this state, not a pristine master.

## Phase 1 — autonomy revocations against a real environment (AC3)

- **AutonomyDisabled (D5).** Staged `%ProgramData%\gemini-cli\settings.json` =
  `{"security":{"disableYoloMode":true}}`. `ralphy run … --agent gemini` **bailed in
  `prepare_root` before any spawn** (no model call), verbatim:
  `gemini's autonomous mode is disabled by the administrator setting
  security.disableYoloMode in the system settings file — ralphy reports it and does
  not work around it`. Detected by the **pre-spawn** admin tier (`read_admin_tier`),
  not the in-flight needle. Staged file removed afterward.
- **UntrustedWorkspace / exit 55 (D5).** `gemini -p hello` in `C:\Dev\FinCal` without
  `--skip-trust` printed the `revocation::NEEDLES` string verbatim (wrapped in the
  `ESC[31m…ESC[0m` CSI red the note records `vendor_line` stripping):
  `Gemini CLI is not running in a trusted directory. To proceed, either use
  --skip-trust, …`. The trust gate precedes the provider call, so no spend.
- **Residual (unchanged from the validation note):** argv `--policy` sovereignty over
  `invoke_agent` (schema removal) is proved by construction (`policy::tests`) not by a
  live turn; admin-tier-beats-argv and server-pushed admin controls stay out of reach
  (no managed host).

## Phase 2 — real plan-then-execute to green (AC1)

Throwaway directed issue **FinCal #119** ("create `LAB-GEMINI.md`, one line"),
mirroring the Cursor capstone's #117.

- **`gemini-2.5-flash` did NOT converge.** Plan written (feasible), execute committed
  the correct file — then **looped the full 10-min cap**: 410 KB transcript, tool
  calls but **zero `type:"result"` envelopes**, killed by `kill_tree`,
  `outcome=Timeout committed=true saw_result=false → non_green`. The last action was
  the `.ralphy/issue.json` gitignore refusal (#259). Filed as **#275**.
- **`gemini-3.5-flash` closed green.** Sequence:
  1. plan written (feasible), `## Verify` = bare `test -f LAB-GEMINI.md` (honoured the
     issue's no-nested-quote constraint);
  2. execute turn 1 → `Done saw_result=true status=success`, committed;
  3. **protocol-lint handback ×1** — plan carried a self-review step, first output
     lacked `## Self-review findings`; runner handed back once, turn 2 added the
     section (`committed=false`);
  4. **verify gate ran (`test -f LAB-GEMINI.md`) → passed**;
  5. `green — issue closed number=119`, knowledge note + consolidation, run
     `outcome="completed" issues_done=1`.
- Green-run commit `73140ab3` on `afk/run-20260722-101857`; `LAB-GEMINI.md` content
  exact with a single trailing LF; only that file changed; **#119 CLOSED on GitHub**.
- The `.ralphy/` refusal (#259) fired every turn (`plan-charter.md`, `plan.md`,
  `issue.json`, `protocol-failure.md`) — non-fatal for 3.5, fatal-by-timeout for 2.5.

## Phase 3 — usage vs billing, discrepancy explained (AC2)

Execute-turn terminal envelope (`result.stats`), pinned `gemini-3.5-flash`:
```
total_tokens 569377 · input_tokens 565088 · output_tokens 1342 · cached 448012 · input 117076
models: {"gemini-3.5-flash": {…}}   # single key — pinning removed the router
```
The three D9 traps, live:

| Trap | Naive | Correct | Here |
|---|---|---|---|
| 1 billable output | `output_tokens` 1342 | `total − input_tokens` = **4289** | thinking residual 2947 (~3.2× undercount) |
| 2 cached in input | 565088 + 448012 | `input` 117076 = `input_tokens − cached` | disjoint buckets, no double-count |
| 3 models map | one model | heaviest-model fold | one key ⇒ **no router tax** (D8) ✅ |

Ralphy ledger (`ralphy usage --project paulocorcino/FinCal --by model`):
`gemini-3.5-flash · 2.1M tok · $1.32 · 3 rows` (plan+exec1+exec2);
`gemini-2.5-flash · 0 tok · $0.00` (**the killed run reports 0 — usage rides the
envelope**, D9/D18); `gemini-routed · 0 tok · unpriced · 6 rows` (the `model.rs`
`ROUTED_KEY` sentinel, e.g. the model-`None` consolidation — the `+?`).

Vendor console (Gemini 3.5 Flash, operator-read, HITL): **RPM 9/1K · TPM 382.01K/2M ·
RPD 139/10K · spend R$ 25,16** (cumulative for the day).

**Reconciliation (explained, not noted):** Ralphy's 2.1M is **cache-inclusive**
(~1.7M is cache-read, which Google meters separately and cheaply) — the *opposite* of
D9's store under-report, and precisely why trap 2 keeps cache-read separable; Ralphy's
*uncached* (~430K) is the same order as the console's TPM 382K. **Requests track
tool-calls (~17/turn) with no turn-boundary doubling → pinning killed the router at the
billing layer, not just in the envelope** (D8). Gemini's console shows rate-limit
windows + cumulative RPD, so no digit-exact per-run match exists (unlike Cursor's CSV);
the reconciliation is mechanism-level. R$25.16 is the day's cumulative spend across all
models; the capstone's own modeled cost was $1.32.

## Phase 4 — budget kill, clean process tree (AC4)

Windows: the killed `gemini-2.5-flash` run exercised `kill_tree` at the 10-min cap and
reported **0 tokens** (above). A process sweep filtered to the capstone run windows
(10:04–10:41, 10:58) found **zero `gemini`/`node`/`pwsh`/`cmd` survivors** — `kill_tree`
(`taskkill /F /T`, D18) left no orphans; `ralphy.exe` also gone. **Linux/WSL half
deferred** (see Phase 5).

## Phase 5 — cross-platform parity (WSL)

**Deferred** — low-budget ruling. D16 binary resolution (reject the `/mnt/c` shim,
resolve the `~/.nvm` path) stays covered by `ralphy-proc-util` unit tests; the Windows
mechanics closed clean. Recorded as an open item, not observed live this pass.

## Phase 6 — restore & isolation (AC5)

- Operator root re-manifested: **9 278 files, content-level diff vs Phase 0 = 0** —
  `~/.gemini` byte-identical. Neither the isolated ralphy runs (via `GEMINI_CLI_HOME`)
  nor the direct untrusted probe wrote to it (D4 proved live).
- FinCal returned to `master`@`f15623d5`, **8 `afk/run-*` branches — identical to the
  baseline set** (the three run branches this capstone created were deleted, pre-run
  tags removed), worktree clean.
- Host residue removed: staged `%ProgramData%\gemini-cli\` deleted, scratch manifests
  cleaned.

## Phase 7 — HITL ruling (AC6)

Maintainer ruling, 2026-07-22 (low-budget): **both deferred, recorded as open limits.**
- **True quota exhaustion (D11)** — stays deferred; observing it would burn ~10K
  requests/day (run sat at 139/10K), contradicting the budget. `429→Limit(None)` +
  ADR-0030 synthetic cadence remains provisional but is the cheapest to revise and is
  unit-covered.
- **Browser-OAuth isolation (D4)** — documented as an open limit; the operator uses
  `gemini-api-key`, fully validated and byte-identical-proven. OAuth's file-based
  credential under the relocated root is untested.

## Follow-ups filed

**#275** — the Gemini child cannot read its own `.ralphy/` artifacts (gitignore
refusal); non-fatal for a capable model, but the weakest flash (`gemini-2.5-flash`)
loops to the budget cap without emitting a result.

## Verdict

AC1 ✅ · AC2 ✅ · AC3 ✅ (two hard-stops live; policy-schema residual) · AC4 ✅ Windows
(Linux deferred) · AC5 ✅ · AC6 ✅ ruled (both deferred) · AC7 ✅. Total API spend
≈ $1.4 (green run $1.32 dominant), all on pinned flash.
</content>
