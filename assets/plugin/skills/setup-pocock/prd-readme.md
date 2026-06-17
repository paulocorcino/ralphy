# PRD Index

Each PRD captures the problem, requirements, Definition of Done, and issue breakdown for one roadmap track.

- **Roadmap** → points to PRDs (thin portfolio)
- **PRD** → 1 issue grouping + N issues
- **Issue** → belongs to the grouping, body contains `Part of PRD-NNNN`
- **Grouping 100% closed = PRD DoD verified = PRD `done` = roadmap track `done`**

The "grouping" is whatever your issue tracker uses: a GitHub/GitLab **Milestone**, or the `.scratch/<feature-slug>/` directory for the local-markdown tracker. See [docs/agents/issue-tracker.md](../agents/issue-tracker.md) for the exact commands.

## Index

| PRD | Title | Track | Status | Issues |
|-----|-------|-------|--------|--------|
| — | _pending_ | — | `draft` | — |

## Naming convention

```
NNNN-<trackid>-<slug>.md
```

- `NNNN` — sequential, zero-padded (share the space with ADRs if you keep them)
- `<trackid>` — short track identifier (`t0`, `t1`, …)
- `<slug>` — lowercase kebab-case description

## Workflow

1. `/to-prd` generates the PRD file from conversation context using `_template.md`.
2. Create the issue grouping for the PRD:
   - **GitHub:** a Milestone — `gh api repos/{owner}/{repo}/milestones --method POST -f title="PRD-NNNN: Title"`
   - **GitLab:** a Milestone — `glab api projects/:id/milestones --method POST -f title="PRD-NNNN: Title"`
   - **Local markdown:** none — the `.scratch/<feature-slug>/` directory *is* the grouping.
3. `/to-issues` reads the "Issue breakdown" section and creates the issues with `Part of PRD-NNNN` in the body, each attached to the grouping.
4. Fill in issue numbers in the PRD's issue breakdown table.
5. Execute issues (e.g. `/tdd`, then review).
6. When the grouping reaches 100% and DoD is met: close it, set PRD `status: done`, and update this file and `docs/roadmap.md`.
