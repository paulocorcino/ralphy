# Copilot adapter — live-validation capstone (#272)

The HITL run-to-green of `ralphy-agent-copilot`, executed 2026-07-22 against a real
repository. Companion to [ADR-0041](../adr/0041-copilot-adapter.md); the plan that
drove it is [0041-copilot-validation.md](../adr/0041-copilot-validation.md) and the
operational trail is [272-copilot-capstone-runbook.md](./272-copilot-capstone-runbook.md).
Raw per-command captures are under `docs/live/copilot-272-*.log`.

This is the analogue of the Cursor (#251) and Gemini (#265) capstones. It settles the
three things ADR-0041's two-round spike could not: a paid workload run to green, the
two-currency reconciliation against a real bill, and the interactive-scan inversion.

## Environment

| Field | Value |
|-------|-------|
| Host | Windows 11 Pro 26200; WSL Ubuntu-22.04 (Phase 6) |
| Copilot CLI | **1.0.72 → 1.0.73** (self-updated mid-session; both observed) |
| Ralphy | `target/release/ralphy.exe` (Windows) · `~/ralphy-target-wsl/release/ralphy` (Linux) · branch `feat/copilot` |
| Identity | `copilot login` = `paulocorcino`; `GH_TOKEN` set = same identity (a classic `ghp_` PAT) |
| Sim repo | `C:/Dev/FinCal` (`paulocorcino/FinCal`), base `master`, capstone branch `capstone/copilot-272` |
| Scratch issues | #120 (feasible), #121 (triage), #122/#123 (timeout), #124 (blocked), #125 (WSL) |

## Phase 0 — the preflight refusals fire (D7, D8, D11)

- **Logged-out (D7 guard).** A genuine `copilot logout` was required (the OS credential
  store, not `COPILOT_HOME`, holds the OAuth token — `COPILOT_HOME=<empty>` does **not**
  log out). Logged out + tokens scrubbed, `copilot -p` printed
  `Error: No authentication information found.` (exit 1) — byte-identical to the spike's
  captured block, so `is_copilot_auth_error` (`"no authentication information found"`) is
  validated on 1.0.73. Through ralphy the run stopped with the exact
  `COPILOT_AUTH_ERROR_MSG`, exit 1, no child work, clean cleanup.
- **D8 token scrub — proven stronger than the note anticipated.** `GH_TOKEN` was set
  (`paulocorcino`), yet the logged-out child *errored* instead of authenticating. Since
  `GH_TOKEN` takes precedence over stored creds, the only way a logged-out child errors is
  if the token **never reached it** — so the scrub is proven live, the logged-out state
  making it observable. (Wrong-*identity* attribution stays out of reach without a
  second-account token; recorded as a scoped limit.)
- **D7 MCP receipt.** Every real run emitted
  `session.mcp_servers_loaded` → `github-mcp-server status: "disabled" source: "builtin"`;
  the guard passed on it, failed closed on none. Confirmed live on the plan, execute,
  consolidate and triage paths.
- **D11 `continueOnAutoMode` preflight.** With the key set `true` in the vendor config, the
  run stopped with `continue_on_auto_mode_violation` **before any child spawned** — proven
  by the fact that, logged out, the *D11* message won over the *auth* error (D11 is checked
  before the child is invoked). Config restored byte-identical afterward.

## Phase 1 — plan-only dry run (D2, D4, D6, D10)

Issue #120, `--dry-run`. Real plan artifact (`## Feasible: yes`, `## Done when`,
`## Verify`, `## Steps`, trailer). Priced **$1.13**, no unknown model.

- **D2 charter integrity.** The plan's first usage row carried `input_tokens = 23505` — the
  ~24 KB `prompt.execute.md` charter arrived on stdin intact (no ~32 KB argv truncation).
- **D4.** No `--model` passed (planning emit `model=` empty); ran the operator's default
  `claude-sonnet-5`.
- **D6.** Plan carries **no `## Execution model:` line**.
- **D10 — fold byte-exact.** The minted `--session-id b160d934…` keyed **9 rows** in
  `assistant_usage_events`, all `turn_index: 0` (so *id* is the key → sum, not keep-last).
  The sums matched ralphy's report to the digit:

  | field | store sum | ralphy |
  |-------|-----------|--------|
  | input_tokens | 290243 | up=290243 |
  | cache_read_tokens | 250641 | cr=250641 |
  | cache_write_tokens | 39381 | cw=39381 |
  | output_tokens | 2473 | out=2473 |

  Field mapping, `token_details_json` (rate card), and `total_nano_aiu` all present.

