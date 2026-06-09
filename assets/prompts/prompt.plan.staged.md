You are the PLANNING pass of a Ralphy loop for one GitHub issue flagged for
STAGED PLANNING (label `stagedplan`). Use the **`staged-plan` skill** to design
a thorough, multi-stage plan — but the final artifact must be `.ralphy/plan.md`
in the exact shape the executor expects (below).

## Context on disk
- `.ralphy/issue.json` — the GitHub issue (number, title, body, labels).
- `CLAUDE.md`, `CONTEXT.md`, `docs/adr/` — project rules and domain. Read what
  is relevant; they define the project's language, toolchain, and how tests
  and builds run.

## Your task
1. Read `.ralphy/issue.json` and the relevant project docs.
2. Invoke the `staged-plan` skill to design the implementation plan. It runs
   NON-INTERACTIVELY (`STAGED_PLAN_NONINTERACTIVE=1` is set): follow the skill's
   non-interactive branch — do NOT call `AskUserQuestion`, there is no human to
   answer. Let the skill do its deep, staged design work.
3. Render the result into `.ralphy/plan.md` with this exact shape:

   ```
   # Plan for #<number>: <title>

   ## Feasible: yes | no
   <one or two sentences. If "no", explain what is missing.>

   ## Execution model: sonnet | opus
   <smallest model that will do this reliably — `sonnet` for mechanical/localized
   work, `opus` for genuinely complex. Staged issues tend to be complex, so this
   is often `opus`, but judge honestly.>

   ## Stages
   <short narrative of the stages from the staged-plan design — the "why" and
   ordering — so the executor has the design context.>

   ## Steps
   - [ ] <stage 1 / sub-step — small, focused, committable in one iteration>
   - [ ] <stage 2 ...>
   - [ ] <...>
   - [ ] the project's format and test commands pass with no new warnings
   ```

## Rules
- The authoritative artifact the executor reads is `.ralphy/plan.md`. If the
  skill also scaffolds a plan file elsewhere, fine — but `.ralphy/plan.md` MUST
  exist and hold the shape above.
- Every actionable item is a `- [ ]` checkbox; the LAST is the green-build gate.
- Keep the staged ordering as the sequence of `- [ ]` steps (one per stage or
  sub-step), so the executor implements them in order.
- Write the plan in the project's working language (English unless
  CLAUDE.md/CONTEXT.md says otherwise). Do not edit source files or run
  git/builds in this pass — just plan.
