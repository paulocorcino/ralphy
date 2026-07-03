# Human-return labels outrank queue labels

Status: accepted and implemented (#89).

The queue is built by **positive selection only**: `gh issue list --label
<queue-label>` per configured queue label, unioned and sorted
([github.rs](../../crates/ralphy-core/src/github.rs) `build_queue`). After
that, the only label of the issue *itself* that the run loop inspects is
`stop-before` ([runner.rs](../../crates/ralphy-core/src/runner.rs)). The
human-gate labels (`ready-for-human`/`HITL`) matter only when the issue appears
as a **blocker** of another issue (ADR-0014) — never on the issue being worked.

Two consequences of that gap:

1. **Contradictory labels execute.** An issue carrying `AFK` + `needs-info`
   (waiting on its reporter) or `AFK` + `ready-for-human` is worked normally.
   ADR-0001 assumed the roles are mutually exclusive; nothing enforces it.
2. **The ADR-0015 park is defeated.** When `verify.require_verify_gate` parks a
   gateless done, the runner labels the issue `ready-for-human` and leaves it
   open — but Ralphy never removes labels anywhere in the codebase, so the
   queue label stays. The next `ralphy run` re-queues and re-works the very
   issue the park meant to hand to a human.

## Decision

A precedence rule, deterministic and enforceable in one place: **labels that
return an issue to a human outrank labels that queue it for the agent.**

An issue is worked iff it carries a queue label AND none of the
**human-return labels**:

- `ready-for-human` / `HITL` (the ADR-0014 human gate, resolved through the
  repo's triage mapping like every triage role),
- `needs-info` (waiting on the reporter),
- `needs-triage` (waiting on a maintainer's triage pass),
- `wontfix` (will not be actioned),
- `triage-agent` (waiting on an agent triage pass — ADR-0017).

Enforcement point: a self-label check in the run loop, beside the existing
`stop-before` check — **skip with a recorded reason and continue the queue**,
the same visibility contract as a blocked-by skip (runstate skip reason,
console line, Telegram note). We deliberately do not filter at queue-build
time: a silently absent issue looks like "not labelled", while a visible skip
tells the operator exactly which label parked it.

`stop-before` keeps its own distinct semantics (halt the run *before* the
issue) — it is flow control, not a return-to-human state, and is unchanged.

`--only-issue` does **not** override a human-return label (unlike its existing
`stop-before` override): `stop-before` is the operator's own pause mark, so the
operator naming the issue explicitly unwinds it; a human-return label may
record someone *else's* state (a reporter owing information, a parked verify
gate) that a run flag should not steamroll. Removing the label is the explicit
human act that re-opens the door.

## Amends

- **ADR-0015**: the park in mechanism 1 is now effective across runs — a
  parked issue stays invisible to subsequent runs until a human removes either
  label. No label removal is required for safety.
- **ADR-0001**: the canonical roles' mutual exclusivity is now enforced at run
  time rather than assumed. When both sides are present, the human side wins.

## Consequences

- The re-park bug is fixed without introducing label removal; `remove_label`
  (ADR-0017) is hygiene, not a safety requirement.
- Label state on the board becomes trustworthy: `needs-info` genuinely means
  "will not be worked until someone acts", whatever else the issue carries.
- A repo whose custom triage mapping renames these labels keeps the behavior:
  the check resolves names through the same `triage-labels.md` mapping the
  label specs use.