## Phase 2 — full run + the classification ladder (D3)

- **2a — green close.** Issue #120 executed to `outcome=Done`, `exited_cleanly=true`,
  `committed=true`, verify gate passed (a `python -c` byte check), issue **closed green**
  with the acceptance ledger written back. Commit `3c00c4a4`. The executor **resumed** the
  finalized Phase-1 plan (planning tokens = 0). $1.18.
- **D3 `codeChanges` is a false friend — proven.** The file was written via a `python -c`
  **shell** command. The real diff was `VALIDATION.md | 1 +`; the envelope's
  `codeChanges.filesModified` reported **`.ralphy/plan.md`** (the plan checkboxes), not
  `VALIDATION.md`. The adapter never consulted it — the HEAD-diff `committed` guard closed
  the run. Sentinel `RALPHY_DONE_EXIT` present ×3.
- **2b — Timeout → non_green.** Issue #123 (80 incremental commits), plan finalized
  uncapped then run with `--max-minutes-per-issue 1`. Killed at ~61 s:
  `outcome=Timeout, timed_out=true, committed=true` → the run closed **`non_green`** —
  commits did **not** buy a green close without the clean-exit sentinel. Tree-kill left
  **0** surviving Copilot children. (Note: the cap bounds each phase independently — a
  looser 2-min cap let fast Copilot finish 40 commits green.)
- **2c — Blocked.** Issue #124 (unbuildable: a required out-of-band secret). The **planner**
  caught it as `infeasible` *before* execute (arguably better — no execute tokens spent), so
  the executor's `RALPHY_BLOCKED_EXIT → Outcome::Blocked` path was not exercised live; it
  stays unit-validated (`classify_blocked_on_blocked_sentinel`). Engineering a
  plan-feasible/execute-blocked issue was not pursued (the planner probes the env thoroughly;
  more paid attempts, low marginal value).

## Phase 3 — usage & billing (D10) — the Cursor inversion

