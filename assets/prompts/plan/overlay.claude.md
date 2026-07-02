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
         branch). Resolve every HIGH finding before finishing; if one cannot be
         fixed autonomously, record it under `## Notes & decisions` and block
         instead of declaring done.
<!-- slot: self-review-guidance -->
- The penultimate step is an independent `reviewer`-skill self-review
  (spawned as a subagent) over this issue's commits — include it by DEFAULT.
  Omit it only when the change carries no domain logic at all (pure
  data/fixtures/docs), and record that omission as a `## Decisions` bullet
  with a one-line why. A plan that includes the step buys a real review: the
  executor must record the reviewer's findings in the plan, so do not include
  it as ritual for changes where it cannot find anything tests don't. The
  LAST step is always a green-build/test gate.
<!-- slot: ledger-example -->
- [verified] cargo test passes with new test covering parse_ledger — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
<!-- slot: planning-mode-intro -->
<!-- slot: skill-invocation -->
<!-- slot: stages-section -->
<!-- slot: mode-rules -->
