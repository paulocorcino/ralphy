# Issue tracker: Local Markdown

Issues and PRDs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The PRD is `.scratch/<feature-slug>/PRD.md`
- Implementation issues are `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`
- Triage state is recorded as a `Status:` line near the top of each issue file (see `triage-labels.md` for the role strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.

## PRD / roadmap model (optional)

Only relevant if this repo adopted the PRD/roadmap track model (`docs/prd/`, `docs/roadmap.md`). If not, ignore this section.

There is no separate "milestone" object — the **`.scratch/<feature-slug>/` directory is the grouping**:

- The PRD lives at `.scratch/<feature-slug>/PRD.md` (the `/to-prd` output). Optionally keep a copy under `docs/prd/` if you want the cross-linked `docs/prd/README.md` index.
- Each issue file under `.scratch/<feature-slug>/issues/` carries `Part of PRD-NNNN` near the top.
- The PRD is `done` when every issue file's `Status:` line is closed and the Definition of Done is met; then update `docs/roadmap.md` (and `docs/prd/README.md` if used).