- **The inversion holds.** The daemon `GET /api/usage` (`scan_copilot`, ephemeral daemon so
  the operator's `daemon-require-login` posture was untouched) returned **27 Copilot
  interactive records, zero with null tokens** — the exact inverse of Cursor's `tokens:
  null` (#251). Every number matched the store to the digit (e.g. session `9128577d` =
  146382 input == store), `lower_bound: false` (exact, not a floor). Run-owned sessions
  (plan `b160d934`, execute `b4fedf7e` = 638895 exactly) are correctly de-duped into
  `records`, not `interactive`; a session **without** a store row produces **no** record
  (never fabricated).
- **Incremental rows.** The plan's 9 rows share one `session_id`, all `turn_index: 0` — the
  sum rule holds.
- **Under-report finding (Cursor #269 shape).** Ralphy's per-issue total for #120 (638895)
  is the **execute session alone**. The **end-of-run knowledge-consolidation** Copilot call
  (`9128577d`, 5 calls, **146382 input tokens**) is a real run-level cost the per-issue
  ledger and run `records` do **not** capture — only the interactive scan sees it. Triage is
  the same shape (session `9ba5dcf6`, 255415 input tokens for one issue).

### Phase 3.4 — the two currencies, against a real bill

The GitHub billing dashboard CSV for 2026-07-22:

| Meter | Value |
|-------|-------|
| **GitHub actual bill** | **215.93 AI credits = $2.16** (Claude Sonnet 5); +0.50 credits GPT-5.3-Codex (not this capstone) |
| **Store `total_nano_aiu`** (ralphy's D10 source of truth) | **204.02 credits** across 113 rows |
| **Ralphy USD counterfactual** (ADR-0034 list price) | ~$13 projected |
| Envelope `premiumRequests` | 1/run — legacy field, meaningless on the credits platform |

Stated plainly:

1. **Ralphy's `$` over-states GitHub's bill ~6.5×.** The plan cost ralphy **$1.13** but
   GitHub only **17.375 credits = $0.17**. ADR-0034 prices tokens at the vendor's metered
   list price; GitHub bills a bundled AI-credit rate ($0.01/credit buys far more tokens).
   The counterfactual is honest about *being* a counterfactual — now quantified.
2. **The store meter itself is a floor.** 204.02 vs 215.93 credits — it under-counts the
   real bill by ~5.5%, because *"hidden model work such as compaction counts toward credits
   but does not show as a visible assistant response"* (`copilot help limits`).
3. **`premiumRequests` is legacy.** GitHub has moved to AI credits; "premium requests" is the
   legacy platform. ADR-0041 D10's "premium requests" language is updated accordingly.
4. Monthly quota is 1500 credits; the capstone consumed ~216 (~14%).

## Phase 4 — one-shot / triage + skills receipt (D9, D12)

- **Triage (live).** `ralphy triage --agent copilot` produced a real verdict for #121:
  `bounce — comment, swap triage-agent → needs-info`, applied, exit 0 (session `9ba5dcf6`).
  D12's no-image path passes `&[]`.
- **One-shot D7/D8 hardening.** The shared `build_copilot_init_command` is unit-tested for
  the five blast-radius flags + the three-var scrub; live, the **consolidate** one-shot
  (which shares the builder) emitted the `status:"disabled"` receipt.
- **D9 skills receipt.** `session.skills_loaded` listed **16 skills** with resolved
  `.agents/skills/<name>/SKILL.md` paths; Copilot injected its own (`customize-cloud-agent`),
  confirming the guard checks **presence, not set-equality**. The `.agents/skills/.gitignore`
  merge is **per-entry** (`/reviewer`, `/setup-pocock`, `/staged-plan`) so the operator's
  committed sibling skills survive.

## Phase 4b — the limit (D11) — deferred as a documented open limit

No real account-quota ceiling was hit. The `--max-ai-credits 30` session cap is **not** a
reliable proxy: the model is *told* its remaining budget and **self-throttles**
(`"tight budget of 24 AI credits… be strategic"`), finishing before the cap blocks. And even
a session-cap surface (`"increase or unset the session limit"`) differs from account
exhaustion — which `is_copilot_limit_text` deliberately does not match. `is_copilot_limit_text`
therefore stays **class-validated** (unit tests over `rate limit exceeded` / `out of ai
credits` / `429`, and the auth/limit predicates proven disjoint) with the **real ceiling
unobserved** — a maintainer ruling keeps it deferred (the Gemini #265 AC6 pattern). D11's
`continueOnAutoMode` guard is separately proven (Phase 0.4).

## Phase 5 — host hygiene

- **Config byte-identical** across every run (`~/.copilot/config.json` vs baseline) — the
  Cursor "config rewrite" failure does not recur.
- **No token-bearing write into the repo tree** (the only `.ralphy/` hits are issue *text*
  quoting a secret *filename*, and `.ralphy/` is gitignored).
- **0 surviving Copilot children** after any run, including the killed one.
- **Store growth bounded and outside the repo** (~360 KB→620 KB inside `~/.copilot`);
  Copilot writes `process-*.log` under its own home, none in the repo.
- `session.background_tasks_changed` fired but left no surviving child.

## Phase 6 — cross-platform parity (WSL)

No Linux-native Copilot existed (only the Windows `copilot.exe` reachable via `/mnt/c`
interop, which ralphy-Linux does not use); installed `@github/copilot` on the WSL nvm node
→ **1.0.73 Linux**, then `copilot login`. Reusing the Windows checkout via `/mnt/c` hit the
runbook-predicted git autocrlf friction, so a **WSL-native clone** (`~/FinCal-wsl`) was used.
Plan-only parity on #125 was **identical** to Windows:

| Aspect | Windows | WSL |
|--------|---------|-----|
| D7 receipt | `status:disabled` | `status:disabled` |
| minted UUID = store key | `b160d934` | `0c1cc009` |
| D10 fold | up=290243 = store | up=231157 = store (8 rows) |
| store schema | `assistant_usage_events` | identical |
| AI-credit meter | `total_nano_aiu` | 15.31 credits |
| D6 no exec-model line | absent | absent |
| D4 operator model | claude-sonnet-5 | claude-sonnet-5 |
| skills exposure | reviewer/setup-pocock/staged-plan | identical |
| pricing | $1.13 clean | $0.92 clean |

Only difference is **platform-appropriate, not a divergence**: the plan's verify commands are
`sh -c "test …"` on Linux vs `python -c` on Windows (the agent adapts). The logged-out auth
string is identical on both. Both installs are 1.0.73 — no version skew.

## Verdict

Every phase executed against a real repository. **0, 1, 2a, 2b, 3, 3.4, 4, 5, 6** are green.
**2c** fired the planner-infeasible block live (executor `RALPHY_BLOCKED_EXIT` unit-validated).
**4b** — the real account-quota ceiling stays **unobserved**, deferred by maintainer ruling;
`is_copilot_limit_text` is class-validated. ADR-0041 is moved to **accepted** on this basis.

## Follow-ups filed

- (none required — no adapter defect surfaced; the under-report of run-level sessions
  (consolidation, triage) in the per-issue ledger and the ~5.5% store-vs-bill floor are
  documented above as accounting facts, not bugs.)
</content>
