<!-- slot: execution-model -->
<!-- slot: self-review-step -->
   - [ ] Self-review: run the **inline `reviewer` skill** (auto-discovered via
         `skills.paths`), invoked by name over ONLY the commits you made for
         this issue — **not** a subagent, and not the whole branch. Resolve
         every HIGH finding before finishing; if one cannot be fixed
         autonomously, record it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review: run the **inline `reviewer`
  skill** (auto-discovered via `skills.paths`) over ONLY the commits you made
  for this issue — **not** a subagent, and not the whole branch. Include it
  by DEFAULT; omit it only when the change carries no domain logic at all
  (pure data/fixtures/docs), and record that omission as a `## Decisions`
  bullet with a one-line why. A plan that includes the step buys a real
  review: the executor must record the reviewer's findings in the plan, so
  do not include it as ritual for changes where it cannot find anything
  tests don't. Resolve every HIGH finding before declaring done.
- The LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
