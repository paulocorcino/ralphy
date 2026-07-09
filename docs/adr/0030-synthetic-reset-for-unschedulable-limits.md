# A synthetic reset window for usage limits that carry no schedulable time

Status: accepted (implemented 2026-07-09).

A usage limit reaches the runner as `Outcome::Limit(reset: Option<String>)`. When
the vendor gives a parseable reset time (Codex an absolute RFC3339 instant, Claude
a relative one), the run auto-resumes: `RunClock::wait_for_reset` sleeps to that
target and re-runs the phase. But when the limit carries **no** schedulable reset —
Kimi's HTTP-403 `access_terminated_error` (whole-account billing-cycle quota) only
promises "the next cycle", and its exit-75 chat limit is likewise timeless
([ADR-0028](./0028-kimi-adapter.md) D9) — the runner had nothing to wait on and so
**stopped and reported** the limit. In practice that ended the run the moment an
account was throttled, mid-queue, leaving 10+ issues unattempted until a human came
back to re-run it by hand. This was observed live on a Kimi run (issue #46 misread
as `stuck`, then correctly as `Limit(None)`, then stopping the whole queue).

The operator's intent for an unsupervised overnight/weekend run is the opposite:
**wait it out.** A billing cycle resets on its own; the run should pause, keep
checking, and resume when the quota returns — for as long as it takes, with the
human deciding when to give up.

## D1 — Treat "no reset" as "retry in ~30 min", reusing the wait we already have

Rather than add a second waiting mechanism (a fixed-interval poller) alongside
`wait_for_reset`, a `Limit(None)` **synthesises** a reset hint of `now + 25min`
(`runner::clock::synthetic_reset`) and feeds it through the existing
`wait_for_reset` path. That path already parses the hint, adds its 5-minute policy
buffer (so the effective wake is ~30 min out), emits a heartbeat, and honours the
run deadline — all reused unchanged. Each cycle re-synthesises from the current
clock, so a still-limited retry simply parks another ~30-min window. The loop is
unbounded: it ends only when the run's global deadline (`--minutes`) cuts it, or a
human interrupts. This is deliberately a **blind** poll — there is no cheap probe
for "is the quota back", so the retry itself is the probe.

## D2 — The no-progress cap guards the scheduled path only

`execute_phase` abandons an issue after two consecutive no-commit limit resumes, so
a genuinely stuck issue does not burn reset windows forever. A synthetic wait makes
no per-issue progress **by definition** — the whole account is paused, not this
issue — so counting it against that cap would abandon every issue the moment the
quota ran out, defeating D1. The synthetic path is therefore exempt from the cap;
the cap still applies to a real, scheduled reset. Resolving the underlying
no-progress question is the human's call: re-running continues the work.

## D3 — Auto-resume is now the default for every agent; `--stop-on-limit` opts out

With D1, a limit that carries no parseable reset is no longer useless to auto-resume
on — so the per-agent force that pinned Kimi and OpenCode to `stop_on_limit`
(previously justified by "no schedulable reset, auto-resume is never useful") is
removed. All four adapters now auto-resume by default; `--stop-on-limit` is the
single, explicit opt-out for contexts that must not hang (e.g. CI, where a run
should fail fast rather than park for hours — a bounded `--minutes` also caps the
wait). This reverses the "no-reset ⇒ stop" fail-safe that
[ADR-0028](./0028-kimi-adapter.md) D9 relied on; that fail-safe existed only
because there was no wait to fall back on, which D1 now provides.

## D4 — Ctrl-C ends the run without consolidation (accepted, not yet cooperative)

During a synthetic wait the run may park for hours or days; the human stops it with
Ctrl-C. Today there is no cooperative signal handler, so Ctrl-C terminates the
process immediately: the `wait_for_reset` sleep (1-second slices) is cut and the run
ends **without** the normal end-of-run consolidation/finalization (`finalize_run`
does not run). This is an accepted limitation, not a goal — a graceful,
consolidating interrupt would require a SIGINT handler setting a cooperative cancel
flag observed by the wait loop, and is left as follow-up. Recorded here so the
absence is a decision, not an oversight.
