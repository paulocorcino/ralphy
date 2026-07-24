<!-- slot: execution-model -->
   ## Execution model: one piped turn
   <one line. The executor receives the execution charter on standard input as a
   SINGLE turn — there is no resume-with-more-instructions idiom in Ralphy's use
   of this vendor, and no model tier to pick. So price the plan for a session
   that reads nothing but `.ralphy/plan.md`, the issue and the repository:
   state the signatures, the literal assertions and the traps inline rather than
   leaving them to be asked about.>

<!-- slot: self-review-step -->
   - [ ] Self-review: activate the `reviewer` skill IN THIS TURN — never as a
         subagent, since delegation to subagents is denied for the whole run —
         over ONLY the commits made for this issue, not the whole branch. For a
         small mechanical diff (single crate, no new control flow,
         follow-a-pattern edits), a direct adversarial re-read by the executor
         itself is the lighter variant instead of the full skill invocation.
         Record findings under `## Self-review findings`. Resolve every HIGH
         finding before finishing; if one cannot be fixed autonomously, record
         it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review over ONLY the commits you made for
  this issue — include it by DEFAULT. Activate the `reviewer` skill IN THIS
  TURN — never as a subagent, since delegation to subagents is denied for the
  whole run — hunting for what tests can't catch, with the findings recorded
  under `## Self-review findings`. Scale the depth to the diff: a
  multi-file/multi-crate change with real domain logic earns the skill's full
  pass; a small mechanical change (single crate/package, no new control flow,
  follow-a-pattern edits) earns a direct adversarial re-read by the executor
  itself instead.
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
   Ralphy's skills are materialized into the configuration root Ralphy owns for
   this run and are discovered BY NAME (`reviewer`, `setup-pocock`,
   `staged-plan`) — the operator's own `~/.gemini/skills` is never read, so a
   step that wants a skill must name it exactly. Delegation to subagents is
   denied for the whole run, so a named skill activates and runs inside the
   turn that names it, never handed off.
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
- Your own vendor's native plan mode is NOT in use: this pass runs in ordinary
  execution mode, and you MUST write `.ralphy/plan.md` yourself. That mode writes
  its plan into a vendor-private directory whatever it is instructed, so its
  output would never reach Ralphy. Refusing to write the file because planning
  "should not make edits" fails the pass — the plan file IS the deliverable of
  this pass.
- Delegation to subagents is denied by policy for the whole run. Do the work in
  this turn; a step that assumes a subagent will carry it is a step that never
  runs.
