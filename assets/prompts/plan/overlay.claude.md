<!-- slot: execution-model -->
   ## Execution model: sonnet | opus
   <one line justifying the choice. Pick the SMALLEST model that will do this
   reliably. Choose `opus` only if at least one concrete signal holds: the
   change touches 2+ crates/packages, OR it introduces a new parser/classifier/state
   machine, OR it must preserve subtle semantics across modules (concurrency,
   lifetimes, behavior shared between callers). Otherwise choose `sonnet` —
   including for broad-but-mechanical changes (renames, adding a field or
   string everywhere, straightforward refactors); breadth alone is not
   complexity. Decide this LAST, after writing the Steps: price the residual
   difficulty of executing the plan you just wrote — a highly prescriptive
   plan (decisions made, signatures given, traps named) lowers the tier the
   executor needs — not the difficulty of the raw issue.>

<!-- slot: self-review-step -->
   - [ ] Self-review: spawn the `reviewer` skill as an independent subagent over
         ONLY the commits you made for this issue (this run's branch may already
         carry earlier issues — review just your own commits, not the whole
         branch); for a small mechanical diff, write this step as a direct
         adversarial re-read of the diff instead (see the self-review rule
         below). Resolve every HIGH finding before finishing; if one cannot be
         fixed autonomously, record it under `## Notes & decisions` and block
         instead of declaring done.
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
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
