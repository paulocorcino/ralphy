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


## Amendment (2026-07-29): the auth detector only judges a run that FAILED

The mid-run checkpoint above was applied to the child's whole combined
stdout+stderr, unconditionally, at the end of every execute call. That log is
not the CLI's diagnostics — it is the CLI's diagnostics *plus everything the
agent read*. Every adapter documents its own auth banner in its own `auth.rs`,
so the detector self-triggers the moment an agent greps its adapter's sources.

Observed on 2026-07-29 (#356): a Codex execute finished green — clean exit,
`RALPHY_DONE_EXIT`, eleven commits, self-review clean — and the run was then
aborted with "Codex is not authenticated (401 Unauthorized) — run `codex login`
and retry" against an authenticated account, because a repo-wide grep had echoed
`ralphy-agent-codex/src/auth.rs` into the log. The error propagates out of
`Agent::execute` through `runner.rs`'s `?`, so the phase never reached its
outcome: no ledger row, no verify gate, no close, and the rest of the queue was
abandoned. An hour of flagship execution survived only as commits.

**The execute-time auth bail is now reached only when the vendor run did not
exit cleanly** (`run_exec_session`, `ralphy-adapter-support`). This preserves the
checkpoint's whole purpose — it exists to explain a failure that would otherwise
masquerade as `Outcome::Stuck` — and rests on the measurement this ADR already
records: a logged-out `codex exec` *exits 1* after its retry storm. The plan-time
bail needed no change; it was already reached only when no plan landed.

The Claude adapter had reached the same conclusion from the other side, skipping
`user`/`assistant` transcript records so tool results cannot self-trigger
(`is_claude_auth_error`). Content anchoring alone is not enough for a vendor whose
output has no envelope: this repo's own test fixtures hold verbatim copies of the
genuine banner, so a grep of them is indistinguishable from the real thing. The
run's *outcome* is the signal that cannot be forged by something the agent read.

If a vendor CLI is ever observed exiting `0` on an auth failure, this gate is
what has to change — not the detector.
