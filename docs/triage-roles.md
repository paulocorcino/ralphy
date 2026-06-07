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

## Per-repo label strings

These canonical names are the defaults. A repo may map a role to a different
actual label string via `docs/agents/triage-labels.md` (written by Matt Pocock's
setup skill); Ralphy reads the `ready-for-agent` mapping from there when present.
The right-hand column of that file is the source of truth for what string a repo
actually uses.
