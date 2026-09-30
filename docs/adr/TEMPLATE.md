# <Short title: the decision, not the problem>

<!--
HOW TO USE THIS TEMPLATE

Copy this file to docs/adr/NNNN-<slug>.md. NNNN is the highest number in
docs/adr/ plus one. Delete every comment block before you commit.

Write only when all three are true. Otherwise do not write an ADR:
  1. Hard to reverse: changing your mind later has a real cost.
  2. Surprising without context: a future reader of the code would ask "why
     is it done this way?"
  3. A real trade-off: there were real alternatives, and you chose one for
     stated reasons.

Plain English, short sentences, no idioms (see AGENTS.md). Use the words in
CONTEXT.md. Do not invent synonyms.
-->

Status: proposed
Kind: structural
Protects: <one or two characteristics>

<!--
Status — exactly one of:
  proposed | accepted | deferred | rejected | superseded by ADR-NNNN
Change it in the same commit that changes the fact. "proposed" means not yet
built. When the code ships, the status becomes "accepted". A status that
contradicts the code misleads every agent that reads it.

Kind — exactly one of:
  structural  decides a boundary: which crate or process may call which, a
              dependency rule, a disk or wire format, or who owns a fact.
              A structural ADR MUST fill the Compliance section.
  feature     decides how one behavior works.
  vendor      decides how one agent adapter works (see ADR-0040).
  process     decides how the repo is built, tested or released.

Protects — the architecture characteristic(s) this decision serves, from
docs/ARCHITECTURE.md §2: recoverability, integrity of change, consistency
of the workbench, extensibility, observability and cost, testability,
responsiveness, operability; or the constraints portability, security. This line answers
"why" in one word. It is what lets a later reader judge a trade-off.
-->

## Context

<!--
What forces this decision: the facts, the limits, what was measured (name the
tool and the date). One or a few short paragraphs. Evidence as file:line or
issue number. Do not narrate the history of the discussion.
-->

## Decision

<!--
What we decided, as short numbered rules (D1, D2, …) so code and other ADRs
can cite one rule. State each rule as a fact that is true or false about the
code ("The daemon never runs git"). Include the explicit "no"s: what this
decision forbids is often worth more than what it allows.
-->

**D1.** <rule>

## Consequences

<!--
Optional. Only effects that are not obvious: what becomes harder, what new
cost appears, which other ADR this changes. If this decision amends or
replaces another ADR, say so here AND add a back-reference line at the top of
the other ADR, under its Status line:
  Amended by ADR-NNNN (<one-line summary>).
-->

## Considered options

<!--
Optional. Only rejected options whose rejection is not obvious, each with the
reason in one or two sentences. This stops someone from proposing the same
option again in six months.
-->

## Compliance

<!--
REQUIRED when Kind is structural. Optional for other kinds.

One line for each rule in Decision that code can break. Each line is exactly
one of these two forms:

  - D1: checked by `crates/xtask/tests/<file>.rs` (<test name>).
  - D2: not checked by code: <why, e.g. "needs a live vendor CLI"; or
        "manual: reviewed in the PR">.

"Not checked by code" is allowed. It makes the gap visible instead of hiding
it. Do not list behavior tests here: a test that pins what a feature does is
not a check that the boundary holds. A check here fails when *any* code,
including code that does not exist yet, breaks the rule.

If the rule is already broken by existing code, say so and name the ratchet:
  - D3: checked by `crates/xtask/tests/<file>.rs` as a ratchet: baseline N
        known violations (<where>); the check fails if the count goes up.
-->

- D1: <checked by … | not checked by code: …>

<!--
AMENDMENTS

Do not rewrite an accepted decision in place. Add a section at the end:

  ## Amendment YYYY-MM-DD — <what changed>

and state which rule (D-number) it changes. Update the Status line if needed.
If the amendment adds or changes a rule that code can break, add its line to
Compliance.
-->
