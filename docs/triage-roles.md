# Triage roles (canonical reference)

> **Credit:** these roles are **[Matt Pocock](https://github.com/mattpocock)'s**,
> defined in his [engineering skills](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).
> Ralphy adopts them verbatim and does not redefine them.

The triage vocabulary Ralphy speaks is Matt Pocock's canonical set of five roles.
They form a **state machine**: an incoming issue moves through evaluation, may
bounce back for more information, and ends up either agent-ready, human-ready, or
closed without action. The roles are **mutually exclusive** — an issue occupies
exactly one at a time.

This file is reference only; the decision to follow these roles (and to keep flow
control separate) is recorded in [adr/0001-triage-vocabulary-and-stop-before.md](./adr/0001-triage-vocabulary-and-stop-before.md).

## The five roles

| Canonical label   | Purpose                                                        | Ralphy's behaviour                                            |
| ----------------- | ------------------------------------------------------------- | ------------------------------------------------------------ |
| `needs-triage`    | Maintainer still needs to evaluate the issue.                 | Ignored — not in the queue.                                   |
| `needs-info`      | Blocked, waiting on the reporter for more information.        | Ignored — not in the queue.                                  |
| `ready-for-agent` | Fully specified, AFK-ready: an agent can pick it up with **no** human context. | **The queue.** Worked in ascending issue-number order, then closed on green. Alias: `AFK`. |
| `ready-for-human` | Requires human implementation.                                | Never queried, so never worked. Alias: `HITL`.               |
| `wontfix`         | Will not be actioned.                                         | Ignored — not in the queue.                                  |

## State machine

```
            ┌───────────────┐
new issue → │ needs-triage  │
            └───────┬───────┘
        ┌───────────┼─────────────┬───────────────┐
        ▼           ▼             ▼               ▼
  ┌───────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────┐
  │ needs-info│ │ready-for-agent│ │ready-for-human│ │ wontfix  │
  └─────┬─────┘ └───────┬──────┘ └──────────────┘ └──────────┘
        │ (reporter      │ Ralphy works it,
        │  replies)      │ closes on green
        └──► needs-triage ▼
                     (closed)
```

Only `ready-for-agent` is Ralphy's concern. The other four are maintainer/human
states it deliberately leaves alone.

## What is NOT a triage role

`stop-before` is a **flow-control** label, not a triage role. It does not describe
an issue's readiness — it tells a running Ralphy queue to pause before working
that issue. It sits outside this vocabulary on purpose; see CONTEXT.md and the
ADR.

`triage-agent` is an **operational** label, not a triage role (ADR-0017). It marks
"an agent triage pass (`ralphy triage`) will evaluate and normalize this issue
before it enters the queue". Like `stop-before`, `AFK`, and `HITL` it is fixed and
non-configurable — it stays out of the `docs/agents/triage-labels.md` mapping — and
`ralphy init` syncs it automatically. It is also a **human-return** label: while
present it parks the issue out of the run queue (ADR-0016), so triage and run never
race. `ralphy triage` consumes it, then swaps it for `ready-for-agent` (promote /
consolidate), `needs-info` (bounce), or `ready-for-human` (escalate). A `promote`
also posts a marked **evidence-stamp** comment recording the evidence gate it
passed, so the AFK judgment is auditable rather than a bare label flip (ADR-0027). The two
human-return arms split the debt they used to conflate: `bounce` = the reporter
owes information → `needs-info`; `escalate` = a maintainer owes a decision →
`ready-for-human` (ADR-0018 §3). The promotion bar for promote/consolidate is
the ADR-0018 evidence gate — confirmable at source, localizable, contract-preserving
— not spec executability alone; see
[adr/0018-triage-evidence-gate-and-escalate.md](adr/0018-triage-evidence-gate-and-escalate.md).

### Human-return precedence (ADR-0016)

A label that returns an issue to a human outranks any queue label. When a queued
issue also carries `ready-for-human`/`HITL`, `needs-info`, `needs-triage`,
`wontfix`, or `triage-agent`, the run **skips it with a visible reason and
continues** — the human side wins. `--only-issue` does not override a human-return
label (unlike its `stop-before` override): removing the label is the explicit human
act that re-opens the door.

## Per-repo label strings

These canonical names are the defaults. A repo may map a role to a different
actual label string via `docs/agents/triage-labels.md` (written by Matt Pocock's
setup skill); Ralphy reads the `ready-for-agent` mapping from there when present.
The right-hand column of that file is the source of truth for what string a repo
actually uses.
