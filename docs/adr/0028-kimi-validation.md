# Kimi adapter — live end-to-end validation note (issue #155)

Capstone validation of the Kimi adapter ([docs/adr/0028-kimi-adapter.md](./0028-kimi-adapter.md))
against a **real repository**, closing issue
[#155](https://github.com/paulocorcino/ralphy/issues/155). This records what was run
and observed; the decisions live in ADR-0028, whose deferred items were resolved
from these results and whose Status is flipped to **accepted** on the strength of
this note.

## Environment

- `kimi 1.48.0` (installed via `uv tool install` at `~/.local/bin/kimi`, **off a
  fresh `PATH`** — resolved through `resolve_program("kimi")`, which probes
  `~/.local/bin`, exactly as ADR-0028's Consequences require).
- Auth: **`kimi login`** OAuth (token at `~/.kimi/credentials/kimi-code.json`); the
  adapter manages no provider key (D6). Signed in throughout — plan and execute
  only run logged in, so a clean plan/execute is itself the auth-OK signal.
- Model: `kimi-code/kimi-for-coding` passed explicitly with `-m` (D4); no config
  parse.
- Target repo: `C:\Dev\FinCal` (a real Next.js + Prisma + SQLite finance app),
  GitHub `paulocorcino/FinCal`, base branch `feat/kimi`, run branches cut as
  `afk/run-*` off `feat/kimi`.
- Host: Windows 11, `node 22.22`, `npm 11.12`, `docker 28.1.1` (daemon up) — the
  stack issue #29 needs.
- Ralphy: built from `feat/kimi-validation-155` at validation time (carries the
  fixes below).

## What was run, and the observed outcome

### Phase 1 — plan-only dry-run (`--only-issue 29 --agent kimi --dry-run`)

```
ralphy.exe run --repo C:/Dev/FinCal --only-issue 29 --agent kimi \
  --base-branch feat/kimi --dry-run --verbose
```

- `kimi --print` drove the plan headless; a `.ralphy/plan.md` was produced with
  **15 open steps**, `Feasible: yes`, a full acceptance ledger, `## Verify`
  commands, decisions, and caveats — including a correctly-handled caveat for the
  `.env` real-secrets rule (never `cat .env`; checked via `git status` only). ✓
  (ADR-0028 acceptance #1, plan half)
- **Token harvest is live (D7).** Snapshot-diffing `wire.jsonl` yielded
  `input 21 256 · cache_read 222 720 · output 8 435` for the plan session — usage
  is not on the stdout stream and was recovered from the session store as designed.
- Repo returned to `feat/kimi`; the empty run branch was removed. Duration ≈289 s.
- **Finding (fixed):** the run priced out as `$?` — `pricing.toml` had no
  `kimi-code/kimi-for-coding` row (only OpenCode's `k2p6`). Added the native id to
  the default price table (indicative K2-family list price) so `--agent kimi` runs
  cost out instead of logging "unknown model".

### Phase 2 — full non-dry-run (`--only-issue 29 --agent kimi`)

**Attempt A (pre-fix) — `Stuck`, and the reason is the headline finding.**

`kimi` executed real work and committed **3 times**, then exited **1** with no
`RALPHY_DONE_EXIT`. The adapter classified it **`Stuck`** (`exit_code=Some(1)`,
`committed=true`, no sentinel → progress guard downgrades the commits, not a false
`Done`); the run stopped non-green and handed the branch back. The classification
ladder (D2) is validated exactly: three commits did **not** buy a green close
without the clean-exit sentinel.

The exit 1 came from **inside kimi**, not the work:

```
'charmap' codec can't encode character '✔' in position 574: character maps to <undefined>
```

`✔` is the `✔` Prisma's `generate` prints during `npm install`. Kimi captured
that **tool-subprocess** output and crashed encoding it to a cp1252 stdout. This is
the ADR-0028 D5 Windows-encoding hazard **surfacing on child-output capture**, a
path `--output-format stream-json` (which the adapter forces, and which keeps
*kimi's own* rendering ASCII-safe) does not cover.

**The fix (folded into the adapter).** `PYTHONUTF8=1` (Python UTF-8 Mode, PEP 540)
puts kimi's stdio on UTF-8 so captured Unicode can't crash it — and, unlike
`PYTHONIOENCODING=utf-8` (the D5 trap that flips kimi into the Textual TUI, "No
Windows console found"), it does **not** re-trigger the TUI, because it touches
encoding, not console detection. Verified live with a minimal `✔`-emitting
subprocess: exit 0, clean `stream-json`, final text ended `RALPHY_DONE_EXIT`, no
TUI. Now set on `build_kimi_command`, `build_kimi_init_command`, and the gate's
Kimi login probe (with `PYTHONIOENCODING` still stripped). No-op on an
already-UTF-8 Linux locale. Regression-tested.

**Attempt B (post-fix) — green.**

```
ralphy.exe run --repo C:/Dev/FinCal --only-issue 29 --agent kimi \
  --base-branch feat/kimi --max-minutes-per-issue 120 --verbose
```

- plan → **21 steps**; `execute()` ran ≈25 min → **`Done`**
  (`exited_cleanly=true`, `committed=true`, `exit_code=Some(0)`), **zero** charmap
  crashes in the session log. The fix holds under a real, subprocess-heavy run
  (`npm ci`, `prisma generate`, `next build`, `docker build`, `vitest`).
- **Verify gate** (ADR-0011) re-ran all 12 `## Verify` commands over the committed
  code and **passed**; **green — issue #29 CLOSED** with a full Handoff comment
  (delivered artifacts, commits `b138ca1`/`1350e1a`, environment traps, working
  commands, residue, plan-friction) and the acceptance ledger written back. ✓
  (ADR-0028 acceptance #1, full `plan→execute→commit→close-green`)
- Per-issue usage (live): `input 139 381 · cache_read 4 734 464 · output 30 549`.

### Skills discovery (D8, `--skills-dir`)

During the green run, `.ralphy/skills/{reviewer,staged-plan}` was materialized and
pointed at with `--skills-dir`; `git status` showed nothing under `.ralphy`
(`.ralphy/.gitignore = *` holds), and the plan's self-review step invoked the
`reviewer` skill from that store. This resolves D8's deferred layout: **a
ralphy-owned `.ralphy/skills` container, gitignored** — no `.agents/`/`.kimi/`
residue in the target repo, no symlink dance.

### Phase 3 — failure paths (D6/D9)

- **Auth-error stop (D6):** not force-reproduced live — a `kimi logout` would have
  broken every subsequent validation run. The detector `is_kimi_auth_error`
  (exit 1 + `LLM not set`) is unit-tested and takes precedence over generic
  classification; auth-OK is proven positively by every logged-in plan/execute.
- **Usage-limit / exit-75 (D9):** a real 429 could not be forced without burning
  quota (the one item ADR-0028 already settled by source, not observation). The
  mapping **exit 75 → `Limit(None)`** is grounded in kimi's `RETRYABLE = 75`
  (`kimi_cli/cli/__init__.py`) and unit-tested (`classify_limit_on_exit_75`);
  `--stop-on-limit` is force-enabled for Kimi (`effective_stop_on_limit`,
  unit-tested). No text-scraping.

### One-shot flows (`diagnose` / `draft-issues` / `triage`)

- **Triage** was run live as a Kimi one-shot (`ralphy triage --agent kimi`,
  preview) — kimi drove the judgment through its native `stream-json` (Shell /
  ReadFile / Glob tool calls) and produced a verdict; captured in
  `triage.log`/`triage-kimi.log`.
- **Diagnose** and **draft-issues** were exercised via `ralphy init` on the same
  repo (`.ralphy/diagnosis.json`, `.ralphy/init-state.json` recording
  `completed: [diagnose, git, scaffold, skills, labels, issues]`, and the drafted
  FinCal backlog). The one-shot command builders carry the same `PYTHONUTF8=1`
  contract.

## Incidental finding (vendor-neutral — not a kimi defect)

The green run's verify gate **hung ≈43 min** on one plan-authored verify command:

```
sh -c "npx prisma migrate deploy && npm run dev -- --port 3002 & PID=$! && sleep 15 && curl ... && kill $PID"
```

The `&` backgrounds the whole `… && npm run dev` compound; `kill $PID` kills the
subshell, not the `next dev` child, so an orphaned dev server lingers and git-bash's
`sh -c` never reaps → the gate blocks. Killing the orphan (`taskkill /F /PID`)
unblocked the gate, which then passed and closed green. This is a **verify-command /
gate-robustness** issue any vendor's planner could author (kimi's own Handoff even
documents the stale-`node.exe` trap it hit) — recommended as a follow-up, not a
Kimi adapter bug. Candidate hardening: reap each verify command's process group on
Windows, or teach planners not to leak background jobs.

## Deferred decisions resolved (ADR-0028 Consequences)

- **`ACCEPTS_IMAGES` → `false` (settled).** The model advertises
  `image_in`/`video_in`, but `kimi --print` exposes **no** image/attachment flag —
  its only input is a text/`stream-json` charter on stdin — so there is no verified
  multimodal delivery path. `true` would make triage attachment-fetch (ADR-0025 §4)
  pull images the adapter cannot hand to the CLI. Stays `false` until Kimi ships a
  `--print` image channel.
- **`--skills-dir` layout → `.ralphy/skills` container, gitignored.** Confirmed
  materializing and loading the `reviewer` skill live (above).

## Cross-vendor parity

Diff over the Kimi commit range (`c43ebd2..a11e303`) plus this note's fixes:

- **No `ralphy-core` `pub` surface change.** The one core edit is
  `DEFAULT_MAX_MINUTES_PER_ISSUE` `0 → 60`, and it came from an **independent**
  refactor (`e273400`, "update per-issue wall-clock budget default") that merely
  shares the commit which added the ADR-0028 file — it applies to **all** vendors
  uniformly, not a Kimi-specific change. The const's name, type, and visibility are
  unchanged.
- `ralphy-agent-claude/src/settings.rs`: **doc-comment only**.
- `ralphy-adapter-support`: `HeadlessRun` gained an **additive** `exit_code:
  Option<i32>` field (load-bearing for D9's exit-75 mapping); other adapters just
  populate it.
- **Untouched:** the Claude/Codex/OpenCode adapters' logic, the existing
  prompts/plugin, `hook.rs`, `guard.rs`, and the `ANTHROPIC_API_KEY` clearing —
  structurally, as ADR-0028 promised.

## Defects found and fixed (folded into the code + ADR-0028)

1. **Windows cp1252 crash on subprocess-output capture** (`'charmap' codec can't
   encode '✔'`, exit 1) — killed the first live execute despite real commits.
   Fixed by setting `PYTHONUTF8=1` on every Kimi child (`command.rs`, `gate.rs`),
   alongside the existing `PYTHONIOENCODING` strip. Regression-tested.
2. **`kimi-code/kimi-for-coding` unpriced** — every `--agent kimi` run reported
   `$?`. Added the native id to the default `PriceTable` (`pricing.rs`), tested.

All three green-gate checks pass on this branch: `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace`.

## Docs updated alongside

- README `--agent` table / prerequisites / usage-limit / everyday-flags now
  enumerate **kimi**; the intro scope line lists it.
- CONTEXT.md's **Adapter** definition now names Claude, Codex, Kimi, and OpenCode.
- ADR-0028 Status flipped to **accepted**, with the deferred items marked resolved.
