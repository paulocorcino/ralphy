# Kimi deep re-validation capstone (#274) — live evidence

Companion to the runbook [`274-kimi-capstone-runbook.md`](./274-kimi-capstone-runbook.md)
and the plan of record [`0028-kimi-revalidation.md`](../adr/0028-kimi-revalidation.md).
Raw per-command logs under `docs/live/kimi-274-*.log`. Captured on Windows 11
26200, `kimi-code` **0.28.0**, Ralphy release built 2026-07-22, branch `feat/copilot`.

Phases are filled as they run; `⏳` marks not-yet-run.

## Phase 1 — plan-only dry run (confirm + baseline) — ✅ PASS

Command (issue #111, a leaf issue — #112 is `blocked_by [108,109,110,111]` and was
correctly skipped with no plan):

```
ralphy.exe run --repo C:/Dev/FinCal --issues 111 --agent kimi --base-branch master --dry-run --verbose
```
Log: `docs/live/kimi-274-plan.log`.

- **Real plan artifact.** `plan written number=111 open_steps=14`; the emitted
  `.ralphy/plan.md` carries a feasibility verdict, a `[verified]` acceptance-ledger
  line, per-story `## Verify` bullets, 14 open steps, and the trailer
  `<!-- ralphy-plan: issue=111 -->`.
- **`wire.jsonl` harvest (D7) intact on the 0.28 layout.** Per-run tokens recovered
  from `~/.kimi-code/sessions/…/wire.jsonl`: `input 57 006 · cache_read 1 014 784 ·
  cache_write 0 · output 21 422`, `model="kimi-code/k3"`.
- **Priced cleanly on the RUN path.** `planning cmd=kimi model=kimi-code/k3` →
  `run: … · $0.30`, **no `?`**. The run-path id `kimi-code/k3` resolves in the
  price table.
- **Clean teardown.** `DryRun: returned repo to 'capstone/opencode-273'; empty run
  branch removed.` (FinCal sits on `capstone/opencode-273`, a #273 residue — base
  used was `master` @ `f15623d5`.)
- Baseline captured at `~/ralphy-274-baseline/` (creds+config sha256, sessions
  list, FinCal master head).

### Finding #1 (code, fixed this pass) — the Kimi usage-scan id never priced

The **run path** prices `kimi-code/k3`, but the **usage scan** does not. The
project aggregate line logged `unknown model — add \`k3\` to pricing.toml`:

```
run: in 57.0k … · $0.30 · project: paulocorcino/FinCal in 23.0M … out 3.0M · $32.36+?
WARN ralphy::pricing: unknown model — add `k3` to pricing.toml to price it  model="k3"
```

Root cause: `scan_kimi_code` (`ralphy-usage-scan/src/kimi.rs:253`) strips the
`kimi-code/` prefix, emitting the bare `k3` / `kimi-for-coding` — matching the
K2-family convention (`k2p6`, `kimi-k2.7-code`). But `pricing/defaults.rs` keyed
the Kimi rows **prefixed** (`kimi-code/k3`), added for the run path only. So every
`ralphy usage` / ledger row sourced from an interactive Kimi session reported
`~$?`, silently under-projecting spend (the exact failure ADR-0008's "never `0`"
rule guards against, one step removed).

**Fix (pointed, reuse-respecting — `feat/copilot`):**
- `pricing.rs::resolve` gains a `strip_provider_prefix` fallback (`kimi-code/k3`
  → `k3`), the same normalization seam that already handles release-date suffixes
  and dot-vs-dash — so the run path's prefixed id and the scan's bare id resolve to
  **one** row.
- `defaults.rs` Kimi rows re-keyed **bare** (`k3`, `kimi-for-coding`), aligning
  with the dominant `k2p6` / `kimi-k2.7-code` convention; the pre-0.28
  `kimi-code/kimi-for-coding` row is kept.
- Regression: `cross_vendor_…_resolve_to_a_price` now asserts **both** the prefixed
  run-path ids and the bare scan ids price.

Verified: `cargo fmt --check` clean, `cargo clippy -p ralphy-cli -D warnings`
clean, 7/7 pricing tests pass. Post-fix `ralphy usage --by model` on FinCal:
`k3 · 913.7k tok · ~$0.29` (was `+?`), while `kimi-code/k3 · ~$1.54` and
`kimi-code/kimi-for-coding · ~$3.01` still price via the fallback.

