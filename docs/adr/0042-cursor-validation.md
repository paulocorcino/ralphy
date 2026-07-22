# Cursor adapter — live end-to-end validation note

Companion to [ADR-0042](./0042-cursor-adapter.md). Like the Kimi and OpenCode
validation notes ([0028](./0028-kimi-validation.md), [0005](./0005-opencode-validation.md)),
this records what *was* run. It began life as the plan the capstone had to execute
and is now the note that execution produced.

It exists because one decision was deliberately left provisional. **D11 (usage)
could not be settled from a spike.** The spike proved no local store carries tokens;
whether capturing `result.usage` from the stream is *sufficient* — across a resumed
session, a run that hits its budget, and a run the operator later inspects — is a
question only a real run against a real repository answers. Phase 3 answered it.

Status: **accepted.** The capstone ran against `paulocorcino/FinCal` on 2026-07-22
(issue [#251](https://github.com/paulocorcino/ralphy/issues/251)); every phase below
executed and passed. The run surfaced four follow-up issues — verify-gate tokenizer
[#268](https://github.com/paulocorcino/ralphy/issues/268) (fixed in `cca8d0a`/`ce54f92`),
per-issue under-count [#269](https://github.com/paulocorcino/ralphy/issues/269), harvest
budget [#270](https://github.com/paulocorcino/ralphy/issues/270), and logged-out
stale-plan masking [#271](https://github.com/paulocorcino/ralphy/issues/271) — none of
which invalidate the adapter; each is recorded in its phase. The raw numbers, commands
and log lines behind each phase are in
[docs/evidence/251-cursor-capstone-live.md](../evidence/251-cursor-capstone-live.md).

## Environment the run had

- Cursor Agent CLI **2026.07.17-3e2a980 on BOTH** Windows
  (`%LOCALAPPDATA%\cursor-agent\agent.cmd`) and WSL (`~/.local/bin/cursor-agent`) — the
  plan expected a `…07.16` Windows build, but the box had updated, so the "version skew"
  Phase 5 was written to probe did not exist (it became a version-parity check). Both are
  **off `PATH`** as D14 predicted, except `cursor-agent.cmd` is in fact resolvable on the
  Windows PATH despite `which` missing it.
- Auth: `agent login` (browser OAuth), credential at `%APPDATA%\Cursor\auth.json`.
- **Tier: Cursor Pro** (`about --format json` → `subscriptionTier: Pro`). Early phases
  hit Free-tier quota exhaustion; the Pro upgrade unblocked Phases 1–5. Pro changes the
  billing picture — see Phase 3 §4.
- Model: `--model auto` passed explicitly (D4). The `auto` route priced cleanly through
  the family normalization (`pricing/defaults.rs` prices `auto` directly).
- Target repo: `C:\Dev\FinCal` (`paulocorcino/FinCal`), base branch **`master`**. Run
  branches cut as `afk/run-*`.
- **`.cursorindexingignore` (content `*`) committed on master** before every non-Phase-0
  run. `worker.log` inspected: **no `Applying change` line anywhere** across the whole
  validation — D6 held.

## Phase 0 — the preflight gate refuses (D6, D8) ✅

1. **Missing opt-out** ✅ — with `.cursorindexingignore` removed, the run stops with the
   ADR-0013 message naming the file, its `*` content, and the opt-in key; `Applying
   change` = 0, no child spawned.
2. **Logged out** ✅ (with a nuance, [#271](https://github.com/paulocorcino/ralphy/issues/271)).
   With the operator logged out (`status --format json` → `isAuthenticated:false`, exit 0),
   a clean run stops with `Cursor is not authenticated — run `agent login`…` (exit 1). The
   stop is driven by the **in-flight stderr matcher** (`is_cursor_auth_error`), NOT the
   `status --format json` preflight the plan named — `probe_cursor_login` is wired only
   into `ralphy init`, not `ralphy run`. And a leftover `.ralphy/plan.md` **masks** the
   auth failure: the first pass served a stale plan (reported `infeasible`, 0 tokens, no
   cursor.log) instead of the login stop; removing it surfaced the stop on the next run.
   Filed as #271.
3. **Invalid API key** ✅ — a garbage `CURSOR_API_KEY` yields
   `⚠ Warning: The provided API key is invalid.`, classified as auth (not a generic
   "no plan") — the third vendor string D8 exists for.

## Phase 1 — plan-only dry run ✅

- `.ralphy/plan.md` written **by the agent** in execution mode (not `--mode plan`, D9):
  feasibility verdict, `[verified]` acceptance ledger, `## Verify`, open steps, trailer.
- Minted `create-chat` id **equals** `system/init.session_id` (D10).
- Priced cleanly — no "unknown model" for the cursor pass; the `k3`/`+?` warning that
  appears is a **pre-existing kimi gap** in project-cumulative totals, not a cursor
  failure.
- Repo returned to master; empty run branch removed.
- **Intermittent lingering child** (D3's "stream may end early", made concrete): a pass
  sometimes ends on `thinking/completed` with no terminal `result` envelope and the child
  lingers to the idle watchdog; sometimes it completes cleanly (~6.6 min). Seen 1-of-2.

## Phase 2 — full non-dry-run ✅

- **Green close reaching `DONE_SENTINEL`** ✅ — FinCal #117: execute ended
  `Done / exited_cleanly / saw_envelope=true`, the verify gate passed, the issue closed
  green (`is_error:false`, sentinel last). The run also validated ralphy's learning loop:
  the planner read the prior run's `verify-failure.md` and rewrote its `## Verify` to a
  tokenizer-safe form, citing the earlier failure.
- **[#268], found live:** the FIRST green attempt (#116) was blocked not by the adapter
  but by the verify gate — the cursor planner authored a defensive
  `sh -c "test \"$(git diff-tree …)\" = \"README.md\""`, and the no-shell verify
  tokenizer (ADR-0011) does not honor `\"`, mis-splitting it into a garbage argv that
  fails `exit 2` and burns the repair budget. The committed work was correct; only the
  verify command was un-tokenizable. Fixed (`cca8d0a` classified the related quota shape;
  `ce54f92` rejects nested-quote verify lines at parse time, mirroring #181).
- **Classification ladder** ✅ — a deliberate `--max-minutes-per-issue` kill mid-run:
  `outcome=Timeout, saw_envelope=false, committed=true → non_green`; 101 commits did NOT
  buy a green close (the Kimi precedent holds). The killed run reports 0 tokens (usage
  rides the envelope), and D11's credit-vs-token warning auto-fired.
- **Progress asymmetry measured** ✅ — stream `editToolCall` **+39/−12** vs actual
  `git diff` **208 files, +27,170/−120**; the delta is shell-driven merge/cherry-pick work
  the stream does not account for (D3/§C2 confirmed — the stream's number would be wrong by
  exactly the shell-driven work).

## Phase 3 — the usage question (D11) ✅

1. **Single run** ✅ — `result.usage` is captured from the envelope; the on-disk store
   carries no token count on a real workload: `meta.json` has no token field, `store.db`
   is 140 protobuf/JSON blobs whose only "token"+digit string is skill prose, and no
   `agent-transcripts` JSONL is written on this build. D11 stands unchanged.
2. **Resumed session** ✅ — a direct 2-turn `--resume` probe (same minted UUID) on the
   current build: turn 1 `input 12941 / cacheRead 5248`, turn 2 `input 100 / cacheRead
   18176`. Input collapses while cacheRead grows — **incremental**, so D11 keeps the
   **sum** rule; ralphy's run totals (far exceeding any single envelope) confirm it sums.
   Semantics did not change under the Pro/version change.
3. **Interactive-session scan** ✅ — the daemon's `GET /api/usage` (`scan_cursor`)
   enumerated 68 cursor interactive sessions (11 from the capstone day), **every one with
   `tokens: null`** — unavailable, never 0, never invented (contrast the claude synthetic
   entry's `{input:0,…}`). Exercised via an ephemeral daemon so the operator's
   `daemon-require-login` posture was untouched.
4. **The unit mismatch** ✅ — ralphy's per-run tokens match Cursor's dashboard **to the
   digit** for the counted passes (#117 = 455 794). But every Pro-tier event is
   `Cost = "Included"` (flat subscription, $0 marginal): ralphy's `$0.38` is a modeled
   token-price projection, not Cursor's bill. **[#269], found here:** the
   knowledge-consolidation invocation (375 994 real tokens, a distinct dashboard event) is
   NOT in ralphy's per-issue total (plan+execute only) — ralphy under-reports cursor spend
   per issue by a whole invocation.

## Phase 4 — the token cost of the foreign harvest (D12) ✅

- Harvest floor ≈ **15 679 input tokens per invocation** (78 foreign skills); ~100% of a
  trivial task's input, ~17% of a real plan's input, and once harvested the skills move
  into cacheRead and are re-read every turn (Phase 1 plan: cacheRead 1 264 640).
- **Cross-vendor multiple** — the same one-line-file issue on `--agent claude` (#118):
  claude 175 904 total tokens (fresh input 377 — it does not auto-harvest) vs cursor #117
  831 048 (plan+execute+consolidate) → **~4.7×** total tokens, **~42×** on uncached input.
  The delta is overwhelmingly the foreign-skill harvest.
- **[#270], recommendation:** Cursor needs a harvest-aware per-issue budget default
  distinctly higher than a non-harvesting vendor's — a budget tuned on another vendor reads
  wrong for Cursor. Feeds ADR-0038.

## Phase 4b — the limit, whenever it arrives (D13) ✅ (captured on Free tier)

The ceiling was hit during Free-tier exhaustion:
`ActionRequiredError: You've hit your usage limit  Get Cursor Pro for more Agent usage…`,
exit 1, **no reset hint**, **no `result` envelope**. Reset is not daily (still exhausted
the next day, before the Pro upgrade). The capstone also found the **plan-phase** Free-tier
limit arrives as a *bare* `ActionRequiredError:` stderr line with no terminal record —
the shape the D13 spike never handled — which the fold skipped ("produced no plan") until
`cca8d0a` folds a bare-stderr `ActionRequiredError` into `vendor_error`. D13 is now
validated against the real strings.

## Phase 5 — cross-platform parity ✅

Phase 1 repeated on WSL (Ubuntu-22.04, WSL-native ralphy build). Both installs are on the
**same** CLI build, so this became a parity check, and parity held on every mechanic: the
init/auth record (`apiKeySource=login, model=Auto, permissionMode=default`, minted-UUID
session), the envelope shape (`cacheWrite=0`, incremental usage), `auto` pricing, the
skill harvest, and the D11 credit-vs-token warning are byte-identical. The only difference
was the plan *content* — WSL judged #108 infeasible because the FinCal app tree had drifted
to docs-only (app code lives on unmerged `afk/*` branches) — a correct feasibility read of
the current tree, not a platform divergence. (`permissionMode` reports `default` on both
platforms despite `--force`; the `"force"` in `outcome.rs`'s fixture is synthetic.)
Environment friction: WSL git on `/mnt/c` needed `core.autocrlf=true` to see the CRLF
checkout as clean; a WSL-native repo avoids it.

## What would have failed this validation (none did)

- Any `Applying change` line in a `worker.log` during any phase — none seen.
- A `.cursorignore` written by Ralphy — never.
- `~/.cursor/cli-config.json` differing before/after a run in the four model keys — the
  Pro upgrade changed auth/tier fields, but the four model keys were unchanged.
- `ralphy usage` reporting a token number for an interactive Cursor session — it reports
  `null`, never a fabricated number.
