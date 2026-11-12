# Live capstone — `ralphy run --agent cursor` end-to-end (#251)

Raw-evidence companion to the accepted note
[docs/adr/0042-cursor-validation.md](../../adr/0042-cursor-validation.md). Where that
file records the verdict per phase, this one carries the numbers, commands and log
lines behind it.

Host: Windows 11 (10.0.26200) + WSL Ubuntu-22.04. `cursor-agent --version` →
`2026.07.17-3e2a980` on **both** platforms. Account: **Cursor Pro**
(`about --format json` → `subscriptionTier: Pro`).
Lab repo: `C:\Dev\FinCal` (`paulocorcino/FinCal`), base `master`;
`.cursorindexingignore` (content `*`) committed on master.
Binary: `./target/release/ralphy.exe` from `feat/copilot`. Date: 2026-07-22.

Standard invocation:

```bash
./target/release/ralphy.exe run --repo C:/Dev/FinCal --issues <n> --agent cursor \
  --base-branch master [--dry-run] --verbose
```

## Phase 0 — refusals (D6, D8)

- **§1 missing opt-out** — `.cursorindexingignore` removed → ADR-0013 stop names the
  file, `*` content, opt-in key; `Applying change` = 0, no child spawned.
- **§2 logged out** — `cursor-agent status --format json` → `isAuthenticated:false`,
  **exit 0**. A clean run stops: `Error: Cursor is not authenticated — run`agent
  login` (or `cursor-agent login`) and retry` (exit 1); cursor.log carries
  `Error: Authentication required. Please run 'agent login' first, …`. Driven by the
  in-flight stderr matcher `is_cursor_auth_error`, NOT a `status` preflight
  (`probe_cursor_login` is wired only into `ralphy init`). **Masking bug (#271):** with
  a leftover `.ralphy/plan.md`, the first pass served the stale plan (`infeasible`, 0
  tokens, no cursor.log) instead of the login stop; removing it surfaced the stop.
- **§3 invalid key** — garbage `CURSOR_API_KEY` → `⚠ Warning: The provided API key is
  invalid.`, classified as auth (the third D8 string), not a generic "no plan".

## Phase 1 — plan-only dry run (#108)

Agent-written `.ralphy/plan.md` in execution mode (D9): feasibility verdict,
`[verified]` ledger, `## Verify`, open steps, trailer. Minted create-chat id ==
`system/init.session_id` (D10). `auto` priced cleanly ($0.93). Repo returned to master.
The `k3`/`+?` warning is a pre-existing kimi gap, not cursor. **Intermittent lingering
child**: 1-of-2 passes ended on `thinking/completed` with no `result` envelope, child
lingered to the idle watchdog (D3 "stream may end early").

## Phase 2 — non-dry-run

- **Green close (#117)**: execute `Done / saw_envelope=true`, verify gate passed, issue
  closed green. The planner had read the prior run's `verify-failure.md` and rewrote its
  `## Verify` to a tokenizer-safe form, citing the earlier failure (learning loop).
- **#268 (found + fixed)**: the first attempt (#116) was blocked by the verify gate, not
  the adapter. The planner authored
  `sh -c "test \"$(git diff-tree --no-commit-id --name-only -r HEAD)\" = \"README.md\""`.
  The no-shell tokenizer (ADR-0011) doesn't honor `\"` → mis-split argv → `sh: -c: line
  1: syntax error near unexpected token '('` → exit 2, ×3, repair budget burned,
  `verify_failed`. The committed README was correct; only the verify command was
  un-tokenizable. Fix `ce54f92` rejects nested-quote verify lines at parse time.
- **Mid-run kill (#108)**: `--max-minutes-per-issue 5` → `outcome=Timeout,
  saw_envelope=false, committed=true → non_green`; 101 commits did NOT buy a green close.
  Killed run reports 0 tokens (usage rides the envelope). D11 credit warning auto-fired.
- **Progress asymmetry**: stream `editToolCall` **+39/−12** vs `git diff` **208 files,
  +27,170/−120** — delta is unreported shell-driven merge/cherry-pick (D3/§C2).

## Phase 3 — usage (D11)

1. **Store has no tokens** (run 20260722-043752, #117): `meta.json` = `{schemaVersion,
   createdAtMs, hasConversation, updatedAtMs, cwd}` (no token field); `store.db` = one
   `blobs(id,data)` table, 140 protobuf/JSON blobs, only "token"+digit string is skill
   prose ("Cuts token usage ~75%…"); no `agent-transcripts` JSONL on this build.
2. **Resume is incremental** (direct 2-turn `--resume`, same UUID):

   | turn       | inputTokens | cacheReadTokens | outputTokens |
   | ---------- | ----------- | --------------- | ------------ |
   | 1          | 12941       | 5248            | 33           |
   | 2 (resume) | 100         | 18176           | 18           |

   Input collapses, cacheRead grows → incremental → D11 keeps the **sum** rule.
3. **Interactive scan** (daemon `GET /api/usage`, ephemeral `RALPHY_DAEMON_DIR` to avoid
   the operator's `daemon-require-login`): `scan_cursor` enumerated 68 cursor sessions
   (11 same-day), **all `tokens: null`** — never 0, never invented.
4. **Unit mismatch** (dashboard CSV vs envelopes, exact matches):

   | pass | input | cacheRead | output | Cost (Pro) |
   | ------ | ------- | ----------- | -------- | ----------- |
   | #117 plan | 43169 | 242560 | 5635 | Included |
   | #117 execute | 24629 | 136960 | 2841 | Included |
   | #117 consolidate | 33398 | 337152 | 5444 | Included |

   Ralphy's run total (455 794 = 67798/379520/8476) matches Cursor **to the digit** for
   plan+execute. But every Pro event is `Cost = "Included"` — ralphy's `$0.38` is modeled.
   **#269:** the 375 994-token consolidate pass is a real event NOT in ralphy's per-issue
   total.

## Phase 4 — foreign harvest (D12)

- Harvest floor ≈ **15 679 input tokens/invocation** (trivial "OK" probe); ~100% of a
  trivial task's input, ~17% of a real plan's; Phase 1 plan cacheRead = 1 264 640.
- **Cross-vendor** (same one-line issue on `--agent claude`, #118): claude total 175 904
  (fresh input 377 — no auto-harvest) vs cursor #117 831 048 → **~4.7×** total, **~42×**
  uncached input. #270 recommends a harvest-aware per-issue budget (ADR-0038).

## Phase 4b — the limit (D13)

Free-tier ceiling: `ActionRequiredError: You've hit your usage limit  Get Cursor Pro for
more Agent usage…`, exit 1, **no reset hint, no `result` envelope**. The plan-phase limit
arrives as a **bare** `ActionRequiredError:` stderr line (no JSON result) — the shape the
D13 spike never handled; `cca8d0a` folds it into `vendor_error`. Fixture
`crates/ralphy-agent-cursor/fixtures/usage-limit-stderr-2026-07-22.log`.

## Phase 5 — cross-platform parity (WSL)

Phase 1 repeated on WSL (native ralphy build, `CARGO_TARGET_DIR=~/ralphy-target-wsl`).
Same CLI build both platforms → parity check. Byte-identical on every mechanic: init
(`apiKeySource=login, model=Auto, permissionMode=default`, minted UUID), envelope
(`cacheWrite=0`, incremental), `auto` pricing ($0.53), skill harvest, D11 warning. Only
difference: WSL judged #108 infeasible (FinCal app tree drifted to docs-only) — a correct
feasibility read, not a platform divergence. `permissionMode` is `default` on both despite
`--force` (the `"force"` in `outcome.rs`'s fixture is synthetic). Friction: WSL git on
`/mnt/c` needed `core.autocrlf=true` to see the CRLF checkout as clean.

## Follow-ups filed

\#268 verify-gate nested-quote tokenizer (fixed `ce54f92`) · #269 per-issue under-count
(consolidate pass dropped) · #270 harvest-aware budget (ADR-0038) · #271 logged-out
stale-plan masks the auth stop.
