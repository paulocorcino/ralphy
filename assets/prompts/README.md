# Agent charters

The files in this folder are the instructions Ralphy gives the agent on
**every project it works on**, not on this repo only. The adapters compile
them into the binary with `include_str!`. A sentence changed here changes what
the agent does in every user's repo.

## Before you change a charter

- **Ask whether the rule helps a typical project.** Most target repos have
  none of this repo's documents (`docs/ARCHITECTURE.md`, its fact index,
  `CONTEXT.md`, ADRs). A rule that serves only Ralphy's own codebase goes in
  this repo's [AGENTS.md](../../AGENTS.md), which the agent loads when it
  works here.
- **Start from a real run.** A change needs a case seen in a run: a plan or a
  handoff where the charter led the agent wrong. A rule added "to be safe"
  makes every plan longer and gives the agent more text to weigh.
- **Change the least text you can.** These charters work today. Prefer
  sharpening an existing rule to adding a new one, and keep existing rules
  unchanged unless the run shows they are wrong.

## Where each file comes from

- `prompt.plan*.md` are generated from `plan/`: edit `plan/template.md` or an
  overlay, then regenerate. See [plan/README.md](plan/README.md).
- `prompt.execute.md`, `prompt.triage.md`, `prompt.diagnose.md`,
  `prompt.consolidate.md` and `prompt.init-issues.md` are edited directly.

Every change here is flagged by the capabilities check (`xtask`) as agent
instructions, because the agent that reads it has shell access on the user's machine.
