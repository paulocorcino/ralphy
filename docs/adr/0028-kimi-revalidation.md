# Kimi adapter — deep re-validation note (the #251 bar)

Companion to [ADR-0028](./0028-kimi-adapter.md) and a follow-up to the original
capstone note [0028-kimi-validation](./0028-kimi-validation.md) (issue
[#155](https://github.com/paulocorcino/ralphy/issues/155)). This file **was** the
plan; it is now the note of what issue
[#274](https://github.com/paulocorcino/ralphy/issues/274) actually ran. Raw evidence
lives in [`docs/evidence/274-kimi-capstone-live.md`](../evidence/274-kimi-capstone-live.md)
and `docs/live/kimi-274-*.log`.

**Status: proposed — and it stays proposed.** The marquee D9 ceiling *was* captured
live with a real exit code and string, satisfying the "capture" bar — but the capture
**refuted** the exit-75 mapping (the real billing cap exits `1` + text, not `75`, and
the adapter misclassifies it). Accepting an ADR whose D9 is contradicted would be
wrong; acceptance waits on [#282](https://github.com/paulocorcino/ralphy/issues/282)
landing and a WSL live pass (deferred — the account quota was exhausted at capture
time, which is *why* the ceiling was reachable).

## The migration that re-grounded the plan

The plan was written against `kimi` **1.48.0** (a Python/Textual-TUI build); the host
and the adapter code have since migrated to **`kimi-code` 0.28.0** (migrator run
2026-07-20, `~/.kimi-code`). The adapter source already tracked 0.28; only the plan
text lagged. Four premises were re-grounded (evidence doc, drift ledger), **none a
code defect**:

- `PYTHONUTF8=1` — absent from the crate; 0.28 is not the Python build (env inherited
  untouched, asserted by `get_envs().count() == 0`). Phase 2 confirmed **zero charmap
  crashes** across a 16-min subprocess-heavy build with no env coercion.
- the auth string is `auth.login_required` (not `LLM not set`);
- the model is `kimi-code/k3` (not `kimi-for-coding`);
- the store/creds live under `~/.kimi-code/`, not `~/.kimi/`.

## What each phase found

- **Phase 1 — plan-only (✅).** Real plan artifact; the `wire.jsonl` D7 harvest still
  recovers `input/cache_read/output` on the 0.28 layout; run-path prices cleanly.
- **Finding #1 — usage-scan pricing (fixed).** `scan_kimi_code` strips the
  `kimi-code/` prefix to a bare `k3`, but `pricing/defaults.rs` keyed only the
  prefixed form, so every `ralphy usage` Kimi row reported `~$?`. Fixed on
  `feat/copilot`: `pricing.rs::resolve` gained a `strip_provider_prefix` fallback and
  the rows were re-keyed bare (`k3`/`kimi-for-coding`), aligning with the dominant
  `k2p6`/`kimi-k2.7-code` convention. Green gate clean.
- **Phase 2 — green run + stream-vs-diff (✅).** The `Stuck`→`non_green` ladder holds
  (5 commits without the clean-exit sentinel do **not** buy a green close — the
  HEAD-diff `committed` guard decides). Stream progress ≈ real `git diff` (6/14 steps,
  no inflation). A fresh green close is carried from #155 (the eligible issues are
  large multi-step features; the agent Stuck at the turn budget).
- **Phase 3 — usage & billing (✅).** The scan reports a real, non-null, now-priced
  number. Billing reconciliation resolves **categorically**: `kimi-code` exposes no
  usage/billing CLI surface and bills a **flat subscription + per-cycle quota**, not
  metered tokens — so Ralphy's `$` is a pure ADR-0034 counterfactual with no line item
  to sit beside, and the only real cost signal is the binary quota-exhausted 403.
- **Phase 4 — one-shot builders (✅, code-verified).** `build_kimi_command` (run) and
  `build_kimi_init_command` (init) share the same command core with the env inherited
  untouched; init deliberately omits `--skills-dir` (tested). No token-scrub or
  encoding contract exists for the one-shot surface to miss.
- **Phase 5 — host hygiene (✅, + amendment).** Config byte-identical across
  authenticated runs; `.ralphy`/`.agents` gitignored, no token-bearing tracked write,
  no `.kimi` repo residue. **Amendment:** Kimi rotates its OAuth token on every
  authenticated run (and `kimi logout` strips the model catalog from `config.toml`),
  so "credentials byte-identical before/after" is unachievable — the correct
  stop-condition is *no credential loss, no scope/structure change, no logout*.
- **Phase 0 — auth stop D6 (⚠️ gap → #281).** A real `kimi logout` is **not**
  recognized: it strips the catalog, so the adapter's pinned `-m kimi-code/k3` yields
  `config.invalid` (not `auth.login_required`), which `is_kimi_auth_error` misses →
  `produced no plan`. The stop is clean (no loop/commit), only misclassified. Re-login
  restores the catalog and auth.
- **Phase 4b — the ceiling D9 (⚠️ marquee → #282).** A real billing-cycle 403 was hit
  live. It exits **`1` + `provider.api_error: 403 … usage limit for this billing
  cycle`**, not exit `75`. Three defects: (1) exit-1-not-75; (2) `is_kimi_limit_text`
  matched only the stale `access_terminated_error` — **fixed** to also match the 0.28
  prose (regression test, green gate); (3) the plan path passes `|_log| None` and never
  detects a limit (what the live repro hit — 5/7 adapters surface a plan-time
  `PlanLimit`; Kimi and OpenCode are the outliers). Not swallowed into a wall-timeout
  (exits fast). D9 is observed-live-but-refuted.
- **Phase 6 — WSL parity (◐ deferred).** Ubuntu-22.04, the Linux `kimi-code` is at
  `~/.kimi-code/bin/kimi` on PATH via `~/.bashrc` (not `~/.local/bin`) — a
  `resolve_program` fragility for non-interactive launches. The encoding no-op is moot
  (no `PYTHONUTF8` in 0.28). A live plan/execute + the exit-75 platform-identity check
  are deferred: they need a WSL-native ralphy build and, for the limit, a quota that
  has since been exhausted.

## Landed on `feat/copilot`

- `crates/ralphy-cli/src/pricing.rs` + `pricing/defaults.rs` — Finding #1 (scan pricing).
- `crates/ralphy-agent-kimi/src/auth.rs` — `is_kimi_limit_text` now matches the 0.28
  billing-cycle body (#282 defect 2).

## Filed

- [#281](https://github.com/paulocorcino/ralphy/issues/281) — `kimi logout` full-logout
  not recognized as an auth stop (D6).
- [#282](https://github.com/paulocorcino/ralphy/issues/282) — the real billing ceiling
  (D9) misclassified: exits 1 not 75, stale matcher (fixed), plan path never detects
  limits.

## To flip to accepted

- #282 implemented (plan-time `PlanLimit` route + the 0.28 matcher) so a real ceiling
  classifies as `Limit`, and ADR-0028's D9 section rewritten to "exit-1 + text".
- #281 ruled on (the `config.invalid` conflation) so `kimi logout` stops on the auth
  message.
- A WSL live pass once quota resets: Phase 1 + a short execute, and the exit-1 ceiling
  confirmed platform-identical.