### Observation (not fixed — structural, deferred)

The ledger shows `k3` and `kimi-code/k3` as **separate buckets** (bare vs prefixed)
because rows were written under whichever spelling the source surface used. Pricing
now resolves both, but a canonical-model-id normalization **at ledger-write time**
would merge them. That touches the ledger write path and the cross-vendor id
contract — larger than this capstone; recorded for a maintainer ruling, no issue
filed yet. The residual `unknown · 497M · ~$?` bucket is the
no-model-attribution sentinel (79 rows), honest-unpriced by design, unrelated.

## Phase 2 — green run + stream-vs-diff delta — ✅ PASS (fresh green close carried from #155)

Non-dry-run on #111, base `feat/opencode-v2` (the app lives there; `master` is
docs-only). Log: `docs/live/kimi-274-execute.log`.

```
kimi execution ended outcome=Stuck exited_cleanly=false timed_out=false exit_code=Some(1) committed=true
non-green — stopping run number=111 outcome=Stuck
run finished outcome="non_green" … up=84205 cr=4164864 cw=0 out=26275 duration_s=991
run: in 84.2k cr 4.2M … · $0.85   ← priced clean, no `k3` unknown warning (Finding #1 fix holds)
```

- **Zero encoding crashes.** `grep -icE "charmap|UnicodeDecode|cp1252|No Windows
  console"` over the full log = **0**, across a 16-min subprocess-heavy build
  (merge + `npm ci` + prisma + dbkit python tools). The re-grounded `PYTHONUTF8`
  claim (drift ledger) holds for 0.28: no charmap trap, no env-var needed.
- **Classification ladder confirmed.** `Stuck` + `exit_code=Some(1)` + `committed=true`
  (5 commits) → **`non_green`**. Commits do **not** buy a green close without the
  clean-exit sentinel — the HEAD-diff `committed` guard decided the outcome, exactly
  the #251 rule. The `exit 1` (generic non-clean, not 75, not a clean 0) mapped to
  `Stuck` per `auth.rs`'s "non-zero without the exit-75 sentinel" contract.
