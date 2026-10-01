# OpenCode #273 — deep re-validation, e2e runbook

Live execution of `docs/adr/0005-opencode-revalidation.md` (the #251 bar) against
the authorized **FinCal** lab. This file is the operational runbook + the gap
ledger; evidence lands in `docs/live/opencode-273-*.log`. Rewritten into the ADR
note when the phases close.

## Environment (captured 2026-07-22)

| Item | Value |
|---|---|
| opencode build | **1.18.4** (first note ran 1.16.2 → version skew to record) |
| resolver | `resolve_program` → PATH/`PATHEXT`, `~/.local/bin` fallback (`ralphy-proc-util/src/lib.rs:127`) |
| providers authed | `kimi-for-coding`, `zai-coding-plan` (z.ai/GLM — glm cap candidate for F4b) |
| model | no `-m` (D4) — record what opencode resolves |
| auth.json | `~/.local/share/opencode/auth.json` · sha256 `751b8041…ca6b` |
| config | `~/.config/opencode/opencode.jsonc` · sha256 `4e901f9e…4a61` |
| store | `~/.local/share/opencode/opencode.db` (192 MB) + live `-wal`/`-shm` |
| server log | `~/.local/share/opencode/log/opencode.log` (F4b hunting ground) |
| lab | `C:\Dev\FinCal` @ `capstone/opencode-273` (from clean `master`, `.ralphy/` untracked) |
| blocker #41 | **CLOSED** — still confirm live no tracked `.ralphy/plan.md` (F5) |

## Code pre-verification (what the plan feared vs what the code already does)

The map already resolves several plan worries — annotate, don't re-litigate:

- **D9 server-log limit (F4b marquee):** the code is **no longer stream-only**.
  `execute` runs `parse_opencode_limit(stdout).or_else(parse_opencode_log_limit(log))`
  (`events.rs:199` + `events.rs:245`), fed by `--print-logs --log-level ERROR`
  (`command.rs:45`), plus an **early-kill on a stderr limit line** (`lib.rs:266`).
  So the "silent swallow → burn 60 min" thesis is *already addressed in code*.
  F4b's job shrinks to: **confirm the log-tail detector fires on a real cap** and
  capture the exact string — not to discover silence.
- **Auth stop (D6/F0):** `is_opencode_auth_error` = case-insensitive
  `providerautherror` substring, precedence over DONE tested (`events.rs:36,320`).
- **Classify ladder / Timeout / committed guard (F2):** shared `classify`
  (`classify.rs:39`), limit outranks Timeout/Stuck, `committed` is a progress
  signal only (`lib.rs:289`). Handled + tested.
- **`.cmd` shim (F6):** resolves `.cmd` not `.exe`, tested (`proc-util lib.rs:411`).
- **Pricing (F3):** unknown model → `None` (never `$0`), ADR-0034 (`pricing.rs:7`).

## Open gaps carried in (disposition decided live)

| # | Gap | Code evidence | Disposition |
|---|---|---|---|
| G1 | **WAL-safety**: `scan_opencode` reads live `.db` in place, no sidecar copy; Copilot copies `.db`+`-wal`+`-shm` via private `copy_store` | `opencode.rs:48-51` vs `copilot.rs:66` | **Measure in F3 first** (is the under-count real on the live store?), then **file issue** — promoting `copy_store` mid-capstone would contaminate the F3 measurement |
| G2 | **#41 in-adapter guard**: no abort when `.ralphy/plan.md` is *already tracked*; dirty check ignores `.ralphy/` | `git.rs:269`; Cursor has a resume guard, OpenCode doesn't | #41 CLOSED at core level; confirm live in F5. Issue only if the tracked-plan trap reproduces |
| G3 | **One-shot `out_path`**: triage/draft artifact path is caller-controlled | `tasks.rs:83,131` | Verify CLI keeps `out_path` under `.ralphy/` in F4; issue if it escapes |
| G4 | **cwd leak (FOUND LIVE F1)**: opencode child operates on the *parent process cwd*, not `--repo`/`ws.repo_root()`. `--repo <other>` from a different cwd makes opencode read the wrong repo's `.ralphy/issue.json` and write plan.md/edits there | `command.rs:54` sets `current_dir` yet ineffective; hypothesis: inherited `PWD`/`INIT_CWD` overrides it | **Issue #278** — reproducible, mechanism unproven, don't blind-patch |

## Live ledger (F1)

- **F1 DONE** (from correct cwd, `f1b-cwd-fincal.log`): plan-only re-confirmed on
  opencode **1.18.4**, `{type,part}` schema holds, plan.md #108 written. Model
  resolved **`k3`** (kimi). Token baseline: `in 38.8k cr 351.1k cw 0 out 5.0k · $0.17`.
  ADR-0034 confirmed live: unknown model `k3` → `+?`, never `$0`.
- **G4 cwd bug** surfaced by the first F1 attempt (run from `C:\Dev\ralphy` cwd):
  opencode read `C:\Dev\ralphy\.ralphy\issue.json` (#266) and wrote plan.md into
  the ralphy repo. Re-run from FinCal cwd → correct. Residue cleaned. → **#278**

- **F0** (`f0-auth-stop.log`): moving `auth.json` aside does **NOT** log opencode
  out on 1.18.4 — the session ran full (35.7k tok) on a cached credential and
  declined only because the branch is docs-only. `is_opencode_auth_error`
  (`providerautherror`) never fires; the documented backstop ("produced no plan")
  is what surfaces. **D6 genuine-revoke is HITL** (matches the #29 note). auth.json
  hash restored identical.
- **F2** (`f2-timeout-kill.log`): `--max-minutes-per-issue 2` cut the **plan**
  phase (~2.5 min on this vendor) before plan.md → surfaced as "produced no plan",
  not `Timeout`. Execute-phase `Timeout`/`non_green` is unit-covered
  (`outcome.rs` Timeout-wins / Timeout→Limit). Observation: a per-issue cap tighter
  than the planner's runtime masks a plan-phase Timeout as plan-absence.
- **F3** (WAL probe): `ralphy usage` = real 537.0M tok (no null/fab). WAL holds
  committed-uncheckpointed rows (delta 1 at rest, `-wal` 5 MB). In-place read sees
  the WAL in the same-dir case, but scan is **not** copy-safe like copilot → **#279**.
- **F4**: write-containment **code-verified** — `issues_draft_path` =
  `.ralphy/issues-draft.json`, `diagnose_repo` → neutral temp cwd, `triage` →
  `.ralphy/triage.log`; skills injection unit-tested (`command.rs`). No live triage
  target in FinCal without polluting the repo. G3 **not a gap**.
- **F5**: auth.json + config byte-identical before/after all runs; FinCal tree
  clean, `.ralphy/` untracked + gitignored; no ralphy-repo residue. **#41 does not
  reproduce** → G2 not a gap.
- **F4b CLOSED by historical evidence + tests** (no fresh cap needed): the
  swallowed-limit strings are already in the server log
  (`~/.local/share/opencode/log/opencode.log`) from real caps —
  - GLM 5h cap (2026-07-11): `AI_APICallError: Usage limit reached for 5 hour.
    Your limit will reset at 2026-07-11 22:14:08` — **retried ~11× in a silent
    backoff loop** on the same cap (the [[opencode-silent-quota-timeout]] behavior),
    carries a reset timestamp.
  - Kimi billing-cycle (2026-07-09): `AI_APICallError: You've reached your usage
    limit for this billing cycle… Upgrade to get more: <url>` — no reset.
  The adapter routes these to stderr (`--print-logs --log-level ERROR`) and
  `parse_opencode_log_limit` matches + extracts the reset. Both exact strings are
  already pinned as unit fixtures (`events.rs:473` `log_limit_detects_zai_5h_cap_with_reset`,
  `:432` kimi) — **27/27 events tests green on 1.18.4**. The original note's "silent
  swallow → burn 60 min" thesis no longer holds.
- **F6 VALIDATED** (Linux/WSL, `f6-wsl-native.log`): Linux ralphy (built in WSL)
  drove native opencode `~/.opencode/bin/opencode` **1.18.3** to a full plan of #108
  (`Feasible: yes`). Parity holds on every axis: native shim resolution, `{type,part}`
  schema, store topology (`opencode.db` + `-wal`/`-shm` — so **#279 applies
  cross-platform**), skills container (`.ralphy/skills` + `.gitignore *`), pricing
  unknown→`+?` (model `big-pickle`). Divergences: version skew 1.18.3 vs 1.18.4;
  model codename differs (provider's own resolution, not ralphy).
  - F6 findings along the way: (a) `resolve_program` on Linux **rejects the /mnt/c
    Windows npm shim** that `which` finds (DrvFs exec/symlink) — native binary must
    be on PATH; (b) Linux ralphy over a `/mnt/c` Windows checkout trips the
    dirty-tree guard (cross-OS mode/EOL drift) — a native Linux clone is required.
    Both are /mnt/c-interop artifacts, not core defects; noted, not filed.
- **F0 CLOSED via the plan's masking-fallback clause** (live, WSL 1.18.4,
  `f0-wsl-noauth.log` / `f0-wsl-badcred.log`): every unauthenticated/misconfigured-
  provider path reachable in WSL masks on the client stream as
  `{"type":"error","error":{"name":"UnknownError","data":{"message":"Unexpected
  server error. Check server logs for details."}}}` — **never** the
  `providerautherror` that `is_opencode_auth_error` matches. The typed cause
  (`ProviderModelNotFoundError: Model not found: zai-coding-plan/glm-5.2`) lives only
  in the server log, and no ralphy detector matches it (the log-scan hunts limit
  strings only). So a run against an unauthed provider does **not** stop on the auth
  ladder — it falls through to "produced no plan"/Stuck, wall timeout the only
  backstop. Confirms and hardens the #29 thesis on the current build. (A true
  `ProviderAuthError` needs a *configured* provider with a revoked key — not
  reachable in WSL, where a bare `auth.json` key doesn't register the provider.)
  - **CONFIRMED with genuine revoked keys (Windows, both providers configured,
    keys revoked at the provider site 2026-07-22)** → **issue #280**:
    - z.ai/glm-5.2 client event `{"name":"APIError","data":{"message":"身份验证失败。",
      "statusCode":401,...}}` (server: `AI_APICallError: 身份验证失败。`).
    - kimi client `{"name":"UnknownError",…}` (server: `ProviderModelNotFoundError`).
    Neither `is_opencode_auth_error` (`providerautherror`), `parse_opencode_limit`
    (wants 429, got 401), nor the log-scan matches → the run classifies as
    `Stuck`/`non_green`, hiding the real "authentication failed" cause. The D6 auth
    detector is **dead against real revoked-credential shapes**. `f0-win-zai-revoked.log`
    / `f0-win-kimi-revoked.log`.

## Result: machine-validatable phases all closed

F0 (masking recorded), F1, F2, F3, F4, F4b, F5, F6 — done. Two issues filed
(#278 cwd-leak, #279 WAL not-copy-safe, cross-platform-confirmed). Genuine
credential-revocation (true `ProviderAuthError`) is the only path needing real
provider action.

## Phase ledger

- [ ] **F0** auth stop — force revoked cred, expect stop on `providerautherror`
      (or record UnknownError masking + wall-timeout backstop)
- [ ] **F1** plan-only on 1.18.4 — `{type,part}` schema holds; capture clean
      per-run token baseline (F3 anchor)
- [ ] **F2** full run — `Stuck`/`Done` ladder + committed-guard-not-stream +
      `--max-minutes-per-issue` kill → `Timeout`/`non_green`
- [ ] **F3** usage — G1 WAL under-count probe; real token number (no null/fab);
      Ralphy total vs provider bill, unit mismatch stated
- [ ] **F4** one-shots — `triage`/`diagnose`/`draft-issues`; G3 out_path check;
      per-issue budget
- [ ] **F4b** (marquee) real cap — confirm log-tail detector fires; exact
      message/exit/reset captured
- [ ] **F5** hygiene — auth.json/config byte-identical (hashes above); no tracked
      `.ralphy/plan.md`; no mid-checkpoint WAL residue; unasked artifacts
- [ ] **F6** WSL/Linux — repeat F1; shim/schema/store/skills/pricing parity
- [ ] no push, no PR, any phase

## HITL boundary

F4b (real provider cap) and F6 (WSL) need a human. F0–F4 + F5 are machine-driven
here. Small gaps fixed inline; larger ones → issues at the end.
