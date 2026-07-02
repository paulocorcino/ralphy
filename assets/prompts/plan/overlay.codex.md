<!-- slot: execution-model -->
   ## Execution model: low | medium | high
   <one line justifying the choice. This is a vendor-neutral COMPLEXITY tier
   that maps to the executor's reasoning effort. Pick the SMALLEST that will do
   this reliably: `low` for mechanical, localized, well-understood changes (add
   a string, a field, a UI binding, a straightforward refactor); `medium` is the
   default for ordinary feature work; `high` only when the work is genuinely
   complex (cross-cutting changes, tricky concurrency/lifetimes/type-plumbing,
   subtle correctness, or ambiguous design needing judgment). Default to
   `medium` unless a concrete reason makes `low` or `high` the right call.>

<!-- slot: self-review-step -->
   - [ ] Self-review: dispatch the auto-discovered `reviewer` skill
         (`.agents/skills/reviewer/SKILL.md`) as a Codex subagent scoped to
         ONLY the commits you made for this issue — not the whole branch.
         Resolve every HIGH finding before finishing; if one cannot be fixed
         autonomously, record it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a Codex-native self-review: dispatch
  `.agents/skills/reviewer/` as a Codex subagent scoped to ONLY the commits you
  made for this issue — include it by DEFAULT. Omit it only when the change
  carries no domain logic at all (pure data/fixtures/docs), and record that
  omission as a `## Decisions` bullet with a one-line why. A plan that
  includes the step buys a real review: the executor must record the
  reviewer's findings in the plan, so do not include it as ritual for changes
  where it cannot find anything tests don't. Resolve every HIGH finding
  before declaring done. Phrase it as a Codex subagent dispatch — not as a
  Claude Task-tool invocation.
- The LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