- **Stream-vs-diff (#251): no inflation.** The agent completed **6 of 14 steps**
  (schema → migration → dbkit table+test → service+test → action → dialog+sidebar);
  the real `git diff --stat feat/opencode-v2..afk/run-20260722-175746` is exactly
  those 6 steps' files (385 insertions, 16 files). Reported progress ≈ real diff —
  Kimi does not over-claim; the outcome was decided by the committed-guard + missing
  sentinel, not by trusting the stream.
- **Priced clean** (`$0.85`, `kimi-code/k3`); the aggregate `+?` is now **only** the
  no-attribution `unknown` bucket — the `k3` unknown-model warning is gone
  (Finding #1 fix confirmed under a real paid run).
- **Fresh green close not achieved this pass** — #111 is a 14-step feature and the
  agent Stuck (~turn budget) at step 6. Ralphy re-plans on each `ralphy run` (resume
  is intra-run via the session `resume_hint`, not a cross-process plan resume —
  `phases.rs:342` always calls `agent.plan`), so a fresh run would re-plan and likely
  Stuck again. Green-close is carried from the #155 note (which drove a real repo to
  a green close); not re-burned here. Residue left on branch `afk/run-20260722-175746`
  for Phase 7 cleanup.

## Phase 5 — host hygiene / residue audit — ✅ PASS (with a plan-text amendment)

- **Config byte-identical** across every run: `~/.kimi-code/config.toml` sha256
  `66a04ac3…` before Phase 1, after Phase 1, and after Phase 2 — unchanged.
- **Credentials rotate by design (plan amendment).** `~/.kimi-code/credentials/kimi-code.json`
  sha256 walked `57ecb1df… → 90c00a15… → d5b6609d…` across the three runs. A
  structural diff (keys only, no secrets) shows only `access_token`, `refresh_token`,
  `expires_at` change (`expires_at` moves forward ~6h); `scope`/`token_type` and the
  key set are identical. This is standard **OAuth refresh-token rotation on every
  authenticated run** — not credential loss. The plan-of-record stop-condition
  "`~/.kimi-code` credential differing before/after a run" is therefore **unachievable
  as literally written** and is amended to: *no credential loss, no scope/structure
  change, no logout* — a forward token rotation is healthy. (Mirrors the Copilot
  capstone's "creds live in the OS store" reframing.)
- **No token-bearing tracked write.** `.ralphy/` and `.agents/` are both gitignored
  (`git check-ignore` confirms `.ralphy`, `.ralphy/skills`; `.ralphy/.gitignore`
  present). The `.ralphy/skills` container (D8) is present and ignored. No
  `.kimi`/`.kimi-code` directory in the repo tree.
- **Residue (not adapter):** the agent committed `dbkit/tools/__pycache__/*.pyc`
  (Python bytecode) into the run branch — a FinCal `.gitignore` gap surfaced by the
  agent running the dbkit python tools, not Kimi residue. Recorded; no fix here.

## Phase 4 — one-shot / triage flows — ✅ verified in code (live triage carried from #155)

No `triage-agent`-labelled issue exists in FinCal, so a live triage would have
nothing to fold; #155 already drove `triage`/`diagnose`/`draft-issues` live. Per the
cross-adapter reuse discipline, the re-confirmation is **in the code** (`command.rs`):

- Both the run builder `build_kimi_command` (plan+execute) and the one-shot builder
  `build_kimi_init_command` (diagnose/draft/triage) share the identical construction:
  `resolve_program("kimi")`, `-p <prompt>`, `--output-format stream-json`, `-m <model>`,
  piped stdio, and **the operator env inherited untouched** — asserted by
  `cmd.get_envs().count() == 0` on **both** (tests `build_command_argv_is_the_0_28_contract`,
  `build_init_command_argv_and_env`). The one-shot surface is not a hole: there is no
  run-path token-scrub or encoding coercion for it to miss (Kimi 0.28 injects
  nothing — unlike Copilot's D8 scrub).
- **Plan drift (re-grounded):** the Phase 4 AC expected the one-shot builders to
  carry a `PYTHONUTF8=1` contract and the `.ralphy/skills` container. In 0.28 code
  **neither applies** — the env is inherited untouched (no `PYTHONUTF8`), and
  `build_kimi_init_command` **deliberately omits** `--skills-dir` (documented +
  tested: "init charters don't invoke the reviewer skill"). The correct
  re-confirmation is the shared command core above, not the two 1.48-era assumptions.

## Phase 3 — usage scan + billing — ✅ RESOLVED (the mismatch is categorical: subscription, not metered)

**Scan (3.1):** `scan_kimi` produces a real, non-null, now-priced number
(`ralphy usage --by model` → `k3 · 913.7k tok · ~$0.29`), sourced from real
`wire.jsonl` turn-usage — never `null`, never fabricated. (Finding #1 made it price.)

**Billing (3.2) — no per-token bill exists to reconcile against.** The `kimi-code`
0.28 CLI exposes **no** usage/billing/account subcommand (`kimi --help`: only
`export/provider/acp/web/login/doctor/vis/migrate/upgrade`), and the only URL it
emits is the **pricing** page `kimi.com/code/#pricing` — not a usage dashboard. The
live 403 (`usage limit for this billing cycle … upgrade your plan`) proves Kimi-code
is a **flat subscription + billing-cycle quota**, not metered per-token billing.

Therefore the unit mismatch is **categorical**, and stateable in full without the
operator's billing page:
- **Ralphy's number** sums `wire.jsonl` tokens and projects USD via the ADR-0034
  **list-price counterfactual** (K2-family: input 0.95 / output 4.0 / cache_read 0.16
  per 1M). Concrete this pass: plan #111 = `57 006 / 1 014 784 / 21 422` → **$0.30**;
  execute #111 = `84 205 / 4 164 864 / 26 275` → **$0.85**.
- **Kimi's actual charge** is a fixed subscription fee + a per-cycle token/request
  quota. There is **no line item** to place beside Ralphy's tokens.
- **The only real cost signal Kimi exposes is the binary quota-exhausted 403** — not
  a dollar figure. Ralphy's `$` is a pure counterfactual (ADR-0034), and the honest
  cross-check against reality is the **ceiling event** (Phase 4b), not a bill.

This mirrors the Copilot/Cursor "bills in credits, not tokens" pattern, taken one step
further: Kimi bills in *subscription + quota*, so even a credit-to-token mapping is
absent. No under-report of the Cursor #269 shape is possible because there is no
metered bill to under-report against.

- **Scan produces a real, non-null, priced number.** Post-Finding-#1, `ralphy usage
  --by model` on FinCal reports `k3 · 913.7k tok · ~$0.29` — a real number sourced
  from `scan_kimi`, never `null`, now priced. The interactive `wd_fincal` wire.jsonl
  carries real turn-scope usage (`inputOther 487 687 · output 122 572 · inputCacheRead
  10 182 144`), so no session with a row reports `null`.
- **Deferred to the HITL session:** the faithful `scan_kimi` interactive surface is
  the daemon `GET /api/usage` (which excludes run-owned session ids — `ralphy usage`
  reads the persisted *ledger*, a different cut, so its totals don't line up to the
  digit with a raw wire.jsonl sum that includes run sessions). The daemon
  digit-match **and** the token-vs-Kimi-billing reconciliation (unit mismatch) both
  wait for the operator (billing is HITL regardless).

## Phase 0 — auth stop (D6), force-reproduced — ⚠️ GAP FOUND (issue #281)

Operator ran a real `kimi logout` (credentials dir emptied). A clean plan-only run
on #111 (base `feat/opencode-v2`, no stale `.ralphy/plan.md`) **did not stop on the
auth message** — it fell through to the generic error:

```
planning cmd=kimi model=kimi-code/k3
Error: kimi produced no plan at C:/Dev/FinCal\.ralphy\plan.md
```
Log: `docs/live/kimi-274-loggedout.log`, kimi.log:
```
error: failed to run prompt: config.invalid: Model "kimi-code/k3" is not configured in config.toml…
```

- **Root cause:** `kimi logout` strips the login-populated model catalog from
  `~/.kimi-code/config.toml` (`default_model` + every `[models.*]` gone). With the
  adapter's pinned `-m kimi-code/k3`, the logged-out signal is **`config.invalid`**,
  not `auth.login_required`. `is_kimi_auth_error` matches only the latter (the
  *expired-token* variant, config intact — what #155 saw positively), so the
  *full-logout* variant is unhandled → misclassified as `kimi produced no plan`
  (the [[cursor-plan-quota-misclassified]] shape).
- **The stop itself is clean** (no loop, no commit, no run branch left) — the miss
  is the *classification*, not a runaway. The scaffold checks `is_auth_error` first
  (correct order); only the matcher is incomplete.
- **Filed:** issue **#281** — add the full-logout markers to `is_kimi_auth_error`
  via the shared `auth_error` helper (Codex multi-group precedent verified in code),
  with a maintainer ruling on the `config.invalid` conflation tradeoff. Evaluated as
  structural (design decision), not a drive-by fix, per house rules.
- **Re-login half — ✅ confirmed.** After the operator's `kimi login`, credentials
  returned and `config.toml` was **repopulated** (`default_model` + 4 `[models.*]`) —
  login restores the catalog logout stripped. Auth is resolved: the next call gets
  past auth to a provider **403 quota** error (not `config.invalid`/auth), proving
  the session is authenticated again.

Two secondary observations: (a) `config.toml` is **not** byte-stable across a
logout (the catalog is stripped), refining the Phase 5 hygiene note — it *is* stable
across authenticated runs; (b) the no-`-m` logged-out signal is cleaner
(`No model configured … use /login to sign in`) but the adapter always pins `-m`.
## Phase 4b — the exit-75 ceiling (D9), marquee — ✅ CEILING OBSERVED LIVE → the mapping is WRONG (issues #282)

**The marquee event happened.** A real `kimi-code` 0.28 **billing-cycle ceiling**
was hit live (from the day's runs) — the limit #155 could never force. It does **not**
arrive as ADR-0028 D9 predicts.

Raw `kimi -p … -m kimi-code/k3` on the exhausted quota → **exit code `1`** (not `75`):
```
error: failed to run prompt: provider.api_error: 403 You've reached your usage limit for this billing cycle. Your quota will be refreshed in the next cycle. …
```
Real ralphy plan-only (`docs/live/kimi-274-limit.log`) → `Error: kimi produced no
plan` — **not** `Limit(None)`, `--stop-on-limit` never engaged.

**Findings (promoted from unit-tested-only to observed-live — and refuted):**

1. **The mapping is a red herring.** `outcome.rs:99` (`exit 75 → Limit(None)`) is
   unit-tested but the **real ceiling exits `1` + text**. Exit-75 stays unobserved.
2. **Execute matcher was stale — [FIXED].** `is_kimi_limit_text` matched only
   `access_terminated_error`; the 0.28 body carries `provider.api_error: 403 … usage
   limit for this billing cycle` (no such token). Fixed on `feat/copilot`: the matcher
   now also matches the 0.28 prose (echo-safe via `detect_limit`'s non-clean-exit
   guard), regression test pins the live string. `cargo fmt`/`clippy`/4-4 auth tests
   green.
3. **Plan path never detects limits.** `lib.rs:180` passes `|_log| None`. This is what
   the live repro actually hit — a billing cap blocks planning first, so it is
   `produced no plan` even with #2 fixed. **5 of 7 adapters** (Codex/Copilot/Gemini/
   Cursor/Claude) surface plan-time limits as typed `PlanLimit`; Kimi & OpenCode are
   the outliers. Recommended fix in **#282** (control-flow change → maintainer ruling).
4. **The good case holds:** the ceiling is **not** swallowed into a wall-timeout burn
   (the OpenCode failure mode) — `kimi` exits `1` fast.

**Verdict:** D9 is **observed-live but refuted** — the ADR-0028 D9 exit-75 claim does
not describe the real 0.28 billing ceiling. ADR stays **proposed**; the D9 section must
be rewritten to "exit-1 + `provider.api_error: 403 … billing cycle` text" per #282.
## Phase 6 — cross-platform parity (WSL) — ◐ PARTIAL (env/resolve findings captured; live Linux run deferred)

- **Environment:** Ubuntu-22.04; the **Linux** `kimi-code` binary is a 160 MB ELF at
  `/home/corcino/.kimi-code/bin/kimi`.
- **`resolve_program` parity finding (plan drift).** The plan expected
  `resolve_program` to find `~/.local/bin/kimi`. In 0.28 the Linux binary is at
  **`~/.kimi-code/bin`** (not `~/.local/bin`, which has no `kimi`), and kimi-code
  registers that dir on PATH via **`~/.bashrc`** (interactive shells) — *not*
  `~/.profile`. `resolve_program` reads process PATH + `~/.local/bin`, so it resolves
  Kimi from an interactive WSL shell but is fragile for a non-interactive launch
  (systemd/cron/login-only) — the [[wsl-vendor-cli-probing]] hazard. The WSL login
  PATH also inherits the *Windows* `.../.kimi-code/bin` via `/mnt/c` (a `kimi.exe`,
  not the ELF), which `command -v kimi` does not match.
- **Encoding (re-grounded):** moot on Linux — 0.28 carries no `PYTHONUTF8` env, so
  there is nothing to be a no-op; the UTF-8 locale needs no coercion (matches the
  drift ledger).
- **Deferred:** a live Linux plan run + short execute (needs a WSL-native ralphy
  build — a `/mnt/c` cargo build is slow and did not survive backgrounding this
  pass) and the **exit-75 platform-identity** check (blocked on Phase 4b's real
  ceiling — HITL). Best run alongside the operator's 4b session.

---

## Final status (capstone closed; ADR stays proposed)

| Phase | Verdict |
|-------|---------|
| 1 plan-only + D7 harvest | ✅ PASS |
| — Finding #1 (scan pricing) | ✅ fixed on `feat/copilot`, green gate clean |
| 2 green run + ladder + stream-vs-diff | ✅ PASS (fresh green close carried from #155) |
| 3 usage scan / billing | ✅ RESOLVED (categorical: subscription, not metered) |
| 4 one-shot builders | ✅ verified in code (live carried from #155) |
| 5 host hygiene / residue | ✅ PASS (+ OAuth-rotation plan amendment) |
| 0 auth stop (D6) | ⚠️ gap → **#281**; re-login half ✅ |
| **4b ceiling (D9), marquee** | ✅ observed live — mapping **refuted**; matcher fixed → **#282** |
| 6 WSL parity | ◐ env/resolve findings; live run + exit-75 deferred (quota) |

**Outcome:** D9 captured live but refuted (real ceiling exits 1 + text, not 75) →
**ADR-0028 stays proposed**, pending #282 + a WSL live pass once quota resets. Two
code fixes landed on `feat/copilot` (Finding #1 pricing; the 0.28 limit matcher);
two issues filed (#281 auth-stop, #282 limit). Green gate clean.

No `git push`, no PR. Ralphy edits on `feat/copilot`; FinCal left on
`capstone/opencode-273`, all `afk/*` run branches intact (a mid-run over-delete was
restored to exact shas).
