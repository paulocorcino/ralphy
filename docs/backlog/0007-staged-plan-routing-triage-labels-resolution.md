# Staged-plan routing + triage-labels.md resolution

**Type:** AFK
**Triage:** needs-triage
**Spec:** [CONTEXT.md](../../CONTEXT.md), [docs/triage-roles.md](../triage-roles.md)

## What to build

Two repo-driven planning refinements: route `stagedplan`-labelled issues through
the staged-plan prompt, and read a target repo's own triage-label mapping to widen
the queue. Parity oracle: the ps1 `stagedplan` routing and `Resolve-TriageLabels`.

Scope:
- An issue labelled `stagedplan` is planned via `prompt.plan.staged.md` (staged-plan
  skill, non-interactive env flag) instead of the standard `prompt.plan.md`.
- Read `docs/agents/triage-labels.md` in the **target** repo: if it maps
  `ready-for-agent` to a different label string, add that to the queue label set.
- `--queue-label` (repeatable) replaces the whole set.

## Acceptance criteria

- [ ] A `stagedplan` issue is planned with the staged prompt; others use the standard one.
- [ ] When the target repo has `docs/agents/triage-labels.md`, the mapped
      `ready-for-agent` label is added to the queue set.
- [ ] Passing `--queue-label` replaces the default set entirely.

## Blocked by

- #5 (queue loop)
