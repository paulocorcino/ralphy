# `ralphy run` keeps auth as a mid-run adapter checkpoint

Status: accepted.

Issue #65 added a `ralphy run` preflight gate for agent binary presence: fail
before queue work if the selected planner/executor CLI is not on `PATH`. Issue
#66 asked whether that preflight should also prove the selected agent is logged
in.

We tested the logged-out path against both supported subscription agents before
deciding:

- `codex exec`, with no `~/.codex/auth.json`, exits `1` after its own retry storm
  and emits `401 Unauthorized` / missing authentication text. The Codex adapter's
  mid-run detector maps that to the actionable error:
  "Codex is not authenticated (401 Unauthorized) - run `codex login` and retry".
- `claude -p`, with `claude auth status` reporting `loggedIn: false`, emits
  `Not logged in · Please run /login`. The Claude adapter's mid-run detector maps
  that to the actionable error:
  "Claude Code is not authenticated - run `claude login` and retry".

Both end-to-end `ralphy run --agent <agent> --queue-label needs-triage
--no-telegram` tests reached the issue planning step, failed on the adapter auth
signal, restored the branch to `main`, and left the worktree clean.

## Decision

**`ralphy run` preflight verifies only agent binary presence, not agent login
state.** Authentication remains the adapter's responsibility and is detected at
the first real agent invocation during planning or execution.

Presence is a cheap, deterministic local check: locate `claude`, `codex`, or the
configured planner/executor CLI on `PATH`. Authentication is not equivalent.
There is no common cheap local auth probe across the supported CLIs that proves a
real run will work. A reliable auth gate would need to spawn the agent itself,
which turns preflight into an extra agent session.

For Codex in particular, the only observed reliable check was a real `codex exec`
call. When logged out, it spent roughly 21 seconds retrying WebSocket/HTTPS
connections before returning the 401 signal. Paying that cost before every healthy
run would optimize for the uncommon logged-out case while slowing the common
logged-in path.

Claude is cheaper when logged out (`claude -p` returns the login signal quickly),
but using a vendor-specific auth gate would make preflight inconsistent while the
adapters already expose the same operator-facing behavior: fail early in planning
with a direct login command and no repository debris.

## Consequences

- #65 stays scoped to binary presence. If `claude` or `codex` is missing from
  `PATH`, `ralphy run` fails before queue construction.
- If the CLI is present but logged out, the run may build the queue and create the
  temporary run branch before the adapter discovers auth failure. That setup is
  reversible, and the tested cleanup path restores `main`, drops the empty run
  branch, and leaves no worktree changes.
- The adapter auth detectors are the single auth checkpoint for `run`. They must
  keep returning clear, vendor-specific remediation messages.
- No follow-up AFK slice is needed for an auth preflight gate.
- A separate UX wart remains: a run can emit the "queue built" notification before
  dying on auth. That can be improved independently without changing the auth
  checkpoint decision.

## Considered options

- **Preflight by spawning the selected agent.** Rejected: it duplicates the first
  real planning invocation and adds agent startup/auth-probe cost to every
  successful run. Codex showed this can be a long retry storm, not a cheap local
  check.
- **Preflight only the agents with cheap status commands.** Rejected: it creates
  vendor-specific behavior and still cannot prove the exact later invocation will
  succeed.
- **Inspect local auth files.** Rejected: file layout is private CLI state, differs
  by vendor/version, and can be wrong relative to the actual runtime environment.

