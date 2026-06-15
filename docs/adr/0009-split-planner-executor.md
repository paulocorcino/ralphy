# Split planner/executor adapters (`--plan-agent`)

A run may use different adapters for planning and execution: `--agent` selects
the executor, the new `--plan-agent` selects the planner and **defaults to the
`--agent` value**, so the single-agent path (`ralphy run --agent claude`) is
byte-for-byte unchanged. The canonical use is `--agent opencode --plan-agent
claude`: Claude plans on the subscription, OpenCode's coder model executes.

## Decision

Wire the split with a composition-root `SplitAgent` wrapper that delegates
`plan()` → planner and `execute()` → executor, holding two `Box<dyn Agent>`. The
core's `Agent` trait and the `run_queue`/`run` signatures stay **untouched** — the
core still sees one `Agent` and never learns it is split (consistent with
ADR-0002 and ADR-0004 D1). Any planner/executor combination is allowed without
validation; the plan artifact is vendor-neutral markdown, so any planner's plan
is executable by any executor.

Usage-limit handling becomes **per-phase**: `QueueConfig.stop_on_limit` splits
into `stop_on_limit_plan` and `stop_on_limit_exec`, each derived from the agent
that serves that phase (via the existing `effective_stop_on_limit`). So a
Claude planner can auto-resume through a plan-time reset while an OpenCode
executor still stops on an execute-time limit. The explicit `--stop-on-limit`
flag still forces both phases.

## Considered options

- **Thread two agents through `run_queue`/`run`.** Honest per-phase agent names
  by construction, but changes the core signatures and forces edits across the
  ~35 `run_queue` call sites in `tests/queue.rs`. Rejected for blast radius: the
  wrapper achieves the same behavior with a CLI-local type and zero core/test
  churn.

## Consequences

- The wrapper reports a single identity via `name()` (the executor's). The
  runner stamps `agent` on **both** the plan and execute ledger lines from
  `agent.name()`, so in a split run the **plan-phase ledger line carries the
  executor's name** even though Claude produced those tokens. The `model` column
  stays per-phase-true because each adapter fills its own `usage.model` — a
  future reader seeing `agent=opencode, model=opus` on a plan line should read it
  as "split run", not a bug. This is the deliberate price of the zero-churn
  wrapper (see ADR-0008 D6 for the per-phase ledger contract).
- `QueueConfig` gains one field; the four `cfg*` test helpers in
  `tests/queue.rs` are the only constructors and absorb the change.
- A `dry_run` split run exercises only the planner (execution is skipped).
