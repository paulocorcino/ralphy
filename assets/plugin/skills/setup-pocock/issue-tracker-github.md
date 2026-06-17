# Issue tracker: GitHub

Issues and PRDs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`, filtering comments by `jq` and also fetching labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`

Infer the repo from `git remote -v` — `gh` does this automatically when run inside a clone.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## PRD / milestone model (optional)

Only relevant if this repo adopted the PRD/roadmap track model (`docs/prd/`, `docs/roadmap.md`). If not, ignore this section.

Each PRD maps to exactly one GitHub **Milestone** that groups its issues.

- **Create the milestone when the PRD is created:**
  ```sh
  gh api repos/{owner}/{repo}/milestones --method POST \
    -f title="PRD-NNNN: Title" \
    -f description="docs/prd/NNNN-<trackid>-<slug>.md"
  ```
- **Attach every issue at creation:** `gh issue create ... --milestone "PRD-NNNN: Title"`
- **Each issue body also includes `Part of PRD-NNNN`** as plain text — text-search fallback: `gh issue list --search "Part of PRD-NNNN"`.
- **Check progress:** `gh api repos/{owner}/{repo}/milestones --jq '.[] | select(.title|startswith("PRD-NNNN")) | {open:.open_issues, closed:.closed_issues}'`
- **Close when DoD met:** `gh api repos/{owner}/{repo}/milestones/<number> --method PATCH -f state=closed`, then set PRD `status: done` and update `docs/prd/README.md` + `docs/roadmap.md`.
