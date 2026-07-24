# OpenCode adapter — deep re-validation plan (the #251 bar)

Companion to [ADR-0005](./0005-opencode-adapter.md) and a follow-up to the
original capstone note [0005-opencode-validation](./0005-opencode-validation.md)
(issue [#29](https://github.com/paulocorcino/ralphy/issues/29)). Like the Cursor
capstone ([#251](https://github.com/paulocorcino/ralphy/issues/251)), this file is
**now the plan** and will be rewritten into the note that execution produces.

The first note validated the shape — plan-only, the `Stuck`/`Done` classification
ladder, the `.ralphy/skills` container (D7), and it fixed three real defects
(`--agent opencode` clap kebab, the `opencode.cmd` shim resolution, the
`{type,part}` event schema). It **explicitly could not settle three things**, and
those are exactly the #251 dimensions this capstone exists to close:

- **The usage-limit path (D9) was never observed live.** "No real 429 was
  reproducible — the gateway surfaces all transient failures as `UnknownError`."
  The mapping rests on the per-issue wall timeout being the *only* backstop, and on
  a thesis, not an event. This is the highest-value target: OpenCode is the vendor
  the [[opencode-silent-quota-timeout]] finding names — it **swallows a provider
  quota limit in a silent retry** (`glm-5.2` 5-hour cap, Kimi billing-cycle cap),
  so Ralphy sees `saw_error = false` and burns the full 60-minute timeout while the
  error lives only in the OpenCode *server* log.
- **Token counts were never reconciled against a bill.** The scan
  (`ralphy-usage-scan/src/opencode.rs`) reads the session store with a documented
  WAL under-count trap; nobody has put Ralphy's summed number next to the
  provider's (Zen / Kimi-For-Coding) billing.
- **The auth-error stop (D6) was never force-reproduced** — moving `auth.json`
  aside still let a run succeed on a cached credential, and an unconfigured provider
  returned the opaque `UnknownError`, not a typed `providerautherror`.

And two flows the original note did not touch at all: **triage / one-shots**, and
**cross-platform parity** (it was Windows-only).

Status: **proposed** — flips to accepted when the phases below execute and D9's
limit surface is captured with a real string rather than reasoned from absence.

## What fails the whole exercise outright

- A run burning the full wall timeout on a swallowed provider limit **without**
  Ralphy recording *anywhere* that a limit — not a generic stall — is what
  happened. Silent is the failure; the capstone's job is to make it loud or prove
  it already is.
- `ralphy usage` inventing a token number for an OpenCode session with no store
  row, or the WAL under-count trap silently truncating a real count.
- The operator's OpenCode config or `auth.json` differing before and after a run.
- A remote push or opened PR from any phase.

## Environment the run needs

- `opencode` (record the exact build; the first note ran `1.16.2`), resolved
  through `resolve_program` (the `.cmd` shim), on Windows and again on WSL/Linux.
- Provider/auth recorded: which provider is authenticated (the first note used
  **Kimi For Coding** via the Zen gateway) and where `auth.json` lives — D6 is
  exercised against exactly it.
- Model: **no `-m`** (D4) so OpenCode resolves its own; record what it resolved to.
- Target repo with a working build, a feasible unblocked issue, and — critically —
  **`.ralphy/plan.md` not already tracked** (the [#41](https://github.com/paulocorcino/ralphy/issues/41)
  trap the first note found: a tracked scratch file strands the repo on the run
  branch).
- A way to reach a **real provider quota** for Phase 4b — a low-cap provider/plan
  or a deliberately exhausted billing cycle — plus access to the OpenCode server
  log where the swallowed error surfaces.

## Phase 0 — the refusals fire (D6), for real this time

- **Auth-error stop** — force a genuine logged-out / revoked-credential state
  (not just moving `auth.json`, which the first note found insufficient) and
  confirm the run stops on `is_opencode_auth_error` (`providerautherror` substring)
  rather than looping "opencode produced no plan". If the gateway still masks it as
  `UnknownError`, record that plainly and treat the wall timeout as the documented
  backstop.
- Confirm no stale `.ralphy/plan.md` masks the stop (the #271 lesson, generalized).

## Phase 1 — plan-only dry run (confirm, don't re-litigate)

Already green in the first note; re-run only to confirm the minted session and the
`{type,part}` schema still hold on the current build, and to capture a **clean
per-run token number** from the store for the Phase 3 reconciliation baseline.

## Phase 2 — full run + stream-vs-diff delta

The ladder (`Stuck`/`Done`, HEAD-diff `committed` guard) is validated; add the one
piece the first note lacked — record the executor's own reported change accounting
next to the real `git diff` for a shell-driven run, and confirm the guard, not the
stream, decided `committed` (the #251 progress-asymmetry check). Also drive a
deliberate `--max-minutes-per-issue` kill → `Timeout`, `non_green`.

## Phase 3 — usage & billing (the reconciliation the first note skipped)

1. **Store is source of truth, WAL-safe** — confirm `scan_opencode` copies the
   `.db` + `-wal`/`-shm` before reading (the under-count trap it documents) and
   sums per-call rows.
2. **Interactive-session scan** — `ralphy usage` / daemon `GET /api/usage`
   (`scan_opencode`) reports a **real token number** for interactive OpenCode
   sessions, matching the store; no session with a row reports `null`, no session
   without a row reports a fabricated number. Ephemeral daemon so
   `daemon-require-login` is untouched.
3. **The unit mismatch** — put Ralphy's per-run token total (and USD projection)
   next to the provider's billing for the same run. State plainly what Ralphy's `$`
   is (an ADR-0034 list-price counterfactual) versus what the provider actually
   charges (a hosted-gateway plan / billing cycle, possibly flat).

## Phase 4 — one-shot / triage flows

Not covered by #29. `ralphy triage --agent opencode` drives a live judgment through
the native stream; `diagnose` / `draft-issues` via `ralphy init` on the same repo;
confirm each one-shot builder injects the same `OPENCODE_CONFIG_CONTENT` skills
container and never writes outside `.ralphy/`. Record the per-issue token cost vs
another vendor for the ADR-0038 budget.

## Phase 4b — the swallowed limit (D9) — the marquee phase

Reach a **real** provider quota (the 5-hour or billing-cycle cap). Capture:

- What Ralphy sees on the client: does `execute()` return `saw_error = false` and
  run to the wall timeout, or does any typed limit reach the adapter?
- What the OpenCode **server log** carries that the client stream does not — the
  swallowed error the [[opencode-silent-quota-timeout]] finding named.
- Whether `parse_opencode_limit` matches anything real, or whether the wall timeout
  is genuinely the only backstop (validating the D9 thesis) — and, if so, whether
  the honest fix is a shorter default budget for this vendor or a server-log tail.
  Record the exact message, exit code and any reset hint. Promote or amend the
  detector on the strength of the captured string.

## Phase 5 — host hygiene / residue audit

- OpenCode config and `auth.json` byte-identical before/after every run.
- Nothing token-bearing written into the target tree; `.ralphy/plan.md` not left as
  a tracked modification (the #41 hygiene follow-up — confirm fixed or re-file).
- The session store's WAL sidecars are not left mid-checkpoint in a way the scan
  under-counts.
- Record any unasked artifact (debug log, update check, temp file) OpenCode writes
  outside the workspace.

## Phase 6 — cross-platform parity

Repeat Phase 1 on Linux/WSL (the first note was Windows-only). Confirm the shim
resolution, event schema, store topology, skills container and pricing are
identical; record any divergence as version skew or a real platform difference.

## What would have failed this validation (to confirm none did)

- A swallowed limit burning the wall timeout with no record anywhere that a limit
  occurred.
- `ralphy usage` inventing a number, or the WAL trap under-counting a real one.
- Config / `auth.json` mutated across a run.
- A push or opened PR from any phase.
