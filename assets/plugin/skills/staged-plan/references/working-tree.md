# Working-tree policy details

The four states declared in `## Execution policy`:

- **`clean-required`** — `git status` must be empty. Default when tree is clean. Subagents may commit freely.
- **`stash-authorized`** — recommended when there are uncommitted changes **unrelated** to this track. Stage 0 stashes them; the final summary reminds the user to `git stash pop`.
- **`integrate-existing`** — recommended when current uncommitted changes **are part of this work** (e.g., user started something then asked for a staged plan to finish it). Stage 0 records them in the report. Subagents must stage only files THEY modify; existing dirty files are folded into the natural stage that owns them.
- **`abort-until-clean`** — when state is ambiguous and the user wants to resolve manually before any plan runs.
