# Copilot adapter — live end-to-end validation plan

Companion to [ADR-0041](./0041-copilot-adapter.md). Like the Cursor, Kimi and
OpenCode validation notes ([0042-validation](./0042-cursor-validation.md),
[0028-validation](./0028-kimi-validation.md),
[0005-validation](./0005-opencode-validation.md)), this file has two lives: it is
**now the plan the capstone must execute**, and it will be rewritten into the note
that execution produced. It is the Copilot analogue of the Cursor capstone
([#251](https://github.com/paulocorcino/ralphy/issues/251)), and it deliberately
carries the phases that capstone taught us the earlier two notes were missing.

It exists because ADR-0041 shipped its decisions from a **two-round spike, with no
paid workload run to green and no quota ever exhausted**. Three things a spike
cannot settle are settled here:

- **D11 (limits) is entirely unobserved.** The spike induced no exhaustion, so
  `is_copilot_limit_text` is `(indicative — refine against a captured limit)` and
  the `Limit(None)` + ADR-0030 mapping rests on absence of evidence. Whether the
  `continueOnAutoMode` preflight actually keeps a real limit from being swallowed
  — the same failure mode that makes OpenCode burn a 60-minute timeout — is
  unproven.
- **The two currencies were never reconciled against a bill.** Copilot's stream
  reports **AI credits** (`premiumRequests`) while `session-store.db` reports
  **tokens**; ADR-0041 D10 picks the database as source of truth and prices tokens
  at the *underlying vendor's* USD list price as an ADR-0034 counterfactual. Nobody
  has yet put Ralphy's summed number next to the GitHub billing dashboard and
  stated the unit mismatch plainly.
- **Copilot keeps a token store, so `ralphy usage` counts interactive sessions —
  the exact inversion of Cursor.** #251 confirmed Cursor reports `tokens: null`;
  the Copilot equivalent must confirm the scan reports a **real, non-fabricated
  number** for an interactive session and that it matches the store.

Status: **accepted** — executed against `paulocorcino/FinCal` on 2026-07-22
([#272](https://github.com/paulocorcino/ralphy/issues/272)). Every phase below ran;
the observations, numbers and log lines are in the companion
[docs/evidence/272-copilot-capstone-live.md](../evidence/272-copilot-capstone-live.md)
(and the raw captures under `docs/live/copilot-272-*.log`). Phases 0, 1, 2a, 2b, 3,
3.4, 4, 5 and 6 are green; 2c fired the planner-infeasible block live (the executor
`RALPHY_BLOCKED_EXIT` path stays unit-validated); **Phase 4b — the real account-quota
ceiling — remains unobserved and is deferred by maintainer ruling** (the
`--max-ai-credits` cap self-throttles and is a different surface). ADR-0041 is moved
to accepted on this basis, and its D11 language is updated for what *was* observed.
The plan text below is preserved as the contract that was executed.

## What fails the whole exercise outright

Mirroring #251's stop conditions, adapted to Copilot's sharp edges:

- Any `session.mcp_servers_loaded` record showing a builtin server `status:
  "connected"` in any phase — the D7 kill switch is decorative if it can be
  bypassed (it must instead **fail the run**).
- A run authenticating as the wrong GitHub identity because `GH_TOKEN` /
  `GITHUB_TOKEN` / `COPILOT_GITHUB_TOKEN` leaked into the child (D8) — the silent
  failure the scrub exists to prevent.
- The operator's `~/.copilot/config.json` (or `$COPILOT_HOME/config.json`)
  differing before and after a run in `continueOnAutoMode` or any model key — the
  vendor's own next interactive session inheriting a Ralphy mutation.
- `ralphy usage` reporting a **fabricated** token number for a session that has no
  store row — a number invented is worse than a number absent.
- A remote push or an opened PR appearing from any run — the product-ethos breach
  the whole D7 posture guards against.

## Environment the run needs

- GitHub Copilot CLI (record the exact build; ADR-0041 was cut against `1.0.71`)
  on Windows, and again on WSL for Phase 6. Record whether the binary is on
  `PATH` or resolved through `resolve_program`.
- Auth: `copilot login` (OAuth). The three token env vars (`COPILOT_GITHUB_TOKEN`,
  `GH_TOKEN`, `GITHUB_TOKEN`) recorded as set/unset in the operator's shell, since
  D8 is exercised against exactly them.
- Plan/tier recorded (`copilot`'s catalog is plan-gated, D4) — note whether the
  account can pin a model at all, because a free account rejects every `--model`.
- Model: default posture (**no `--model`**, D4) for the baseline run so it runs the
  operator's own selection; one pinned-model run only if the tier allows it.
- Target repo: a real project with a working build and a feasible, unblocked issue
  and a deliberately-blockable one. `.ralphy/` must not already track a stale
  `plan.md` (the OpenCode #41 / Cursor #271 masking trap).
- Session-store baseline: copy `~/.copilot/session-store.db` (+ `-wal`/`-shm`) and
  the vendor `config.json` aside before the first run, to diff residue in Phase 5.

## Phase 0 — the preflight refusals fire (D7, D8, D11 guard)

The gate that cannot produce all of these is decorative.

1. **Logged out** — with the operator logged out, a clean run stops with
   `Copilot is not authenticated (no authentication information found) — run
   `copilot login`…` (the `is_copilot_auth_error` string), exit 1, **no child work
   committed**. Confirm no stale `.ralphy/plan.md` masks it (the #271 lesson).
2. **Wrong-identity scrub (D8)** — with a *different* account's `GH_TOKEN` exported,
   a run must still authenticate as the `copilot login` identity, never the token's.
   Verify the child's environment carries none of the three vars, and that the run
   report/commits are attributed to the operator, not the token owner. This is the
   silent-failure refusal the other notes never had.
3. **MCP kill-switch receipt (D7)** — a normal run emits
   `session.mcp_servers_loaded` with `github-mcp-server` `status: "disabled"`; the
   guard **fails the run** if it ever sees `connected`, and also on an *absent*
   receipt for a clean exit. Confirm both the disabled receipt on the happy path
   and that flipping the escape hatch
   (`copilot.allow_builtin_mcp_servers_i_understand_the_risk`) drops
   `--disable-builtin-mcps` and suppresses the failure together.
4. **`continueOnAutoMode` preflight (D11)** — set the key to `true` in the vendor
   config and confirm `continue_on_auto_mode_violation` stops the run **before any
   child spawns** (costs no tokens); an absent/unparsable config is a pass.

## Phase 1 — plan-only dry run

- `.ralphy/plan.md` written by the agent (execution mode, not a native `--plan`):
  feasibility verdict, `[verified]` acceptance ledger, `## Verify`, open steps,
  trailer.
- **Charter integrity (D2)** — the full `prompt.execute.md` (≈24 KB before the
  issue body) is delivered on **stdin**, not argv; confirm a marker planted on the
  charter's first *and* last line both survive, so nothing was truncated at the
  ~32 KB Windows argv ceiling.
- Minted `--session-id <uuid>` **equals** the id the store rows key on (D10) — the
  primary-key read, no snapshot-diff.
- Priced cleanly — no "unknown model" for the copilot pass; the plan runs the
  operator's **current** model (D4) and the run prices from
  `token_details_json` / list price.
- The copilot planning charter must **not** emit a `## Execution model:` line (D6);
  confirm the plan carries no routing promise the executor would ignore.
- Repo returned to base branch; empty run branch removed.

## Phase 2 — full non-dry-run

- **Green close reaching the sentinel** — execute ends `exit 0` + HEAD-diff commit
  + `RALPHY_DONE_EXIT`, on the real `result` envelope (D3); the verify gate passes;
  the issue closes green with the acceptance ledger written back.
- **`codeChanges` is a false friend (D3)** — record the envelope's
  `codeChanges: {linesAdded, linesRemoved, filesModified}` next to the real
  `git diff` for a run whose work went through the **shell** tool, and confirm the
  adapter never consulted `codeChanges` (the HEAD-diff `committed` guard did the
  work). This is #251's stream-vs-diff delta, made concrete for Copilot's most
  dangerous record.
- **Classification ladder** — a deliberate `--max-minutes-per-issue` kill mid-run:
  `Timeout`, `saw_envelope=false`, `committed=true` → `non_green`; commits do not
  buy a green close without the clean-exit sentinel. Also drive a
  `RALPHY_BLOCKED_EXIT <reason>` path → `Blocked`.

## Phase 3 — usage & billing (D10) — the Cursor inversion

1. **Single run** — `result.usage.premiumRequests` (credits) is captured from the
   envelope as a cross-check; `assistant_usage_events` is the **source of truth**,
   summed not keep-last, WAL-copied before read. Field mapping
   (`input_tokens→input`, `cache_read_tokens→cache_read`, …) confirmed against a
   real workload.
2. **Resumed session** — a resumed run on the same minted id: confirm rows are
   **incremental** (two calls both `turn_index: 0`; `id` is the key) so the sum
   rule holds and Ralphy's run total exceeds any single row.
3. **Interactive-session scan (the inversion)** — `ralphy usage` /
   the daemon `GET /api/usage` (`scan_copilot`) enumerates interactive Copilot
   sessions and reports a **real token number** for each, matching the store to the
   digit — *not* `null`. Confirm no session with a store row reports `null`, and no
   session **without** a row reports a fabricated number. Exercise via an ephemeral
   daemon so the operator's `daemon-require-login` posture is untouched.
4. **The unit mismatch** — put Ralphy's per-run token total (and its USD list-price
   projection) next to the GitHub Copilot **billing dashboard** for the same run,
   which is denominated in **premium requests / AI credits**. State the mismatch
   plainly: Ralphy's `$` is a metered-API counterfactual (ADR-0034), not GitHub's
   bill; `request_multiplier` is per-model and independent of the rate card (D6),
   so one call can bill many premium requests. Record whether the per-issue total
   covers every invocation or under-reports one (the Cursor #269 shape).

## Phase 4 — one-shot / triage flows + skills receipt

The element #251 skipped and the operator asked to include.

- **Triage (live)** — `ralphy triage --agent copilot` drives a real judgment
  through the native `--output-format json` stream and produces a verdict; confirm
  the triage path forwards attachments through `TriageRequest::image_paths` →
  `triage_issues` (D12) and that a triage with no images passes `&[]`.
- **`diagnose` / `draft-issues`** — exercised via `ralphy init` on the same repo;
  confirm the one-shot command builders carry the same D7/D8 hardening (MCP
  disabled, tokens scrubbed) as the run path — the triage surface is not a hole in
  the protections the run path has.
- **Skills load receipt (D9)** — a run materializes `.ralphy/skills`, exposes each
  into `.agents/skills/<name>`, and `session.skills_loaded` lists every required
  skill by resolved path; the guard asserts each **required** name is present
  (never set-equality, since Copilot injects its own), and fails closed on an
  absent receipt only for a clean exit. Confirm `.agents/skills/.gitignore` merges
  per-entry lines so the operator's own sibling skills survive and the tree is
  clean for the next run.
- **Per-issue budget** — record the same issue's token cost under `--agent copilot`
  vs another vendor, to feed an honest per-issue budget default (ADR-0038) rather
  than one the operator discovers from a bill.

## Phase 4b — the limit, whenever it arrives (D11) — promote the detector

The capstone's central unknown. When a quota ceiling is hit (Free-tier exhaustion,
or a paid premium-request cap):

- Capture the **exact message**, exit code, any `Retry-After` / reset hint, and
  whether a terminal `result` envelope was present.
- Confirm it classifies as `Limit(None)` + the ADR-0030 synthetic cadence, and that
  `is_copilot_limit_text` matches the **real** string — promoting it from
  `(indicative)` to validated, or amending it if the real wording escapes the
  current class matcher.
- **Crucially, confirm `continueOnAutoMode` did not swallow it** — that no
  vendor-internal model-switch retry hid the limit and made Ralphy burn the wall
  timeout with `saw_error = false` (the OpenCode failure mode D11 names).

## Phase 5 — host hygiene / residue audit

The element both earlier notes and #251 under-covered, and Copilot has concrete
residue vectors ADR-0041 flagged as *not decided*.

- **Config unchanged** — diff `~/.copilot/config.json` (and `$COPILOT_HOME` if set)
  before/after every run: `continueOnAutoMode` and the model keys are byte-identical
  after a run (the Cursor "config rewrite" failure must not recur here).
- **Session-store growth is bounded and outside the repo** — the run's rows land in
  `~/.copilot/session-store.db`, nothing token-bearing is written into the target
  tree, and the WAL sidecars are not left mid-checkpoint in a way the scan
  under-counts.
- **Background tasks** — `session.background_tasks_changed` fired six times in one
  spike probe with no disabling flag found; confirm no Copilot child process
  outlives the run boundary (the per-issue budget assumes process boundary = run
  boundary), and record any that does.
- **Unasked artifacts** — record any debug log, update check, or temp file Copilot
  writes unbidden (the Cursor spike found an unasked temp debug log naming repos);
  confirm none names the operator's repositories outside the workspace.

## Phase 6 — cross-platform parity

Repeat Phase 1 on WSL (WSL-native Ralphy build). Confirm the init/auth record, the
envelope shape, the minted-UUID session, the `session-store.db` topology, the skill
exposure, and pricing are identical; record any divergence and whether it is
version skew between the two installs or a real platform difference. Note git
`core.autocrlf` friction on `/mnt/c` if a Windows checkout is reused.

## What would have failed this validation (to confirm none did)

- Any builtin MCP server `connected` in a `session.mcp_servers_loaded` record.
- A run attributed to a leaked `GH_TOKEN` identity rather than the `copilot login`
  operator.
- `~/.copilot/config.json` differing in `continueOnAutoMode` or a model key across
  a run.
- `ralphy usage` inventing a token number for a Copilot session with no store row.
- A `git push` or an opened PR from any phase.
