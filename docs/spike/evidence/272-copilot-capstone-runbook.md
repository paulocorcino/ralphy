# Runbook — Copilot live-validation capstone (#272)

A trail-to-completion for the HITL capstone of the `ralphy-agent-copilot` vendor
(ADR-0041, issue **#272**). ADR-0041 shipped its decisions from a **two-round
spike with no paid run to green and no quota ever exhausted**; this is the live,
end-to-end reconfirmation a human signs off before the adapter is called done.

This file is the **operational checklist**, not the evidence. On completion the
captured numbers, commands and log lines move into
`docs/evidence/272-copilot-capstone-live.md` (H2 phases mirroring
`251-cursor-capstone-live.md` and `265-gemini-capstone-live.md`), raw per-command
logs go to `docs/live/copilot-272-<probe>.log`, and the verdict-per-phase plus a
back-link land in
[docs/adr/0041-copilot-validation.md](../adr/0041-copilot-validation.md) — which
is itself rewritten from a plan into a note, with ADR-0041's Status flipped to
accepted and its D11 "unobserved" caveats replaced by the captured strings. The
decisions each phase exercises are D2–D12 in
[docs/adr/0041-copilot-adapter.md](../adr/0041-copilot-adapter.md).

**The plan file is the contract.** Its
[*What fails the whole exercise outright*](../adr/0041-copilot-validation.md)
section governs; those five stop conditions are reproduced in the ledger below.

---

## Resolved environment (captured 2026-07-22)

| Field | Value |
|-------|-------|
| Host OS | Windows 11 Pro 26200 |
| Copilot CLI | **1.0.72** (ADR-0041 cut against `1.0.71` — minor skew; a Phase 6 / version-note data point) |
| Copilot on PATH | `…/WinGet/Links/copilot` (WinGet shim, not `resolve_program`) |
| Ralphy binary | `C:/Dev/ralphy/target/release/ralphy.exe` (release, built 2026-07-22) · branch `feat/copilot` |
| `copilot login` identity | `paulocorcino` |
| Token env vars | `GH_TOKEN` **SET** · `COPILOT_GITHUB_TOKEN` unset · `GITHUB_TOKEN` unset · `COPILOT_HOME` unset |
| Vendor `config.json` | no `continueOnAutoMode`, no model keys → D11 preflight **pass** (this file is the Phase 5 byte-diff baseline) |
| Session store | `~/.copilot/session-store.db` present; **4 MB uncheckpointed `-wal`** → reads copy `.db` + `-wal` + `-shm` (D10) |
| Sim repo | `C:/Dev/FinCal` (`paulocorcino/FinCal`), base `master` @ `f15623d5`, tree clean, **no stale `.ralphy/plan.md`** |
| Capstone branch | `capstone/copilot-272` cut off `master` in FinCal (lab, authorized) |
| Baselines | `~/ralphy-272-baseline/` — `session-store.db{,-wal,-shm}.before`, `config.json.before`, `fincal-head.before` |

Standard invocation used throughout (Windows shown; WSL/Linux identical but
`./target/release/ralphy` and forward-slash paths):

```bash
./target/release/ralphy.exe run --repo C:/Dev/FinCal --issues <n> --agent copilot \
  --base-branch master [--dry-run] --verbose
```

---

## Why this is HITL, and what only a human can close

- It **spends real, metered work**: GitHub premium requests / AI credits per turn,
  and `request_multiplier` is per-model and *independent of the rate card* (D6) —
  one call can bill many premium requests.
- The **usage-vs-billing reconciliation (Phase 3.4)** is a judgement call: Ralphy
  sums **tokens** from `session-store.db` (source of truth, D10) while GitHub bills
  in **premium requests / AI credits**. Ralphy's `$` is an ADR-0034 metered-API
  counterfactual, *not* GitHub's bill. A human must place the run total next to the
  billing dashboard and state the mismatch plainly.
- **The limit is unobserved (Phase 4b).** `is_copilot_limit_text` is `(indicative)`;
  a real ceiling (Free-tier exhaustion or a premium-request cap) must be hit to
  promote it — and that cannot be induced on demand.

### Known scoping limitation — D8 wrong-identity

`GH_TOKEN` currently resolves to `paulocorcino` — the **same** identity as
`copilot login`. So the *wrong-identity attribution* consequence cannot be
demonstrated live without a throwaway token from a **second** account. D8 is
therefore validated at the **mechanism** level (all three token vars **absent**
from the spawned child's environment — the actual enforcement) and the attribution
consequence is recorded as a scoped limitation pending a second-account token, for
a maintainer ruling in the wrap-up (the gemini capstone's AC6 pattern).

---

## Phase 0 — the preflight refusals fire (D7, D8, D11 guard) — FREE

The gate that cannot produce all of these is decorative. None of these should
reach a paid child (D11 is asserted to stop *before* any spawn).

1. **Logged out.** With the operator logged out, a clean run stops with the
   `is_copilot_auth_error` string (`Copilot is not authenticated (no
   authentication information found) — run \`copilot login\`…`), **exit 1, no child
   work committed**. Confirm no stale `.ralphy/plan.md` masks it (the #271 lesson —
   already confirmed absent in the baseline). → `docs/live/copilot-272-loggedout.log`
2. **Wrong-identity scrub (D8).** With `GH_TOKEN` exported, a run must still
   authenticate as the `copilot login` identity. **Verify the spawned child's
   environment carries none of `COPILOT_GITHUB_TOKEN` / `GH_TOKEN` / `GITHUB_TOKEN`**
   (the enforceable claim). Attribution-to-wrong-account is out of reach here (same
   identity, see limitation above) — record it as deferred.
3. **MCP kill-switch receipt (D7).** A normal run emits
   `session.mcp_servers_loaded` with `github-mcp-server` `status: "disabled"`; the
   guard **fails the run** on `connected` and on an *absent* receipt for a clean
   exit. Confirm both the disabled receipt on the happy path and that flipping
   `copilot.allow_builtin_mcp_servers_i_understand_the_risk` drops
   `--disable-builtin-mcps` and suppresses the failure together. (Receipt is
   `ephemeral: true` — the scan must not reuse the stream's ephemeral filter.)
4. **`continueOnAutoMode` preflight (D11).** Set the key `true` in the vendor
   `config.json` and confirm `continue_on_auto_mode_violation` stops the run
   **before any child spawns** (costs no tokens); an absent/unparsable config is a
   pass. **Restore `config.json` byte-for-byte afterward** (Phase 5 baseline).

**Pass:** all four refusals fire; D11 stops pre-spawn; the D8 child-env scrub is
proven; config restored.

## Phase 1 — plan-only dry run (D2, D6, D10) — one plan call

```bash
./target/release/ralphy.exe run --repo C:/Dev/FinCal --issues <n> --agent copilot \
  --base-branch master --dry-run --verbose
```

- `.ralphy/plan.md` **written by the agent** (execution mode, not native `--plan`):
  feasibility verdict, `[verified]` acceptance ledger, `## Verify`, open steps,
  trailer.
- **Charter integrity (D2).** The full `prompt.execute.md` (≈24 KB before the issue
  body) arrives on **stdin**, not argv. Confirm a marker planted on the charter's
  **first *and* last** line both survive (no ~32 KB Windows argv truncation).
- **Minted session id (D10).** `--session-id <uuid>` **equals** the id the store
  rows key on — primary-key read, no snapshot-diff.
- **Priced cleanly** — no "unknown model"; the plan runs the operator's **current**
  model (D4); pricing from `token_details_json` / list price.
- **No `## Execution model:` line (D6)** — confirm the copilot planning charter
  emits no routing promise the executor would ignore.
- Repo returned to base branch; empty run branch removed.

**Pass:** real plan artifact, both charter markers survive, minted id keys the
rows, clean price, no exec-model line.

## Phase 2 — full non-dry-run (D3) — paid run to green

- **Green close on the sentinel.** Execute ends `exit 0` + HEAD-diff commit +
  `RALPHY_DONE_EXIT` on the real `result` envelope; verify gate passes; issue closes
  green with the acceptance ledger written back.
- **`codeChanges` is a false friend (D3).** Record the envelope's
  `codeChanges: {linesAdded, linesRemoved, filesModified}` next to the real
  `git diff` for a run whose work went through the **shell** tool, and confirm the
  adapter never consulted `codeChanges` (the HEAD-diff `committed` guard decided
  it). Copilot's single most dangerous record.
- **Classification ladder.** A deliberate `--max-minutes-per-issue` kill mid-run →
  `Timeout`, `saw_envelope=false`, `committed=true` → **`non_green`** (commits do
  not buy a green close without the clean-exit sentinel). Also drive a
  `RALPHY_BLOCKED_EXIT <reason>` path → **`Blocked`**.

**Pass:** green close; `codeChanges` proven a false friend against real diff;
`Timeout`/`non_green` and `Blocked` both reproduced.

## Phase 3 — usage & billing (D10) — the Cursor inversion — HITL

1. **Single run.** `result.usage.premiumRequests` (credits) captured from the
   envelope as cross-check; `assistant_usage_events` is the **source of truth**,
   **summed not keep-last**, WAL-copied before read. Field mapping
   (`input_tokens→input`, `output_tokens→output`, `cache_read_tokens→cache_read`,
   `cache_write_tokens→cache_creation`, `model→model`) confirmed on a real workload.
2. **Resumed session.** A resumed run on the same minted id — rows are
   **incremental** (two calls both `turn_index: 0`; `id` is the key), so the sum
   rule holds and Ralphy's run total exceeds any single row.
3. **Interactive-session scan (the inversion).** `ralphy usage` / daemon
   `GET /api/usage` (`scan_copilot`) enumerates interactive Copilot sessions and
   reports a **real token number** for each, matching the store to the digit — *not*
   `null`. Confirm **no session with a store row reports `null`**, and **no session
   without a row reports a fabricated number**. Exercise via an ephemeral daemon
   (`RALPHY_DAEMON_DIR=<tmp>`) so the operator's `daemon-require-login` posture is
   untouched.
4. **The unit mismatch (HITL).** Put Ralphy's per-run token total (and its USD
   list-price projection) next to the GitHub Copilot **billing dashboard** for the
   same run (premium requests / AI credits). State the mismatch plainly: Ralphy's
   `$` is an ADR-0034 counterfactual, not GitHub's bill; `request_multiplier` is
   per-model and independent of the rate card, so one call can bill many premium
   requests. Record whether the per-issue total covers **every** invocation or
   under-reports one (the Cursor #269 shape).

**Pass:** field mapping confirmed; resume rows incremental; the inversion holds
(real number, never `null`, never fabricated); a table comparing tokens vs the
billing dashboard with the mismatch attributed.

## Phase 4 — one-shot / triage flows + skills receipt (D9, D12)

- **Triage (live).** `ralphy triage --agent copilot` drives a real judgment through
  the native `--output-format json` stream and produces a verdict; confirm the path
  forwards attachments through `TriageRequest::image_paths` → `triage_issues` (D12)
  and that a triage with no images passes `&[]`.
- **`diagnose` / `draft-issues` via `init`.** Confirm the one-shot command builders
  carry the **same D7/D8 hardening** (MCP disabled, tokens scrubbed) as the run path
  — the triage surface is not a hole in the run-path protections.
- **Skills load receipt (D9).** A run materializes `.ralphy/skills`, exposes each
  into `.agents/skills/<name>`, and `session.skills_loaded` lists every required
  skill by resolved path; the guard asserts each **required** name is present (never
  set-equality — Copilot injects its own), failing closed on an absent receipt only
  for a clean exit. Confirm `.agents/skills/.gitignore` merges per-entry lines so the
  operator's sibling skills survive and the tree is clean for the next run.
- **Per-issue budget.** Record the same issue's token cost under `--agent copilot`
  vs another vendor, to feed an honest ADR-0038 per-issue default.

**Pass:** triage verdict live; one-shot builders hardened; each required skill
present in the receipt; a per-issue budget multiple recorded.

## Phase 4b — the limit, whenever it arrives (D11) — promote the detector — HITL

When a quota ceiling is hit (Free-tier exhaustion or a paid premium-request cap):

- Capture the **exact message**, exit code, any `Retry-After` / reset hint, and
  whether a terminal `result` envelope was present.
- Confirm it classifies as `Limit(None)` + the ADR-0030 synthetic cadence, and that
  `is_copilot_limit_text` matches the **real** string — promoting it from
  `(indicative)` to validated, or amending it if the real wording escapes the class
  matcher.
- **Crucially, confirm `continueOnAutoMode` did NOT swallow it** — no
  vendor-internal model-switch retry hid the limit and made Ralphy burn the wall
  timeout with `saw_error = false` (the OpenCode failure mode D11 names).

**Pass (or recorded-deferred):** the real string captured and the detector
promoted; or, if no ceiling arrives this pass, a maintainer ruling to keep D11
provisional (the gemini AC6 pattern).

## Phase 5 — host hygiene / residue audit

- **Config unchanged.** Diff `~/.copilot/config.json` (and `$COPILOT_HOME` if set)
  before/after every run: `continueOnAutoMode` and model keys **byte-identical**
  (the Cursor "config rewrite" failure must not recur). Baseline:
  `~/ralphy-272-baseline/config.json.before`.
- **Session-store growth bounded and outside the repo.** Rows land in
  `~/.copilot/session-store.db`; **nothing token-bearing** written into the target
  tree; WAL sidecars not left mid-checkpoint in a way the scan under-counts.
- **Background tasks.** `session.background_tasks_changed` fired six times in one
  spike probe with no disabling flag; confirm **no Copilot child outlives the run
  boundary** (the per-issue budget assumes process boundary = run boundary), and
  record any that does.
- **Unasked artifacts.** Record any debug log, update check, or temp file Copilot
  writes unbidden (the Cursor spike found an unasked temp debug log naming repos);
  confirm none names the operator's repositories outside the workspace.

**Pass:** config byte-identical; no token-bearing repo write; no surviving child;
unasked artifacts catalogued.

## Phase 6 — cross-platform parity (WSL)

Repeat **Phase 1** on a native WSL Ralphy build, same Copilot build if possible.
Confirm the init/auth record, envelope shape, minted-UUID session,
`session-store.db` topology, skill exposure and pricing are identical; record any
divergence and whether it is **version skew** between the two installs (Windows is
1.0.72; note the WSL build) or a real platform difference. Note git `core.autocrlf`
friction on `/mnt/c` if a Windows checkout is reused.

**Pass:** byte-identical mechanics both platforms, or divergences attributed.

## Phase 7 — Capture, restore, wrap-up (the deliverable)

- Write `docs/evidence/272-copilot-capstone-live.md` (H2 phase structure of
  `251-cursor-capstone-live.md`), embedding raw numbers/commands/log lines.
- Move per-command captures to `docs/live/copilot-272-<probe>.log`.
- **Rewrite `docs/adr/0041-copilot-validation.md` from a plan into a note** of what
  was run; **flip ADR-0041 Status proposed→accepted** and replace its D11
  "unobserved" caveats with the captured strings.
- **Restore host:** `config.json` byte-identical; FinCal back to `master` @
  `f15623d5`, no leftover run branches, `git status --porcelain` empty; capstone
  branch and baseline scratch cleaned per [[fincal-lab-repo]] / [[evidence-discipline]].
- File follow-up issues for any bug found; list under `## Follow-ups filed`.

**Pass:** evidence doc + logs committed on `feat/copilot` (**no new branch in
ralphy, no push, no PR** unless explicitly asked); validation note rewritten;
ADR-0041 accepted; host clean.

---

## Acceptance-criteria ledger (#272)

| AC | Criterion | Phase(s) | Kind |
|----|-----------|----------|------|
| 0 | All preflight refusals fire (logged-out, D8 scrub, D7 receipt, D11 pre-spawn) | 0 | mechanical |
| 1 | Plan-only: real artifact, charter markers survive, minted id keys rows, clean price, no exec-model line | 1 | mechanical |
| 2 | Full run green on sentinel; `codeChanges` false friend; `Timeout`/`non_green` + `Blocked` | 2 | mechanical |
| 3 | Usage semantics + the Cursor inversion (real token number, never `null`/fabricated); token-vs-billing mismatch stated | 3 | **HITL** |
| 4 | `triage` live; one-shot D7/D8 hardened; D9 skills receipt; per-issue budget multiple | 4 | mechanical |
| 4b | Real quota ceiling captured; `is_copilot_limit_text` promoted; `continueOnAutoMode` didn't swallow it | 4b | **HITL** |
| 5 | Config byte-identical; no token-bearing repo write; no surviving child; unasked artifacts | 5 | mechanical |
| 6 | Phase 1 repeated on WSL; divergence attributed | 6 | mechanical |
| 7 | Validation note rewritten; ADR-0041 accepted; no push/PR any phase | 7 | mechanical |

## What fails the whole exercise outright (the plan's stop conditions)

1. Any `session.mcp_servers_loaded` showing a builtin server `status: "connected"`.
2. A run authenticating as the wrong GitHub identity via a leaked
   `GH_TOKEN`/`GITHUB_TOKEN`/`COPILOT_GITHUB_TOKEN` (D8).
3. `~/.copilot/config.json` differing in `continueOnAutoMode` or any model key
   across a run.
4. `ralphy usage` inventing a token number for a session with no store row.
5. A remote push or an opened PR from any phase.

## Guardrails carried from house rules

- **FinCal** work runs on `capstone/copilot-272` (lab, [[fincal-lab-repo]] — clean
  the trail after). **Ralphy** artifacts land on the current branch `feat/copilot`
  only — **no new branch without authorization**, no push, no PR (CLAUDE.md,
  [[no-new-branch-without-authorization]]).
- Every artifact in this trail is **English** ([[canonical-language-english]]).
- Screenshots are for browser-driven verification only, never terminal/CLI output
  ([[evidence-discipline]]).
</content>
</invoke>
