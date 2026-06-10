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
   - [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
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
         after — proving the behavior, not merely that the code builds>
   - [ ] Self-review: dispatch the auto-discovered `reviewer` skill scoped to
         ONLY the commits you made for this issue — not the whole branch.
         Resolve every HIGH finding before finishing; if one cannot be fixed
         autonomously, record it under `## Notes & decisions` and block.
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
- Anchor every step in real code: name the actual file and function/module to
  edit, found by reading the tree NOW. If a step cannot point at concrete code
  even after you have made the open design decisions, the issue is too
  under-specified — mark `Feasible: no` instead of writing a generic step. A
  plan whose steps pass the checkbox count but name no real code is worse than
  an honest `no`.
- Each step must be small enough to complete and commit in one short
  iteration. Prefer many tiny steps over a few large ones.
- The penultimate step is always a self-review: dispatch the auto-discovered
  `reviewer` skill scoped to ONLY the commits you made for this issue — not the
  whole branch. Resolve every HIGH finding before declaring done.
- The LAST step is always a green-build/test gate.
- If "Feasible: no", still write the file (with no `[ ]` steps) so the loop
  can read your reasoning. Do not invent scope the issue did not ask for.
- Write the plan in the project's working language (English unless
  CLAUDE.md/CONTEXT.md says otherwise). Do not modify anything other than
  `.ralphy/plan.md` in this pass.
- Do not run git, builds, or edit source files now. Just plan.

## Acceptance ledger

Canonical format reference — the executor's `parse_ledger` function matches
exactly these two line shapes (em dash `—`, literal `evidence:` key):

- [verified] the test suite passes with a new test covering the ledger parser — evidence: a new test feeds the prompt example through the parser and asserts typed verdicts
- [review-only] a dry-run plan mirrors the issue criteria verbatim — evidence: human inspects produced plan.md in the PR
