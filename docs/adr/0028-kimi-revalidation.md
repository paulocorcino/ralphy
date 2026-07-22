# Kimi adapter — deep re-validation plan (the #251 bar)

Companion to [ADR-0028](./0028-kimi-adapter.md) and a follow-up to the original
capstone note [0028-kimi-validation](./0028-kimi-validation.md) (issue
[#155](https://github.com/paulocorcino/ralphy/issues/155)). Like the Cursor
capstone ([#251](https://github.com/paulocorcino/ralphy/issues/251)), this file is
**now the plan** and will be rewritten into the note that execution produces.

The first note was already thorough — it drove a real repo to a **green close**,
found and fixed the headline Windows cp1252 crash (`PYTHONUTF8=1`, not the
`PYTHONIOENCODING` TUI trap), priced the native model, confirmed the
`.ralphy/skills` container (D8) and the token harvest from `wire.jsonl` (D7), and
ran **triage** live. So this capstone is narrow: it closes only the #251 dimensions
that note left on reasoning rather than observation.

- **The exit-75 limit (D9) was never induced.** "A real 429 could not be forced
  without burning quota" — the mapping `exit 75 → Limit(None)` is grounded in
  Kimi's `RETRYABLE = 75` source constant and unit-tested, but no real ceiling was
  ever hit. The [[opencode-silent-quota-timeout]] finding notes Kimi has a
  billing-cycle cap; this capstone hits it and captures the real shape.
- **Tokens were never reconciled against a bill.** The note recorded per-issue
  usage (`input 139 381 · cache_read 4 734 464 · output 30 549`) but never put it
  next to Kimi's subscription/billing.
- **The auth stop (D6) was never force-reproduced** — a `kimi logout` "would have
  broken every subsequent validation run", so auth-OK was only proven positively.
- **The interactive-session scan was not exercised**, and the run was **Windows-
  only** — yet `PYTHONUTF8` and exit-75 are the two most platform-shaped mechanics
  in the adapter.

Status: **proposed** — flips to accepted when D9's ceiling is captured with a real
exit code and string, and the cross-platform parity is recorded.

## What fails the whole exercise outright

- A real Kimi limit reaching Ralphy as anything other than `Limit(None)` — or
  worse, being swallowed and burning the wall timeout (the OpenCode failure mode).
- `ralphy usage` inventing a token number for a Kimi session with no store row.
- The operator's `~/.kimi` credential or config differing before/after a run.
- A push or opened PR from any phase.

## Environment the run needs

- `kimi` (record the exact build; the note ran `1.48.0`) resolved through
  `resolve_program("kimi")` (off `PATH`, `~/.local/bin`), on Windows and again on
  Linux/WSL.
- Auth: `kimi login` OAuth (`~/.kimi/credentials/kimi-code.json`); model
  `kimi-code/kimi-for-coding` passed with `-m` (D4).
- `PYTHONUTF8=1` set on every child (the note's fix) — Phase 6 confirms it is a
  no-op on a UTF-8 Linux locale and does **not** re-trigger the Textual TUI.
- Target repo with a real subprocess-heavy build (the note used FinCal: `npm ci`,
  `prisma generate`, `next build`, `docker build`) so the encoding path is
  exercised; `.ralphy/plan.md` not already tracked.
- A reachable **billing-cycle / quota ceiling** for Phase 4b, plus access to the
  session log where a limit would surface.

## Phase 0 — the auth stop (D6), force-reproduced

Actually reproduce the logged-out state this time — `kimi logout` in a disposable
session (re-login after) — and confirm the run stops on `is_kimi_auth_error`
(exit 1 + `LLM not set`) rather than looping, then that auth-OK returns on
re-login. Confirm no stale `.ralphy/plan.md` masks the stop.

## Phase 1 — plan-only dry run (confirm + baseline)

Already green; re-run only to confirm the `wire.jsonl` harvest (D7) still recovers
`input / cache_read / output` on the current build and to capture a clean per-run
token baseline for the Phase 3 reconciliation.

## Phase 2 — green run + stream-vs-diff delta

The `Stuck`/`Done` ladder and the `PYTHONUTF8` fix under a subprocess-heavy run are
validated; add only the #251 progress-asymmetry check — the executor's reported
change accounting next to the real `git diff` for shell-driven work, confirming the
HEAD-diff `committed` guard decided the outcome, not the stream. Re-confirm zero
charmap crashes.

## Phase 3 — usage & billing (the reconciliation the note skipped)

1. **Interactive-session scan** — `ralphy usage` / daemon `GET /api/usage`
   (`scan_kimi`) reports a **real token number** for interactive Kimi sessions,
   matching `wire.jsonl` to the digit; no session with a store row reports `null`,
   no session without a row reports a fabricated number. Ephemeral daemon so
   `daemon-require-login` is untouched.
2. **The unit mismatch** — put Ralphy's per-run token total (and USD projection)
   next to Kimi's billing for the same run. State plainly what Ralphy's `$` is (an
   ADR-0034 list-price counterfactual) versus what Kimi's subscription actually
   charges, and whether the per-issue total covers every invocation (the Cursor
   #269 under-report shape).

## Phase 4 — one-shot / triage flows (confirm the surface)

The note ran triage, diagnose and draft-issues; re-confirm each one-shot builder
carries the `PYTHONUTF8=1` contract and the `.ralphy/skills` container, and record
the per-issue token cost vs another vendor for the ADR-0038 budget.

## Phase 4b — the exit-75 ceiling (D9) — the marquee phase

Hit a **real** Kimi limit (billing-cycle / quota cap). Capture:

- The exact exit code — confirm it is **75** as the `RETRYABLE` source constant
  predicts, and that it maps to `Limit(None)` + the ADR-0030 cadence with
  `--stop-on-limit` force-enabled for Kimi.
- The exact message and any reset hint, and whether a terminal record was present.
- Crucially, confirm the limit is **not swallowed** into a silent retry that burns
  the wall timeout (the OpenCode failure mode) — Kimi's clean exit-75 is the good
  case; verify it actually arrives.

Promote the exit-75 mapping from source-grounded-and-unit-tested to
observed-live, or amend it if the real ceiling exits differently.

## Phase 5 — host hygiene / residue audit

- `~/.kimi` credentials and any config byte-identical before/after every run.
- Nothing token-bearing written into the target tree; `.ralphy/skills` gitignored
  (`.ralphy/.gitignore = *`), no `.agents/`/`.kimi/` residue in the repo.
- Confirm the note's incidental verify-gate hang (an orphaned `next dev` from a
  plan-authored `sh -c "… & kill $PID"`) is a verify-command robustness issue, not
  Kimi residue — and record whether the process-group reap follow-up landed.
- Record any unasked artifact Kimi writes outside the workspace.

## Phase 6 — cross-platform parity (the note was Windows-only)

Repeat Phase 1 and a short execute on Linux/WSL. Confirm `PYTHONUTF8=1` is a no-op
on a UTF-8 locale and does not trigger the "No Windows console found" TUI, the
exit-75 mapping is platform-identical, and the `resolve_program` probe finds
`~/.local/bin/kimi`. Record any divergence as version skew or a real platform
difference.

## What would have failed this validation (to confirm none did)

- A real Kimi ceiling arriving as anything but a clean exit-75 `Limit(None)`, or
  swallowed into a wall-timeout burn.
- `ralphy usage` inventing a number for a session with no `wire.jsonl` row.
- `~/.kimi` credentials/config mutated across a run.
- A push or opened PR from any phase.
