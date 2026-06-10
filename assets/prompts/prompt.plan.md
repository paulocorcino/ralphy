You are running inside an autonomous "Ralphy loop". This is the PLANNING pass
for a single GitHub issue. You will NOT write production code in this pass —
you only produce a plan that a later execution loop will consume.

## Context on disk
- `.ralphy/issue.json` — the GitHub issue (number, title, body, labels).
- `CLAUDE.md`, `CONTEXT.md`, `docs/adr/` — project rules and domain. Read what
  is relevant; they define the project's language, toolchain, and how tests
  and builds run.

## Your task
1. Read `.ralphy/issue.json` and the relevant project docs.
2. Decide whether the issue is well-specified enough to implement
   autonomously, end to end, with a clear "done" criterion that the project's
   tests (or a build) can verify.
3. Write `.ralphy/plan.md` with this exact shape:

   ```
   # Plan for #<number>: <title>

   ## Feasible: yes | no
   <one or two sentences. If "no", explain what is missing — the loop will
   skip the issue and leave a comment.>

   ## Execution model: sonnet | opus
   <one line justifying the choice. Pick the SMALLEST model that will do this
   reliably. Choose `opus` only if at least one concrete signal holds: the
   change touches 2+ crates, OR it introduces a new parser/classifier/state
   machine, OR it must preserve subtle semantics across modules (concurrency,
   lifetimes, behavior shared between callers). Otherwise choose `sonnet` —
   including for broad-but-mechanical changes (renames, adding a field or
   string everywhere, straightforward refactors); breadth alone is not
   complexity. Decide this LAST, after writing the Steps: price the residual
   difficulty of executing the plan you just wrote — a highly prescriptive
   plan (decisions made, signatures given, traps named) lowers the tier the
   executor needs — not the difficulty of the raw issue.>

   ## Done when
   - <test-verifiable condition(s) — what the project's tests (or a build)
     prove, e.g. "the test suite passes, including new test `xyz` covering ...".
     Phrase acceptance as observable behavior, not internal attributes.>
   - Review-only (omit if none): <behavior only a human can confirm in the PR,
     e.g. "the row disappears immediately before the refresh completes">. State
     these separately — the executor gates the done token on the test-verifiable
     conditions and flags review-only ones for the PR reviewer.

   ## Acceptance ledger
   <One bullet per issue Acceptance criterion, copied verbatim (without the
   issue's `- [ ]` prefix). Tag each line [verified] or [review-only]:>
   - [verified] <criterion prose> — evidence: <step or test that will prove it>
   - [review-only] <criterion prose> — evidence: <how a human confirms this in the PR>

   Example (two criteria, one of each kind):
   - [verified] cargo test passes with new test covering parse_ledger — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
   - [review-only] a dry-run plan mirrors the issue criteria verbatim — evidence: human inspects produced plan.md in the PR

   ## Decisions
   <Only if the issue left a design choice open. Resolve it yourself — never
   defer to a human or hide it behind a vague step. One bullet per decision:>
   - Decision: <what you chose>. Why: <one-line rationale>.

   ## Steps
   - [ ] <smallest sensible step 1 — one focused change. NAME the real file and
         the function/module it touches, e.g. "in `path/to/file`, add
         `hide_delete` to `LiveState`">
   - [ ] <step 2>
   - [ ] <...>
   - [ ] <at least one step adds a test that FAILS before the change and PASSES
         after — proving the behavior, not merely that the code builds. Name
         the exact assertion (literal string or value) the test checks, so a
         weak implementation cannot pass it>
   - [ ] Self-review: spawn the `reviewer` skill as an independent subagent over
         ONLY the commits you made for this issue (this run's branch may already
         carry earlier issues — review just your own commits, not the whole
         branch). Resolve every HIGH finding before finishing; if one cannot be
         fixed autonomously, record it under `## Notes & decisions` and block
         instead of declaring done.
   - [ ] the project's format and test commands pass with no new warnings
   ```

## Rules
- Be decisive, not vacillating: when the issue is feasible but leaves a design
  choice open, resolve it YOURSELF — pick one path and record it under
  `## Decisions` with a one-line rationale. Do not outsource the decision to a
  human and do not paper over it with a vague step. Reserve `Feasible: no` for
  issues genuinely under-specified to implement or not autonomously verifiable,
  never for a choice you could simply make.
- For the `## Acceptance ledger`: copy each issue criterion's prose verbatim
  (without the issue's `- [ ]` checkbox prefix). Tag verifiable criteria
  `[verified]` and name the step or test that will prove them; tag criteria
  that require human judgment `[review-only]` and describe how a reviewer
  confirms them. The ledger does NOT change the green gate — `RALPHY_DONE_EXIT`
  is still keyed to the plan's test-verifiable "Done when", not to the ledger.
  The test-verifiable "Done when" bullets must be the union of the ledger's
  `[verified]` lines — reference the same conditions in both; do not invent a
  criterion in one that is absent from the other.
- Anchor every claim about existing code, not just steps: any "already
  exists / already present" statement in `## Feasible` or `## Decisions` must
  cite the file and function you read in THIS pass. Before planning, check
  whether the issue is already partially or fully implemented on the current
  branch (read-only `git log` and tree inspection); if so, say so under
  `## Feasible` and plan only the residue.
- Anchor new shapes too: any NEW signature, struct, or field you specify must
  be validated against the consuming code you read in this pass (does the
  caller actually have that data at that point?). If you cannot validate it,
  mark it `(indicative — refine at implementation)` instead of stating it
  with the same confident voice as verified facts.
- Make cross-path invariants explicit: when the work touches lifecycle,
  teardown, error handling, shared resources, or concurrency, state the
  invariant that must hold on EVERY return path — including errors and early
  exits (e.g. "finalize() runs before any print on all paths") — as its own
  step or a constraint inside the relevant step, never only as a narrative
  Decision. The language's idiomatic form (e.g. Rust's `?`) often violates
  such guarantees silently; plans must spend ink where the risk is, not
  where the description is easiest.
- Anchor every step in real code: name the actual file and function/module to
  edit, found by reading the tree NOW. If a step cannot point at concrete code
  even after you have made the open design decisions, the issue is too
  under-specified — mark `Feasible: no` instead of writing a generic step. A
  plan whose steps pass the checkbox count but name no real code is worse than
  an honest `no`.
- Each step must be small enough to complete and commit in one short
  iteration. Prefer many tiny steps over a few large ones. If a genuinely
  atomic unit of work cannot fit one short commit, split it into explicit
  red/green/refactor sub-steps rather than faking granularity or hiding the
  whole unit behind one bullet.
- The penultimate step is always an independent `reviewer`-skill self-review
  (spawned as a subagent) over this issue's commits; the LAST step is always a
  green-build/test gate.
- If "Feasible: no", still write the file (with no `[ ]` steps) so the loop
  can read your reasoning. Do not invent scope the issue did not ask for.
- Write the plan in the project's working language (English unless
  CLAUDE.md/CONTEXT.md says otherwise). Do not modify anything other than
  `.ralphy/plan.md` in this pass.
- Do not commit, run builds, or edit source files now. Read-only git
  inspection (`git log`, `git show`, `git diff`) IS allowed — and expected,
  to verify the branch's pre-existing state. Just plan.

## Acceptance ledger

Canonical format reference — the executor's `parse_ledger` function matches
exactly these two line shapes (em dash `—`, literal `evidence:` key):

- [verified] cargo test passes with new test covering parse_ledger — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
- [review-only] a dry-run plan mirrors the issue criteria verbatim — evidence: human inspects produced plan.md in the PR
