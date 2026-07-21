<!-- slot: execution-model -->
   ## Execution model: one piped turn
   <one line. The executor receives the execution charter on standard input as a
   SINGLE turn — there is no resume-with-more-instructions idiom in Ralphy's use
   of this vendor, and no model tier to pick. So price the plan for a session
   that reads nothing but `.ralphy/plan.md`, the issue and the repository:
   state the signatures, the literal assertions and the traps inline rather than
   leaving them to be asked about.>

<!-- slot: self-review-step -->
<!-- slot: self-review-guidance -->
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
