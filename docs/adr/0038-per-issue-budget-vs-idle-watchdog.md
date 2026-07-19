# ADR-0038 — The per-issue budget is not a liveness watchdog

**Status:** accepted
**Date:** 2026-07-19
**Amends:** ADR-0011 (the verify gate no longer borrows the per-issue budget)
**Supersedes the resolution of:** issue #150

## Context

`DEFAULT_MAX_MINUTES_PER_ISSUE` changed value four times in ten days:

| Commit | Date | Change |
|---|---|---|
| `ec56534` | 2026-07-03 | `90` → `0`, documented as "no per-issue cap by default" |
| `e273400` | 2026-07-08 | `0` → `60`, resolving #150, plus a test pinning the finite default |
| `74881b2` | 2026-07-13 | `60` → `0`, a one-line change riding inside a Telegram commit |

That is not indecision. One number was being pulled by three incompatible
requirements:

1. **A productivity cap** — "this issue does not deserve more than N minutes."
   Wants `0`/unbounded, because a wall clock cannot distinguish a healthy long
   issue from a wedged child, and cutting on elapsed time kills real work.
2. **A liveness watchdog** — "the child is stuck; rescue the run." Wants a finite
   value. This is what motivated #150, whose reported symptom was *"the executor
   came up with `budget_min=0` … a child stuck indefinitely (e.g. wedged on
   `Waiting for API response` without exiting) would make ralphy wait forever."*
3. **An input to derived timeouts** — the ADR-0011 verify gate. Wants a finite
   value **always**.

The collision was already visible in the code: `run.rs` needed a
`match { 0 => VERIFY_GATE_FALLBACK_MINUTES, n => n }` because `0` would otherwise
collapse the verify gate to a 0-second timeout. When a value needs an exception
to avoid meaning opposite things, it is more than one concept.

#150 asked for a progress clock and received a wall clock. It worked for its
symptom and charged case 1 for it. The `0` in `74881b2` was the right value
arriving without the reasoning, leaving the constant contradicting its own
docstring, the `--max-minutes-per-issue` help text, and the test #150 had added.

## Decision

Split the roles. Each clock measures the thing it is named after.

### 1. `max_minutes_per_issue` is an opt-in productivity cap, `0` by default

Unset means **no per-issue cap**; the issue is bounded only by
`--deadline-hours`. `0` is not a sentinel to be defended against — it is the
default, and it is honest: only an operator who asked to be cut gets cut.

### 2. Liveness is an idle watchdog, on by default

A new vendor-neutral `IdleWatch` in `ralphy-adapter-support` keys on **progress**
rather than duration. Configured by the agent-agnostic `--idle-minutes` flag /
`idle_minutes` setting; `0` disables it.

Two defaults, because the two execution paths have different progress signals —
and the signal, not the threshold, is the load-bearing choice:

| Path | Progress signal | Default |
|---|---|---|
| headless | any byte on stdout/stderr | **20 min** |
| interactive (PTY) | agent transcript growth **only** | **45 min** |

PTY bytes are deliberately *not* a progress signal: the TUI redraws its spinner
forever, so bytes keep flowing from a thoroughly wedged child. Transcript growth
is the only honest signal there, and it is coarser — a legitimate 30-minute tool
call advances nothing while it runs — so it must buy more slack before it is
allowed to kill. Two values here is the point of this ADR, not an inconsistency.

An explicit `--idle-minutes` applies to whichever path runs: the operator naming
a number means it.

### 3. Derived timeouts own their clocks

The verify gate reads `verify.timeout_minutes` (default:
`VERIFY_GATE_FALLBACK_MINUTES`) and never inherits the per-issue cap.

## Consequences

- **The per-issue cap stops being a safety net.** Anyone tempted to restore a
  finite default to catch a hang should arm the watchdog instead; that trade was
  made once and cost case 1 to buy case 2.
- **A silently-retried provider quota block is now caught.** OpenCode swallows a
  provider cap into a silent retry that only ever surfaces in a server-side log,
  so `saw_error=false` and the stderr matcher of ADR-0005 never fires. No text
  matcher can see a failure that is never printed — the resulting silence can.
- **Classification is untouched.** An idle kill reports as the `Timeout` the
  runner already understands (the ADR-0023 ladder is unchanged, no new `Outcome`
  variant). This mirrors the constraint ADR-0004/#149 imposed on the
  API-degraded watch.
- **One event for both paths.** The two drivers measure progress differently but
  emit the *same* canonical message (`IDLE_REAPED_MSG`), which the CLI decodes
  into a single `RunEvent::IdleReaped { idle_minutes }` → one console line, one
  Telegram push, one `dev.ralphy.run.idle_reaped` CloudEvent. The asymmetry stops
  at the progress signal; it must not reach the operator. It is emitted at `INFO`
  deliberately: the decoder folds `WARN`/`ERROR` into a generic notice, which
  would dissolve the reap back into per-path noise.
- **A reap is not a recovery.** Clearing the `degraded` flag on a reap would
  otherwise trip the #149 matched-pair edge and push "API recovered, resuming"
  about a child Ralphy had just killed. The reap suppresses that edge and speaks
  for itself — a degraded episode can end by recovery *or* by execution, and only
  the first is good news.
- **No respawn on idle.** The #149 watch re-spawns once on a persistent API
  banner because a visible retry is evidence the child is trying. Silence is
  evidence of nothing, so the idle path ends the drive instead of spending a
  respawn on a guess.
- **A new default can kill healthy work if the signal is wrong.** The mitigation
  is a test, not a threshold: a child that keeps emitting must survive a window
  far shorter than its runtime.

## Known wart (not addressed here)

`max_minutes_per_issue` lives under the `claude.*` settings section but is fed to
the Codex, Kimi and OpenCode adapters through `ResolvedClaude`. That is the same
convergence vice at the configuration layer, but unwinding it is a settings
migration — tracked separately. The new `idle_minutes` knob is top-level
precisely so it does not repeat the mistake.
