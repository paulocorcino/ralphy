# Browser checks

Playwright scripts that check the workbench in a real browser, against a
daemon with a scratch store (`RALPHY_DAEMON_DIR`). CI does not run them. The
rules are in [docs/TESTING.md](../../docs/TESTING.md) → *Browser checks*.

Run one script from the repo root:

```sh
python tests/browser/changes/wb_changes_315.py   # exit 0 = all pass
```

Each script builds the binaries it needs. `RALPHY_TEST_SKIP_BUILD=1` skips the
build. Screenshots go to `.ralphy/screenshots/`, which git ignores.

## One folder per workbench area

A folder holds the scripts for one part of the workbench. Its name matches the
UI module in `crates/ralphy-daemon/assets/ui/` where one exists.

| Folder | What it checks | UI module |
|---|---|---|
| `acceptance/` | operator walkthroughs that cover many areas in one pass | — |
| `agents/` | the console menu's agent roster, the agent state dot | `wb-agents.js` |
| `board/` | the issue board, the issue drawer, labels, a ready plan | `wb-kanban.js` |
| `changes/` | Changes, the diff, sync, push | `wb-changes.js` |
| `columns/` | consoles as columns and rows | `wb-columns.js` |
| `console/` | one console: clipboard, paste, names, touch, sessions | `wb-console.js` |
| `desk/` | the desk: windows, plane, pan, frame, panes, scrollbars | `wb-view.js`, `wb-split.js` |
| `fence/` | fences and their detached window | `wb-detach-link.js` |
| `files/` | the file tree, search, viewers, Monaco, encoding | `wb-viewer.js`, `wb-monaco.js` |
| `fleet/` | the federated sidebar and fleet surfaces | `wb-fleet.js` |
| `hosts/` | hosts and peer tunnels | `wb-hosts.js` |
| `notes/` | notes on the stage | `wb-notes.js` |
| `projects/` | the Projects list | `wb-project.js` |
| `release/` | the release badge and What's new | `wb-release.js` |
| `runs/` | the Runs panel | `wb-runs.js` |
| `security/` | security headers and the CSP | — |
| `spend/` | the Spend tab | `wb-spend.js` |
| `workbench/` | rules for the whole page: shown facts, icons, modals, cost | `app.js` |
| `worktree/` | worktrees and the checkout switcher | — |

A script that imports another script's helpers puts that script's folder on
`sys.path`. The `notes/` scripts use `columns/wb_columns_473.py`, and
`fence/wb_fence_redetach_476.py` uses `notes/wb_note_detach_draft.py`.
