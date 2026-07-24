# Runbook — Kimi deep re-validation capstone (#274)

A trail-to-completion for the HITL deep re-validation of the `ralphy-agent-kimi`
vendor (ADR-0028, issue **#274**). The first capstone ([#155](https://github.com/paulocorcino/ralphy/issues/155),
note [0028-kimi-validation](../adr/0028-kimi-validation.md)) was already thorough —
a real repo to a green close, the encoding fix, the `wire.jsonl` token harvest,
live triage — so this pass is **narrow**: it closes only the Cursor-capstone
([#251](https://github.com/paulocorcino/ralphy/issues/251)) dimensions that note
left on **reasoning rather than observation**. Its marquee target is the
**exit-75 ceiling (D9)**, which #155 could never induce ("a real 429 could not be
forced without burning quota").

This file is the **operational checklist**, not the evidence. On completion the
captured numbers, commands and log lines move into
`docs/evidence/274-kimi-capstone-live.md` (H2 phases mirroring
`251-cursor-capstone-live.md` / `272-copilot-capstone-live.md`), raw per-command
logs go to `docs/live/kimi-274-<probe>.log`, and the plan of record
[`docs/adr/0028-kimi-revalidation.md`](../adr/0028-kimi-revalidation.md) is
rewritten from a plan into a note with its Status flipped **proposed→accepted**.
**The plan file is the contract** — but see the drift ledger below.

---

## ⚠️ Contract drift — the plan predates the `kimi-code` migration

The plan of record (`0028-kimi-revalidation.md`) was written against **`kimi`
1.48.0**, a Python/Textual-TUI build. This host — and the adapter code itself —
have since migrated to **`kimi-code` 0.28.0** (migrator run **2026-07-20**,
recorded in `~/.kimi/.migrated-to-kimi-code`). The adapter source already tracks
0.28; only the plan text lagged. Four plan premises are obsolete and are
**re-grounded against code** for this run:

| Plan text (1.48.0) | Reality in `crates/ralphy-agent-kimi/src` (0.28) |
|---|---|
| `PYTHONUTF8=1` is the headline encoding fix; the Textual "No Windows console found" TUI trap | **No `PYTHONUTF8` anywhere in the crate** (grep: 0 hits). 0.28 is not the Python build; the encoding phase (2, 6) re-targets to "zero decode/charmap crashes under a subprocess-heavy build", not an env-var assertion |
| auth stop prints `LLM not set` / exit 1 | `auth.rs:8` message `Kimi is not authenticated (auth.login_required)…`; `auth.rs:16` matches `auth.login_required` via the **shared** `ralphy_adapter_support::auth_error` helper |
| model `kimi-code/kimi-for-coding` passed with `-m` (D4) | `command.rs:10` default `kimi-code/k3` (config.toml `default_model = "kimi-code/k3"`; `kimi-for-coding` still exists as a named model) |
| creds `~/.kimi/credentials/kimi-code.json`; store under `~/.kimi` | `usage.rs:53` store `$KIMI_CODE_HOME/sessions` else `~/.kimi-code/sessions`; creds `~/.kimi-code/credentials/kimi-code.json` |

**Disposition:** this is a *plan-text* drift, not a code defect — no issue is
filed for it. The re-grounded facts above govern each phase; the plan-of-record
rewrite (Phase 7) folds this migration in as the note's opening. If any phase
finds the **code** still assuming 1.48.0 behaviour, that is a defect and follows
the gap protocol below.

---

## Resolved environment (captured 2026-07-22)

| Field | Value |
|-------|-------|
| Host OS | Windows 11 Pro 26200 |
| Kimi CLI | **`kimi-code` 0.28.0** (`kimi --version`) — #155 ran `kimi` **1.48.0** (a *different product line*, migrated 2026-07-20; the version-skew data point for Phase 6) |
| Kimi on PATH | `C:\Users\PICHAU\.kimi-code\bin\kimi.exe` (git-bash `/c/Users/PICHAU/.kimi-code/bin/kimi`) — resolved by `resolve_program("kimi")` (`command.rs:28`, off `PATH`/`~/.local/bin`) |
| Ralphy binary | `C:/Dev/ralphy/target/release/ralphy.exe` (release) · branch `feat/copilot` |
| Auth store | `~/.kimi-code/credentials/kimi-code.json` (legacy `~/.kimi/credentials/kimi-code.json` still present — Phase 5 must diff the **active** `~/.kimi-code` one) |
| Config | `~/.kimi-code/config.toml` (`default_model = "kimi-code/k3"`) — Phase 5 byte-diff baseline |
| Session store | `~/.kimi-code/sessions/wd_<repo>_<hash>/session_<uuid>/agents/<agent>/wire.jsonl` — `wd_fincal_358d77c78713` already present |
| Sim repo | `C:/Dev/FinCal` (lab, authorized — [[fincal-lab-repo]]); base branch, tree clean, **no stale `.ralphy/plan.md`** |
| Baselines | `~/ralphy-274-baseline/` — `credentials/`, `config.toml.before`, `fincal-head.before`, `sessions-list.before` |

Standard invocation (Windows shown; WSL/Linux identical but
`./target/release/ralphy` and forward-slash paths):

```bash
./target/release/ralphy.exe run --repo C:/Dev/FinCal --issues <n> --agent kimi \
  --base-branch <base> [--dry-run] --verbose
```

---

## Why this is HITL, and what only a human can close

- **The exit-75 ceiling is unobserved (Phase 4b, marquee).** `outcome.rs:99`
  maps `exit_code == Some(75)` → `Outcome::Limit(None)`, and `lib.rs:14` forces
  `--stop-on-limit` for Kimi. This is **unit-tested only** (`outcome.rs:278`
  `classify_limit_on_exit_75`). A **real** billing-cycle / quota cap must be hit to
  promote it from source-grounded to observed-live — and that cannot be induced on
  demand without burning quota (the #155 blocker).
- **The auth stop (D6) needs a real logout.** A `kimi logout` "would have broken
  every subsequent validation run" in #155, so it was only proven positively. This
  pass force-reproduces it in a disposable session and re-logs in.
- **Token-vs-billing reconciliation (Phase 3) is a judgement call.** Ralphy sums
  **tokens** from `wire.jsonl`; Kimi bills a **subscription**. A human must place
  the run total next to Kimi's billing and state the unit mismatch.

## Cross-adapter reuse discipline (house rule)

Any gap that becomes an issue must **first verify in code what the other vendor
adapters already implement**, so we reuse rather than re-invent. The reuse seams
this capstone rides on, to check before proposing anything new:

- **Auth detection** → `is_kimi_auth_error` (`auth.rs:16`) is a thin call into the
  shared `ralphy_adapter_support::auth_error` — the same helper the other adapters
  use. A new auth-string gap extends the shared matcher, not a Kimi-local copy.
- **Program resolution** → `resolve_program("kimi")` (`command.rs:28`) is the
  shared `adapter_support` resolver (PATH + `~/.local/bin`). Cross-platform probe
  gaps land there, once, for every vendor.
- **Interactive scan** → `scan_kimi` (`ralphy-usage-scan/src/kimi.rs:37`) is a
  sibling of `scan_copilot` / `scan_gemini` / `scan_cursor`, all yielding
  `InteractiveRecord`. A scan gap is checked against those first.
- **Limit cadence** → `Outcome::Limit(None)` + ADR-0030 synthetic cadence +
  `--stop-on-limit` is the shared limit contract; Copilot/Cursor/OpenCode share the
  swallowed-limit failure-mode check (Phase 4b).

---

## Phase 0 — the auth stop (D6), force-reproduced — FREE

Actually reproduce the logged-out state — `kimi logout` in a disposable session,
re-login after. Confirm a clean run stops on `is_kimi_auth_error` (the real 0.28
signal `auth.login_required`, surfaced as `Kimi is not authenticated
(auth.login_required) — run \`kimi login\`…`), **exit 1, no child work
committed**, *not* a loop. Confirm no stale `.ralphy/plan.md` masks the stop
(the resume-trailer lesson). Then confirm auth-OK returns on re-login.
→ `docs/live/kimi-274-loggedout.log`

**Pass:** logged-out run stops on `auth.login_required`, no loop, no commit;
re-login returns to green; no stale plan masked it.

## Phase 1 — plan-only dry run (confirm + baseline) — one plan call

Already green in #155; re-run only to (a) confirm the `wire.jsonl` harvest
(`usage.rs`, D7) still recovers `input` / `cache_read` (`inputCacheRead`) /
`output` on the **0.28** store layout `~/.kimi-code/sessions/…/wire.jsonl`, and
(b) capture a clean per-run token baseline for the Phase 3 reconciliation. Confirm
`.ralphy/plan.md` is a real agent-written artifact; repo returned to base; empty
run branch removed. → `docs/live/kimi-274-plan.log`

**Pass:** real plan artifact; `wire.jsonl` harvest recovers all three token fields
on the 0.28 layout; per-run baseline recorded.

## Phase 2 — green run + stream-vs-diff delta — paid run to green

The `Stuck`/`Done` ladder and the encoding path under a subprocess-heavy run are
validated; add only the #251 progress-asymmetry check:

- **Green close.** Execute ends `exit 0` + HEAD-diff commit + `RALPHY_DONE_EXIT`;
  verify gate passes; issue closes green with the acceptance ledger written back.
- **Stream-vs-diff (D9-adjacent, #251).** Record the executor's reported change
  accounting next to the real `git diff` for shell-driven work, and confirm the
  **HEAD-diff `committed` guard** decided the outcome, not the stream.
- **Encoding (re-grounded from `PYTHONUTF8`).** Under FinCal's real
  `npm ci`/`prisma generate`/`next build`/`docker build`, confirm **zero decode /
  charmap crashes** and no Windows-console TUI trap. Record that 0.28 is not the
  Python build, so no `PYTHONUTF8` env assertion applies (drift ledger).

**Pass:** green close; stream-vs-diff recorded with `committed` guard deciding;
zero encoding crashes under the subprocess-heavy build.

## Phase 3 — usage & billing (the reconciliation #155 skipped) — HITL

1. **Interactive-session scan.** `ralphy usage` / daemon `GET /api/usage`
   (`scan_kimi`, `kimi.rs:37`) reports a **real token number** for interactive Kimi
   sessions, matching `wire.jsonl` **to the digit**. Confirm **no session with a
   store row reports `null`**, and **no session without a row reports a fabricated
   number**. Ephemeral daemon (`RALPHY_DAEMON_DIR=<tmp>`) so
   `daemon-require-login` is untouched.
2. **The unit mismatch (HITL).** Put Ralphy's per-run token total (and its
   ADR-0034 USD list-price projection) next to **Kimi's subscription billing** for
   the same run. State plainly: Ralphy's `$` is a metered-API counterfactual, *not*
   what the subscription charges. Record whether the per-issue total covers **every**
   invocation or under-reports one (the Cursor #269 shape).

**Pass:** the scan holds (real number, never `null`, never fabricated); a table
comparing tokens vs Kimi billing with the unit mismatch attributed.

## Phase 4 — one-shot / triage flows (confirm the surface)

Re-confirm each one-shot builder (`triage`, `diagnose`, `draft-issues`) carries
the same child-environment contract and the `.ralphy/skills` container (D8) as the
run path — the triage surface is not a hole in the run-path protections. Record the
per-issue token cost vs another vendor for the ADR-0038 budget.
→ `docs/live/kimi-274-triage.log`

**Pass:** triage verdict live; one-shot builders carry the run-path hardening;
`.ralphy/skills` container present; a per-issue budget multiple recorded.

## Phase 4b — the exit-75 ceiling (D9) — the marquee phase — HITL

Hit a **real** Kimi limit (billing-cycle / quota cap; [[opencode-silent-quota-timeout]]
notes Kimi has one). Capture:

- The **exact exit code** — confirm **75** as the `RETRYABLE` source constant
  predicts, mapping to `Outcome::Limit(None)` (`outcome.rs:99`) + the ADR-0030
  synthetic cadence, with `--stop-on-limit` force-enabled for Kimi (`lib.rs:14`).
- The **exact message**, any reset hint, and whether a terminal record was present.
- **Crucially, confirm it is NOT swallowed** into a silent retry that burns the
  wall timeout (the OpenCode failure mode — `saw_error=false`). Kimi's clean exit-75
  is the *good* case; verify it actually arrives.

Promote the exit-75 mapping from source-grounded-and-unit-tested to
**observed-live**, or amend it if the real ceiling exits differently.
→ `docs/live/kimi-274-limit.log`

**Pass (or recorded-deferred):** real exit code + string captured, mapping
promoted; the limit not swallowed. If no ceiling arrives this pass, a maintainer
ruling keeps D9 provisional (the gemini AC6 pattern) — the ADR stays **proposed**.

## Phase 5 — host hygiene / residue audit

- **Creds/config unchanged.** Diff `~/.kimi-code/credentials/` and
  `~/.kimi-code/config.toml` before/after every run — **byte-identical**. Baseline
  `~/ralphy-274-baseline/`.
- **Nothing token-bearing in the tree.** `.ralphy/skills` gitignored
  (`.ralphy/.gitignore = *`); no `.agents/` / `.kimi/` / `.kimi-code/` residue in
  FinCal; session rows land only under `~/.kimi-code/sessions`.
- **Verify-gate hang follow-up.** Confirm #155's incidental hang (an orphaned
  `next dev` from a plan-authored `sh -c "… & kill $PID"`) is a **verify-command
  robustness** issue, not Kimi residue — and record whether the process-group reap
  follow-up landed (check code before filing anything new).
- **Unasked artifacts.** Record any log/temp/update file Kimi writes outside the
  workspace.

**Pass:** creds/config byte-identical; no token-bearing repo write; verify-hang
attributed; unasked artifacts catalogued.

## Phase 6 — cross-platform parity (the note was Windows-only)

Repeat **Phase 1** and a short execute on Linux/WSL, same `kimi-code` build if
possible. Confirm:

- The encoding path is a no-op on a UTF-8 locale and does **not** trigger any TUI
  (the re-grounded `PYTHONUTF8` claim — now "no charmap crash", not an env assert).
- The **exit-75 mapping is platform-identical**.
- `resolve_program("kimi")` finds the WSL install (`~/.local/bin/kimi` or the
  `kimi-code` bin — record the actual path; [[wsl-vendor-cli-probing]]: `which`
  negatives don't prove absence).

Record any divergence as version skew or a real platform difference.

**Pass:** Phase 1 mechanics byte-identical both platforms, or divergences
attributed.

## Phase 7 — Capture, restore, wrap-up (the deliverable)

- Write `docs/evidence/274-kimi-capstone-live.md` (H2 phase structure), embedding
  raw numbers/commands/log lines; per-command captures to
  `docs/live/kimi-274-<probe>.log`.
- **Rewrite `docs/adr/0028-kimi-revalidation.md` from a plan into a note**, folding
  in the `kimi 1.48.0 → kimi-code 0.28` migration as the opening; **flip its Status
  proposed→accepted**; capture D9 with the real exit code/string (or the
  recorded-deferred ruling).
- **Restore host:** creds/config byte-identical; FinCal back to base, no leftover
  run branches, `git status --porcelain` empty; scratch cleaned per
  [[fincal-lab-repo]] / [[evidence-discipline]].
- File follow-up issues for any **code** defect found — each first verifying the
  reuse seams above — under `## Follow-ups filed`.

**Pass:** evidence doc + logs committed on `feat/copilot` (**no new branch in
ralphy, no push, no PR** unless explicitly asked); plan rewritten into an accepted
note; host clean.

---

## Acceptance-criteria ledger (#274)

| AC | Criterion | Phase(s) | Kind |
|----|-----------|----------|------|
| 0 | Auth stop force-reproduced: `auth.login_required` stops (no loop), re-login returns | 0 | **HITL** (real logout) |
| 1 | Plan-only re-confirmed; `wire.jsonl` harvest recovers input/cache_read/output; baseline captured | 1 | mechanical |
| 2 | `Stuck`/`Done` ladder + encoding re-confirmed (zero charmap crashes); stream-vs-diff delta recorded | 2 | mechanical |
| 3 | `scan_kimi` real number (never `null`/fabricated); token-vs-billing mismatch stated | 3 | **HITL** |
| 4 | `triage`/`diagnose`/`draft-issues` carry the hardening + skills container; per-issue budget | 4 | mechanical |
| 4b | Real exit-75 ceiling captured; mapping promoted to observed-live; not swallowed | 4b | **HITL** |
| 5 | Creds/config byte-identical; no repo residue; verify-hang attributed; unasked artifacts | 5 | mechanical |
| 6 | Phase 1 + short execute on Linux/WSL; encoding no-op; exit-75 platform-identical; `resolve_program` finds the CLI | 6 | mechanical |
| 7 | Plan rewritten into a note; D9 captured with a real exit code/string; no push/PR any phase | 7 | mechanical |

## What fails the whole exercise outright (the plan's stop conditions)

1. A real Kimi limit reaching Ralphy as anything other than `Limit(None)` — or
   swallowed, burning the wall timeout (the OpenCode failure mode).
2. `ralphy usage` inventing a token number for a Kimi session with no `wire.jsonl`
   row.
3. The operator's `~/.kimi-code` credential or config differing before/after a run.
4. A remote push or an opened PR from any phase.

## Guardrails carried from house rules

- **FinCal** work runs in the lab ([[fincal-lab-repo]] — clean the trail after).
  **Ralphy** artifacts land on the current branch `feat/copilot` only — **no new
  branch without authorization**, no push, no PR (CLAUDE.md,
  [[no-new-branch-without-authorization]]).
- Every artifact in this trail is **English** ([[canonical-language-english]]).
- Screenshots are for browser-driven verification only, never terminal/CLI output
  ([[evidence-discipline]]).
- Any issue filed first **verifies the reuse seam in code** (cross-adapter
  discipline above) before proposing new work.
