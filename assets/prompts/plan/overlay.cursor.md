<!-- slot: execution-model -->
   ## Execution model: one piped turn
   <one line. The executor receives the execution charter on standard input as a
   SINGLE turn — there is no resume-with-more-instructions idiom in Ralphy's use
   of this vendor, and no model tier to pick. So price the plan for a session
   that reads nothing but `.ralphy/plan.md`, the issue and the repository:
   state the signatures, the literal assertions and the traps inline rather than
   leaving them to be asked about.>

<!-- slot: self-review-step -->
   - [ ] Self-review: run the **subagent `reviewer` skill**, or if it is not available the **inline `reviewer` skill** (materialized at
         `<repo>/.cursor/skills/reviewer/`, discovered BY NAME — name it exactly
         `reviewer`, it sits among dozens of unrelated harvested skills) over
         ONLY the commits you made for this issue — not the whole branch; for a
         small mechanical diff, write this step as a direct adversarial re-read
         of the diff instead (see the self-review rule below). Resolve every
         HIGH finding before finishing; if one cannot be fixed autonomously,
         record it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review over ONLY the commits you made for
  this issue — include it by DEFAULT, but SCALE it to the expected diff:
  - changes with real domain logic or a multi-file/multi-crate surface get the
    full review: run the **inline `reviewer` skill** (materialized at
    `<repo>/.cursor/skills/reviewer/`, discovered BY NAME among dozens of
    unrelated harvested skills — name it exactly `reviewer`), invoked over
    this issue's commits only;
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
   Skills live in `<repo>/.cursor/skills/` and are discovered BY NAME, alongside
   up to ~78 unrelated skills harvested from other vendors' skill directories on
   this machine. So a step that wants a skill must name it precisely — a vague
   "use the review skill" will not resolve against that crowd.
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
- Your own vendor's native plan mode is NOT in use: this pass runs in ordinary
  execution mode, and you MUST write `.ralphy/plan.md` yourself. Refusing to
  write the file because planning "should not make edits" fails the pass — the
  plan file IS the deliverable of this pass.
