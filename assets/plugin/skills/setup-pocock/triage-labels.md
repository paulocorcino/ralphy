# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.

## Fixed operational labels

These are **not** triage roles and are **not** renameable. Ralphy applies them by
literal string from the code, so remapping them here has no effect — the runner
would keep applying the canonical name. They are listed so the skills recognise
them when they read an issue, and so `ralphy init` and this table agree.

| Label                | Applied by         | Meaning                                                                             |
| -------------------- | ------------------ | ----------------------------------------------------------------------------------- |
| `AFK`                | operator           | Queue member: the agent may work this issue unattended                                |
| `HITL`               | operator           | Human-in-the-loop required before an agent can continue (human gate, ADR-0014)        |
| `stop-before`        | operator           | Flow control: the run halts *before* reaching this issue                              |
| `triage-agent`       | operator / `triage`| Awaiting an agent triage pass (`ralphy triage`, ADR-0017) before it enters the queue  |
| `needs-split`        | the runner         | The planner judged the issue a bundle; parked until a human splits it into children   |
| `needs-human-review` | the runner         | Closed **green**, but the acceptance ledger left `[review-only]` criteria for a human |

`needs-human-review` marks **attention** debt, not unfinished work: a review-only
criterion is usually delivered, it just has no machine-checkable evidence. It
lands on a *closed* issue and is deliberately **not** a human gate — unlike
`ready-for-human`/`HITL`, it never parks anything out of the queue. Find the
backlog with `is:closed label:needs-human-review`.
