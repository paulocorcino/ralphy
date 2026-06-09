# Usage-limit handling: auto-resume by default, from git + `plan.md` (never `claude --resume`)

When a Ralphy execution session hits a subscription usage/rate limit, the run
**waits for the reset and resumes the same issue by default**, and it resumes by
re-running the issue against the committed git history and the live
`.ralphy/plan.md` checklist — never by reattaching to the interrupted Claude
conversation via `claude --resume <session_id>`.

Two orthogonal decisions are recorded here. They are separable — you could
auto-resume via `--resume`, or stop and never resume — so each is argued on its
own.

## D1 — Auto-resume is the default; stopping is the opt-out

Previously a usage limit stopped the run and the operator re-ran manually
(`ralphy.ps1`: *"a usage limit is treated as a stop … you re-run manually"*). We
reverse that default: *mission given is mission accomplished*. On `Outcome::Limit`
the runner blocks until the parsed reset time (plus a 5-minute buffer), then
retries the same issue. A `--stop-on-limit` flag on the queue restores the old
stop-and-report behaviour.

We accept this because the account has **no USD cap** (subscription, no API
spend), so the only cost of waiting is wall-clock time, which is acceptable for an
unattended overnight run. The trade-off is explicit: a slowly-progressing issue
can span several reset windows and run for many hours. It is bounded by two
guards — the global run deadline (a hard ceiling that wins the moment it passes)
and a **progress-aware cap**: two consecutive limit-resumes that commit nothing
(`HEAD` sha unchanged) abandon the issue, so a run never burns reset windows
forever on an issue that cannot move.

A usage limit with **no parseable reset time** falls back to stop-and-report —
auto-resume only engages when there is a trustworthy wake time, because inventing
a fixed sleep risks waking early and re-limiting. A reset that lands **beyond the
run deadline** does not sleep at all; the run stops with `Deadline` immediately.

## D2 — Resume is git + `plan.md`, never `claude --resume <session_id>`

The execution charter is the durability contract: each plan step is committed as
it lands, `plan.md` checkboxes plus a `## Notes & decisions` section are kept
honest at every stopping point, and the agent commits *before* emitting any exit
token (`prompt.execute.md`). A fresh session restarts from those artifacts alone.
The headless `-p` loop already proves this end to end: each call is an independent
process with no session continuity, and progress is detected purely by a change in
the `HEAD` sha.

We rejected `claude --resume <session_id>` even though it would preserve the
uncommitted reasoning of the step in flight, because:

- It contradicts the invariant above by introducing a *second*, divergent resume
  mechanism with its own failure modes, over a contract that already makes resume
  work without it.
- The headless mode cannot use it at all (each `-p` call is a fresh process), so
  adopting it would split resume behaviour across the two execution modes the
  codebase deliberately keeps symmetric — both already converge on
  `Outcome::Limit`.
- Ralphy does not capture a `session_id`; the only handle is "newest `.jsonl`
  transcript", which is racy — any other `claude` invocation on the machine
  produces a newer transcript and would be resumed by mistake.
- `--resume` replays the entire prior conversation back into context, re-spending
  tokens on an account that has just returned from a usage limit — the opposite of
  what is wanted at that moment.

The one case `--resume` would genuinely help — a single large step not yet
committed when the interruption hits — is already the behaviour on a timeout
today, and is mitigated at the source by the charter (split a partial step, commit
before stopping) and by planning granularity, not by session reattachment. The
remedy for "steps too big to re-run cheaply" is finer plan steps, not resuming the
LLM session.

## Consequences

- Auto-resume re-runs **`execute()` only, never `plan()`**: `plan()` deletes
  `plan.md` at the start of every call ("plan fresh every run"), which would
  destroy the very checkboxes the resume depends on. The retry loop wraps
  `execute()` against the existing on-disk `plan.md`.
- Because `.ralphy/plan.md` is gitignored, this in-process resume — which keeps the
  file on disk — preserves both the git history *and* the checkbox progress. A
  detached/cold resume would keep only git; that is the reason the wait is
  in-process and blocking rather than a self-rescheduled process.
- The wait is a poll loop (not one long sleep) so it stays interruptible by a
  process kill and emits a periodic heartbeat for an operator watching the
  terminal. During the wait there is no live Claude session, so the mobile remote
  control cannot reach the run — observability during the wait is terminal-only.
- Re-run cost is bounded by step granularity, which the charter and the planner
  already own.
