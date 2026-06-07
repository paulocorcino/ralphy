# Full queue loop: close-on-green + stop-at-first-non-green

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0002](../adr/0002-core-agnostic-adapter-boundary.md), [CONTEXT.md](../../CONTEXT.md), [docs/triage-roles.md](../triage-roles.md)

## What to build

`ralphy run --repo <r>` works the whole queue: build the queue, iterate in order,
close each green issue, and stop the run the moment one issue is non-green —
handing back the branch as it stands. Parity oracle: the ps1 overnight run over a
queue.

Scope:
- Queue building: union across the queue labels (`ready-for-agent` + `AFK`),
  dedupe by number, ascending order. `gh --label` is an AND filter, so query each
  label and union.
- Iterate; per issue run plan → execute (slice 0003) → outcome.
- Close-on-green: a green queue issue is closed with a comment pointing at the run
  branch; the label is left untouched (the **cycle** in CONTEXT.md).
- Stop-at-first-non-green: BLOCKED / timeout / stuck stops the whole run.
- `--deadline-hours` global budget: don't start a new issue past it.

## Acceptance criteria

- [ ] An issue carrying any queue label is picked up; issues are worked ascending
      by number; duplicates across labels appear once.
- [ ] A green issue is closed with a run-branch comment; its label is unchanged.
- [ ] The first non-green issue stops the run; earlier green issues stay committed
      and closed; the branch is handed back.
- [ ] `--dry-run` never closes an issue.
- [ ] The global deadline prevents starting a new issue past the budget.

## Blocked by

- #3 (interactive execute)
