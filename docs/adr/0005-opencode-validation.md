# OpenCode adapter — live end-to-end validation note (issue #29)

Capstone validation of the OpenCode adapter (docs/adr/0005-opencode-adapter.md)
against a **real repository**, closing issue
[#29](https://github.com/paulocorcino/ralphy/issues/29). This records what was run
and observed; the decisions live in ADR-0005, whose deferred-items were updated
from these results.

## Environment

- `opencode 1.16.2` (npm install on Windows: `opencode.cmd` shim, no `.exe`).
- Provider/auth: **Kimi For Coding** (the only authenticated provider), via the
  opencode/Zen hosted gateway. No `-m` passed → opencode resolves its own model
  (ADR-0005 D4).
- Target repo: `C:\Dev\OCS\appusage` (a real Go project), GitHub
  `paulocorcino/appusage`, working branch `sprint-dev-2`, run branches cut from
  `origin/main`.
- Ralphy: built from `feat/opencode` at validation time.

## What was run, and the observed outcome

### Phase 1 — dry-run / plan-only (`--only-issue 5 --agent opencode --dry-run`)

```
ralphy.exe run --repo C:\Dev\OCS\appusage --only-issue 5 --agent opencode --dry-run --verbose
```

- `opencode run` driven headless; a `plan.md` was produced; the repo returned to
  `sprint-dev-2` and the empty run branch was removed. ✓ (ADR-0005 acceptance #1)
- Issue #5 planned to **0 open steps** — correctly, because the model found it
  *already fully implemented on the branch* by earlier runs ("no residue to
  plan") → the queue **skipped** it (a true negative, not a parse miss).

### Phase 2 — full non-dry-run (`--only-issue 6 --agent opencode`)

Issue #5 is already implemented, so a non-dry-run on it would re-plan to 0 steps
and never reach `execute()`. To exercise execute→classify→close-on-green, the
target was the feasible, unblocked issue **#6** (chosen with the operator).

```
ralphy.exe run --repo C:\Dev\OCS\appusage --only-issue 6 --agent opencode --verbose
```

Observed across three attempts (the gateway is intermittently flaky — see below):

- **Attempt A** (plan): plan failed with the gateway's transient
  `UnknownError` → adapter correctly bailed "opencode produced no plan". No
  adapter bug; the run branch was restored cleanly.
- **Attempt B**: plan → 12 steps (feasible); `execute()` hit the same transient
  `UnknownError` after ~2s → classified **`Stuck`** (`saw_error=true`,
  `committed=false`, non-zero exit). The HEAD-diff progress guard + error-event
  downgrade both exercised live; run stopped non-green, branch handed back, **0
  commits**. ✓ (correct `Outcome`, ADR-0005 D2)
- **Attempt C** (green): plan → 6 steps; `execute()` ran ~19 min →
  **`Done`** (`exited_cleanly=true`, `committed=true`, `saw_error=false`). The run
  made **4 clean commits** (only real source: `internal/ocscontract`,
  `internal/managementqa`, `internal/ocsexport` — no `.ralphy/` scratch);
  **close-on-green** closed issue #6 with the run comment **and the acceptance
  ledger written back** (each criterion tagged `[verified]`/`[review-only]` with
  commit evidence). ✓ (ADR-0005 acceptance #2 + ledger)

### Skills discovery (ADR-0005 acceptance #3, D7)

The green run's execute stream contains
`{"type":"tool_use","part":{"tool":"skill","state":{"title":"Loaded skill:
reviewer-v2"}}}` — opencode discovered and loaded the reviewer skill from the
materialized `.ralphy/skills` **container** pointed at by the injected
`OPENCODE_CONFIG_CONTENT` `skills.paths`. Nothing was written outside `.ralphy/`
in the deliverable. ✓ Resolves D7 granularity: **container dir**.

### Phase 3 — failure paths (ADR-0005 D6/D9)

- **Auth-error stop (D6):** not reproducible here. Moving `auth.json` aside still
  let `opencode run` succeed (resilient/cached credential), and an unconfigured
  provider returned the gateway's opaque `UnknownError`, not a typed
  `ProviderAuthError`. The matcher (`is_opencode_auth_error`, documented
  `providerautherror` substring) stays as-designed; auth.json was restored.
- **Usage-limit / timeout backstop (D9):** no real 429 was reproducible — the
  gateway surfaces all transient failures as `UnknownError`, which *reinforces*
  D9's thesis that the per-issue wall timeout (not a text matcher) is the primary
  limit backstop. The timeout-kill path itself is covered by the shared
  `run_headless` live test and the adapter's classification unit tests.

## Defects found and fixed (folded into the code + ADR-0005)

1. **`--agent opencode` rejected.** clap derived the kebab-cased `open-code` from
   the `OpenCode` variant. Pinned the value to `opencode` (alias `open-code`).
   `main.rs`.
2. **`opencode` binary not found on Windows.** `Command::new("opencode")` finds
   only `.exe`; the npm shim is `opencode.cmd`, and the extensionless `opencode`
   shell shim beside it is "not a valid Win32 application" (os error 193). Added
   `resolve_program`/`find_program` to `ralphy-adapter-support` (honours
   `PATHEXT`, skips the extensionless shim) and resolve through it.
3. **`--format json` parsed against the wrong shape.** opencode 1.16.2 wraps every
   event `{type, …, part:{…}}` (payload under `part`) and errors as
   `{error:{name, data:{…}}}`. The adapter read these fields at the top level —
   extracting **empty** assistant text (breaking every execute sentinel scan) and
   missing typed errors. Added `event_payload`/`error_detail`/`error_name`; reads
   both the live and the flat shapes. Resolves D2's "exact event JSON deferred".

All three have regression tests using the **live-captured** shapes. Full
workspace `cargo test` and `cargo clippy` are clean.

## Open follow-up (not opencode-specific)

`.ralphy/plan.md` is **tracked** in the appusage `origin/main` (committed by an
earlier run, `a3760c2 chore: record Ralphy completion evidence (#5)`). git cannot
ignore an already-tracked file, so the planner's overwrite leaves a modified
tracked file, and the clean-run return-to-orig — a **non-force** `git checkout` in
`ralphy-core` (the dry-run/error `restore()` path forces; the happy path does not)
— aborts, stranding the repo on the run branch. The deliverable and close-on-green
are unaffected. Fix is repo hygiene (`git rm --cached .ralphy/plan.md`) plus
optionally hardening the core checkout-back to tolerate tracked scratch. Filed as
[#41](https://github.com/paulocorcino/ralphy/issues/41).
