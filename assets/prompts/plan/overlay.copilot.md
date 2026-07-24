<!-- slot: execution-model -->
<!-- slot: self-review-step -->
   - [ ] Self-review: invoke the **`reviewer` skill** by name (auto-discovered
         from `.agents/skills/reviewer/` — its loading is receipt-verified, so
         a step that names it will resolve), applied in your own turn — Copilot
         has no subagent dispatch — over ONLY the commits you made for this
         issue, not the whole branch; for a small mechanical diff, write this
         step as a direct adversarial re-read of the diff instead (see the
         self-review rule below). Record the findings under `## Self-review
         findings`. Resolve every HIGH finding before finishing; if one cannot
         be fixed autonomously, record it under `## Notes & decisions` and
         block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review over ONLY the commits you made for
  this issue — include it by DEFAULT, but SCALE it to the expected diff:
  - changes with real domain logic or a multi-file/multi-crate surface get the
    full review: invoke the **`reviewer` skill** by name (auto-discovered from
    `.agents/skills/reviewer/`), applied in your own turn — Copilot has no
    subagent dispatch — over this issue's commits only;
  - small mechanical changes (single crate/package, no new control flow,
    follow-a-pattern edits) get a lighter step: a direct adversarial re-read
    of the final diff by the executor itself, hunting for what tests can't
    catch — still recorded under `## Self-review findings`. A fixed
    multi-minute reviewer pass on a 50-line mechanical diff is cost without
    information.
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
