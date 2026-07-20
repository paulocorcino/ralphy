<!-- slot: execution-model -->
<!-- slot: self-review-step -->
   - [ ] Self-review: a **direct adversarial re-read** of the final diff by the
         executor itself, scoped to ONLY the commits you made for this issue —
         not the whole branch. Read the diff hunting for what the tests cannot
         catch (a wrong branch taken silently, an off-by-one, a discarded error,
         a widened public surface), and record the findings under
         `## Self-review findings`. Resolve every HIGH finding before finishing;
         if one cannot be fixed autonomously, record it under `## Notes &
         decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review over ONLY the commits you made for
  this issue — include it by DEFAULT. Write it as a direct adversarial re-read
  of the final diff by the executor itself, hunting for what tests can't catch,
  with the findings recorded under `## Self-review findings`. Scale the depth to
  the diff: a multi-file/multi-crate change with real domain logic earns a
  hunk-by-hunk pass; a small mechanical change (single crate/package, no new
  control flow, follow-a-pattern edits) earns a single focused pass.
  Omit the step entirely only when the change carries no domain logic at all
  (pure data/fixtures/docs), and record that omission as a `## Decisions`
  bullet with a one-line why. The step buys a real review: the executor must
  record the findings in the plan, so do not include it as ritual.
  Resolve every HIGH finding before declaring done.
- The LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
