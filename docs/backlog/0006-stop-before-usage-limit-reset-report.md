# stop-before pause + usage-limit stop + reset-time report

**Type:** AFK
**Triage:** needs-triage
**Spec:** [ADR-0001](../adr/0001-triage-vocabulary-and-stop-before.md), [CONTEXT.md](../../CONTEXT.md)

## What to build

The two remaining run-control behaviours that stop a queue cleanly: the
`stop-before` flow-control label and usage-limit handling. Parity oracle: the ps1
`stop-before` pause and its usage-limit stop report.

Scope:
- `stop-before`: a fixed (non-configurable) label on a queued issue halts the run
  **before** that issue — issues earlier in the sequence still run. `--only-issue`
  overrides it. The repo is left on the run branch; remove the label and re-run to
  continue. (Flow control, not a triage role — see ADR-0001.)
- Usage-limit detection: recognise a rate/usage-limit in the transcript and treat
  it as a stop (no USD cap — subscription).
- Best-effort parse of the reset time for the stop report ("Resets ~HH:mm; re-run
  after that").

## Acceptance criteria

- [ ] A queued issue labelled `stop-before` halts the run before it; earlier issues
      ran; the repo is left on the run branch.
- [ ] `--only-issue N` ignores the `stop-before` marker.
- [ ] A usage limit stops the run and reports the reset time when parseable.

## Blocked by

- #5 (queue loop)
