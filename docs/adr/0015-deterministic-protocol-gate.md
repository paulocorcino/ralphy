# Deterministic protocol gate at DONE acceptance

Status: accepted.

ADR-0011 made green mean "the runner *saw* the verify commands pass". Two holes
remained, both at the same point of the flow — accepting `RALPHY_DONE_EXIT`:

1. **The gateless close.** When verify resolution lands on `NoGate` (no
   `## Verify` in the plan and no `settings.json` `verify.command` fallback),
   the runner closed the issue on the agent's self-report with only a `warn!`.
   That was the one path where a false `RALPHY_DONE_EXIT` closed an issue
   completely unchecked.
2. **Prompt-only protocol.** The executor charter demands a completion protocol
   at DONE — every step ticked, `## Handoff` and `## Plan friction` written,
   `## Self-review findings` recorded when the plan carries a self-review step,
   the acceptance ledger's `evidence:` filled in. All of it was enforced by
   prompt instructions alone; a session that skipped the protocol still closed
   green, and the omission was invisible on the issue.

This ADR moves both from prompt persuasion to deterministic runner code — three
coupled mechanisms in [runner.rs](../../crates/ralphy-core/src/runner.rs) at the
`Outcome::Done` acceptance point.

## Decision

### 1. `verify.require_verify_gate` (settings, default off)

A new optional boolean in `.ralphy/settings.json` (`verify.require_verify_gate`,
also a `ralphy config` key). When `true` and verify resolution lands on
`NoGate`, the runner does **not** close the issue on the self-report:

- the issue is labeled `ready-for-human` (the ADR-0014 human-gate label, so
  dependents park on it correctly),
- a comment explains why the close was withheld and what the human does next,
- the issue stays open and **the run continues** to the next issue.

Absent/`false` preserves the ADR-0011 behavior exactly: close with a loud
warning. `## Verify: none` is untouched either way — it remains the deliberate,
visible opt-out and still closes.

### 2. Protocol lint (detect + report)

When the executor emits DONE, the runner runs a structural lint over
`.ralphy/plan.md` ([protocol.rs](../../crates/ralphy-core/src/protocol.rs)):

- every `## Steps` checkbox is `- [x]` (no `- [ ]` left);
- `## Handoff` present and non-blank;
- `## Plan friction` present and non-blank;
- `## Self-review findings` present when the steps carry a self-review step;
- no `## Acceptance ledger` line still carrying planner placeholder
  `evidence:` text (empty, an `<angle-bracket template>`, or the planning
  prompt's literal template phrases).

The lint result (✓/✗ per check) is published in the issue's **close comment** —
the same honesty-artifact spirit as the verify-gate comment: the operator reads
in the morning exactly which protocol artifacts backed the close.

### 3. Protocol lint (enforce, one bounce)

On a lint violation, the runner hands the session back to the executor **once**
via `.ralphy/protocol-failure.md` — the same vendor-neutral hand-back mechanism
as `verify-failure.md` — naming exactly which structural checks failed and
requiring each be satisfied honestly (finish the work, write the artifact;
never tick-without-doing). After that one bounce the runner re-runs the SAME
checks:

- pass → normal close path (verify gate still applies afterwards);
- second violation → fall back to today's behavior: close, with the ✗ report
  and a loud warning in the close comment for the human reviewer.

A usage limit hit during the bounce stops the run on the reset (the same global
stance as execute- and repair-time limits); the bounce's tokens are accounted
as their own `protocol-repair` ledger phase (ADR-0008).

## Presence, not truthfulness

The lint checks **presence and shape only**. It can tell that a `## Handoff`
exists, not that it is true; that an `evidence:` line was filled, not that the
named test proves the criterion. Judging content would require another model
call (the exact trust loop this gate exists to break) — so truthfulness stays
where ADR-0011 left it: with the human at merge, now aided by an explicit ✓/✗
report of what was and was not structurally present.

Order of gates at DONE: protocol lint (with its one bounce) first — it is cheap
and purely structural — then the ADR-0011 verify gate over the settled plan,
then the close.

## Consequences

- A false `RALPHY_DONE_EXIT` can no longer close an issue unchecked: with the
  flag on, a gateless done becomes a parked human gate; with it off, the close
  comment at least carries the structural evidence of the protocol.
- Protocol compliance costs a bounded amount: at most one extra execute per
  issue, visible in the ledger as `protocol-repair`.
- Scripted/test agents must produce protocol-clean plans (steps ticked,
  closing sections present) or explicitly script dirty ones — the queue tests'
  `ScriptedAgent` defaults to clean.
