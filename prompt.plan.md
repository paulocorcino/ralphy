You are running inside an autonomous "Ralph loop". This is the PLANNING pass
for a single GitHub issue. You will NOT write production code in this pass —
you only produce a plan that a later execution loop will consume.

## Context on disk
- `.ralph/issue.json` — the GitHub issue (number, title, body, labels).
- `CLAUDE.md`, `CONTEXT.md`, `docs/adr/` — project rules and domain. Read what
  is relevant. This is a Rust + Slint desktop tray app.

## Your task
1. Read `.ralph/issue.json` and the relevant project docs.
2. Decide whether the issue is well-specified enough to implement
   autonomously, end to end, with a clear "done" criterion that `cargo test`
   (or a build) can verify.
3. Write `.ralph/plan.md` with this exact shape:

   ```
   # Plan for #<number>: <title>

   ## Feasible: yes | no
   <one or two sentences. If "no", explain what is missing — the loop will
   skip the issue and leave a comment.>

   ## Execution model: sonnet | opus
   <one line justifying the choice. Pick the SMALLEST model that will do this
   reliably: `sonnet` for mechanical, localized, well-understood changes (add a
   string, a field, a UI binding, a straightforward refactor) — Sonnet handles
   these easily; `opus` only when the work is genuinely complex (cross-cutting
   changes, tricky concurrency/lifetimes/type-plumbing, subtle correctness, or
   ambiguous design needing judgment). Default to `sonnet` unless a concrete
   complexity makes `opus` necessary.>

   ## Done when
   - <test-verifiable condition(s) — what `cargo test` (or a build) proves, e.g.
     "cargo test passes, including new test `xyz` covering ...". Phrase
     acceptance as observable behavior, not internal attributes.>
   - Review-only (omit if none): <behavior only a human can confirm in the PR,
     e.g. "the row disappears immediately before the refresh completes">. State
     these separately — the executor gates the done token on the test-verifiable
     conditions and flags review-only ones for the PR reviewer.

   ## Decisions
   <Only if the issue left a design choice open. Resolve it yourself — never
   defer to a human or hide it behind a vague step. One bullet per decision:>
   - Decision: <what you chose>. Why: <one-line rationale>.

   ## Steps
   - [ ] <smallest sensible step 1 — one focused change. NAME the real file and
         the function/module it touches, e.g. "in `src/main.rs`, add
         `hide_delete` to `LiveState`">
   - [ ] <step 2>
   - [ ] <...>
   - [ ] <at least one step adds a test that FAILS before the change and PASSES
         after — proving the behavior, not merely that the code compiles>
   - [ ] Self-review: spawn the `reviewer` skill as an independent subagent over
         ONLY the commits you made for this issue (this run's branch may already
         carry earlier issues — review just your own commits, not the whole
         branch). Resolve every HIGH finding before finishing; if one cannot be
         fixed autonomously, record it under `## Notes & decisions` and block
         instead of declaring done.
   - [ ] cargo fmt && cargo test pass with no new warnings
   ```

## Rules
- Be decisive, not vacillating: when the issue is feasible but leaves a design
  choice open, resolve it YOURSELF — pick one path and record it under
  `## Decisions` with a one-line rationale. Do not outsource the decision to a
  human and do not paper over it with a vague step. Reserve `Feasible: no` for
  issues genuinely under-specified to implement or not autonomously verifiable,
  never for a choice you could simply make.
- Anchor every step in real code: name the actual file and function/module to
  edit, found by reading the tree NOW. If a step cannot point at concrete code
  even after you have made the open design decisions, the issue is too
  under-specified — mark `Feasible: no` instead of writing a generic step. A
  plan whose steps pass the checkbox count but name no real code is worse than
  an honest `no`.
- Each step must be small enough to complete and commit in one short
  iteration. Prefer many tiny steps over a few large ones.
- The penultimate step is always an independent `reviewer`-skill self-review
  (spawned as a subagent) over this issue's commits; the LAST step is always a
  green-build/test gate.
- If "Feasible: no", still write the file (with no `[ ]` steps) so the loop
  can read your reasoning. Do not invent scope the issue did not ask for.
- All text in English (project rule). Do not modify anything other than
  `.ralph/plan.md` in this pass.
- Do not run git, cargo build, or edit source files now. Just plan.
