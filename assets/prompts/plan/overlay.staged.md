<!-- slot: execution-model -->
   ## Execution model: sonnet | opus
   <smallest model that will do this reliably — `sonnet` for mechanical/localized
   work, `opus` for genuinely complex. Staged issues tend to be complex, so this
   is often `opus`, but judge honestly.>

<!-- slot: self-review-step -->
   - [ ] Self-review: spawn the `reviewer` skill as an independent subagent over
         ONLY the commits made for this issue — not the whole branch; for a
         small mechanical diff, write this step as a direct adversarial re-read
         of the diff instead (see the self-review rule below). Resolve
         every HIGH finding before finishing; if one cannot be fixed
         autonomously, record it under `## Notes & decisions` and block.
<!-- slot: self-review-guidance -->
- The penultimate step is a self-review over this issue's commits — include
  it by DEFAULT, but SCALE it to the expected diff:
  - changes with real domain logic or a multi-file/multi-crate surface get the
    full independent review: spawn the `reviewer` skill as a subagent;
  - small mechanical changes (single crate/package, no new control flow,
    follow-a-pattern edits) get a lighter step: a direct adversarial re-read
    of the final diff by the executor itself, hunting for what tests can't
    catch — still recorded under `## Self-review findings`. A fixed 5–8-minute
    subagent review on a 50-line mechanical diff is cost without information.
  Omit the step entirely only when the change carries no domain logic at all
  (pure data/fixtures/docs), and record that omission as a `## Decisions`
  bullet with a one-line why. Either variant buys a real review: the executor
  must record the findings in the plan, so do not include it as ritual. The
  LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] cargo test passes with new test covering parse_ledger — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->

This issue is flagged for STAGED PLANNING (label `stagedplan`). Use the
**`staged-plan` skill** to design a thorough, multi-stage plan — but the final
artifact must still be `.ralphy/plan.md` in the exact shape the executor
expects (below).
<!-- slot: skill-invocation -->
   Then invoke the `staged-plan` skill to design the implementation plan
   before writing `.ralphy/plan.md`. It runs NON-INTERACTIVELY
   (`STAGED_PLAN_NONINTERACTIVE=1` is set): follow the skill's non-interactive
   branch — do NOT call `AskUserQuestion`, there is no human to answer. Let
   the skill do its deep, staged design work.
<!-- slot: stages-section -->
   ## Stages
   <short narrative of the stages from the staged-plan design — the "why" and
   ordering — so the executor has the design context.>

<!-- slot: mode-rules -->
- The authoritative artifact the executor reads is `.ralphy/plan.md`. If the
  skill also scaffolds a plan file elsewhere, fine — but `.ralphy/plan.md` MUST
  exist and hold the shape above.
- Keep the staged ordering as the sequence of `- [ ]` steps (one per stage or
  sub-step), so the executor implements them in order.
- This issue is already flagged for staged planning — multi-part scope is
  EXPECTED here. Do not apply the source-document bundle rule above (the
  `needs-split` verdict): design stages for the full scope instead of
  recommending a split.
