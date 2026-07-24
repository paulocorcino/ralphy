<!-- slot: execution-model -->
   ## Execution model: low | medium | high | xhigh
   <one line justifying the choice. This is a vendor-neutral COMPLEXITY tier and
   ONE rung on a single cost/power ladder — it selects both the executor model
   and how hard it reasons: `low` → the fast model at low effort (mechanical,
   localized, well-understood changes: add a string, a field, a UI binding, a
   straightforward refactor); `medium` → the everyday model at medium effort,
   the default for ordinary feature work; `high` → the flagship at medium effort,
   for genuinely complex work (cross-cutting changes, tricky
   concurrency/lifetimes/type-plumbing, subtle correctness, or ambiguous design
   needing judgment); `xhigh` → the flagship at high effort, reserved for the
   hardest cases where `high` would visibly under-think. Pick the SMALLEST that
   will do this reliably; default to `medium` unless a concrete reason makes
   another rung the right call.>

<!-- slot: self-review-step -->
   - [ ] Self-review: delegate the review to one Codex subagent running the
         auto-discovered `reviewer` skill (`.agents/skills/reviewer/SKILL.md`),
         scoped to ONLY the commits you made for this issue — not the whole
         branch. Wait for the review result and adjudicate every finding; for a
         small mechanical diff, write this step as a direct adversarial re-read
         of the diff instead (see the self-review rule below). Resolve every
         HIGH finding before finishing; if one cannot be fixed autonomously,
         record it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a Codex-native self-review over this issue's
  commits — include it by DEFAULT, but SCALE it to the expected diff:
  - changes with real domain logic or a multi-file/multi-crate surface get the
    full review: delegate to one independent Codex subagent running
    `.agents/skills/reviewer/`, scoped to ONLY this issue's commits, then WAIT
    for its result and incorporate the findings before any closing paperwork —
    "background" is an interface property, not a contract in headless runs;
  - small mechanical changes (single crate/package, no new control flow,
    follow-a-pattern edits) get a lighter step: a direct adversarial re-read
    of the final diff by the executor itself, hunting for what tests can't
    catch — still recorded under `## Self-review findings`. A fixed multi-minute
    subagent review on a 50-line mechanical diff is cost without information.
  Omit the step entirely only when the change carries no domain logic at all
  (pure data/fixtures/docs), and record that omission as a `## Decisions`
  bullet with a one-line why. Either variant buys a real review: the executor
  must record the findings in the plan, so do not include it as ritual.
  Resolve every HIGH finding before declaring done.
- The LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
