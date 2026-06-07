# Branch modes + preconditions + run wrap-up reporting

**Type:** AFK
**Triage:** needs-triage
**Spec:** [CONTEXT.md](../../CONTEXT.md)

## What to build

The remaining run-lifecycle parity around branches and reporting. Slice 0001 cut a
new run branch; this completes the branch-mode matrix, the preconditions, and the
end-of-run reporting. Parity oracle: the ps1 main/`finally` block.

Scope:
- `--branch-mode current`: commit straight onto the branch the repo is already on
  (no new branch, `--base-branch` ignored); refuse detached HEAD.
- Clean-tree precondition for both modes (ignoring `.ralphy/`).
- Base-branch validation; best-effort `git fetch origin` first.
- Auto-add `.ralphy/` to the target repo's `.gitignore` on first run.
- Wrap-up: commit count over the compare ref, oneline log, and the right closing
  state per mode/outcome (clean run → return to original branch, run branch kept;
  non-green stop → leave on run branch; dry-run new → drop empty branch).

## Acceptance criteria

- [ ] `--branch-mode current` commits onto the current branch with no new branch;
      detached HEAD is refused.
- [ ] A dirty working tree aborts before any branch work (`.ralphy/` ignored).
- [ ] `.ralphy/` is added to the target repo's `.gitignore` on first run.
- [ ] End-of-run leaves the repo in the correct branch state per mode and outcome,
      matching the ps1.

## Blocked by

- #5 (queue loop)
