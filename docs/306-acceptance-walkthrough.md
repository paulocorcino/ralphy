# #306 operator acceptance walkthrough — 2026-07-25

Daemon: real, one process, one browser process, Localhost policy on port 7396
over a scratch `RALPHY_DAEMON_DIR`. Produced by
`crates/ralphy-daemon/tests/wb_accept_306.py`, 72/72 checks passed,
`ALL SYMPTOMS NOT REPRODUCIBLE`. The same script, same commit, was run in the
pinned Linux container (command below) and reported the same
`ALL SYMPTOMS NOT REPRODUCIBLE` with exit 0.

The consolidated human-in-the-loop gate for PRD #296 (#300–#305). Nine scenarios
in ONE browser process against ONE daemon, each asserting external behaviour an
operator can see.

| # | Slice | Criterion it closes | Result | Screenshot |
|---|-------|---------------------|--------|------------|
| 1 | #305 | The canvas tab reads Consoles · `window.WBTranslate` undefined, `GET /wb-translate.js` 404 | PASS | none (tab-strip + absence asserts; the desk shot below shows the strip) |
| 1b | #305 | No translation affordance is present anywhere (the DOM half — run **after** scenario 6, see below) | PASS | none (selector counts behind two positive controls) |
| 2 | #303 | Console windows resize from an edge and from a corner, and a top or left edge resize holds the opposite edge | PASS | none (geometry asserted from `offsetLeft/Top/Width/Height`) |
| 3 | #304 | The console menu lists the daemon's roster and marks an agent that has a live session | PASS | `306-agent-menu-2026-07-25.png` |
| 4 | #301 | The board refreshes on an explicit control and on the document becoming visible, and does not refresh while hidden | PASS | none (call-count deltas on the stubbed fold) |
| 5 | #302 | The issue drawer shows a real body and a comment thread with author and date | PASS | none (drawer text asserted; see `302-issue-drawer-2026-07-25.png`) |
| 6 | #300 | The Runs panel shows a run started from a terminal, and advances live during it | PASS | `306-runs-live-2026-07-25.png` |
| 7 | #301 | A card for an issue being worked is visibly marked, and clicking through reaches the run | PASS | `306-board-run-2026-07-25.png` |
| 8 | #303 | After a daemon restart, the desk is restored: free consoles come back on their own, agent consoles come back as placeholders, all in their saved rectangles | PASS | `306-consoles-desk-2026-07-25.png` |
| 9 | — | Dated screenshots are written under the existing convention · no uncaught page error | PASS | (asserts the four files above exist non-empty AND were written by this run) |

## Run commands

Windows (the gate as run for this record):

```
python crates/ralphy-daemon/tests/wb_accept_306.py
```

Linux, in the pinned Playwright container against a Linux `ralphy` built in
`rust:1.97` (`MSYS_NO_PATHCONV=1` is mandatory in git-bash on Windows, or `/w`
is mangled into `W:/`):

```
MSYS_NO_PATHCONV=1 docker run --rm -v "C:/Dev/ralphy:/w" -w /w \
  -e CARGO_TARGET_DIR=/w/target/linux rust:1.97 \
  sh -c "cargo build -p ralphy-cli --bin ralphy && cargo build -p ralphy-daemon --bins"

MSYS_NO_PATHCONV=1 docker run --rm --shm-size=1g -v "C:/Dev/ralphy:/w" -w /w \
  -e RALPHY_WB_TARGET=/w/target/linux/debug \
  mcr.microsoft.com/playwright/python:v1.60.0-noble \
  sh -c "pip3 install --quiet --break-system-packages playwright==1.60.0 && \
         python3 crates/ralphy-daemon/tests/wb_accept_306.py"
```

That image ships the browsers (`/ms-playwright/chromium-1223`) but NOT the
`playwright` PyPI package, hence the `pip3 install` — it downloads a wheel, not
a browser. `git` is already present (2.43.0).

`RALPHY_WB_TARGET` overrides where `ralphy` and `session_test_child` are found
and skips `build()` — the browser container has Playwright but no cargo.

## Stub / real boundary

Real: the daemon process and its RESTART, `runs.list` / `runs.watch` and the
whole run channel, `tree.*`, `GET /api/agents`, `GET /api/sessions`, the
`/ws` control channel and the `/ws/session` PTY sockets, the desk store, every
client-side fold and policy under test.

Stubbed at `WBDaemon.observe`, which FALLS THROUGH to the real transport for
every other verb: `board.list`, `label.set`, `issue.show`. The board fold spawns
a CLI making tracker calls a throwaway fixture repo cannot answer. The
fall-through is load-bearing — scenario 6's live run pushes ride the same
`observe`. The refresh POLICY (scenario 4), the navigation (scenario 7) and the
drawer RENDER (scenario 5) are all client-side and untouched by the stub; the
folds themselves ride #198's and #302's own tests.

Agent launches resolve to `session_test_child` through
`RALPHY_DAEMON_AGENT_OVERRIDE`, so scenario 3 proves the real launch/attach path
with no vendor CLI installed and no quota spent.

## Detail

- **Scenario 1** — the tab strip renders exactly one tab, `.tab-title` reads
  exactly `Consoles`, and it shows no visible `.tab-close`; opening `README.md`
  appends a closable tab AFTER it while `tabs[0]` stays
  `{id: "consoles", closable: false}` and the file tab renders a VISIBLE
  `.tab-close`. Translation, runtime half: `window.WBTranslate` is `undefined`
  and `GET /wb-translate.js` returns 404.
- **Scenario 1b** — the DOM half of the translation sweep, deliberately
  DEFERRED until scenario 6 has put a run on disk. `.plan-xlate` lived inside
  `<template x-if="openSlug && projectRuns().length">` (`index.html:496`),
  which REMOVES the subtree — so a count of 0 taken at scenario 1, against the
  fixture's empty `.ralphy/runstate/`, would pass with the feature fully
  restored. Two positive controls run first (`.plan-wrap` count 1, so the plan
  pane really rendered; `[data-act="reload"]` present, so the markdown viewer
  really mounted — `openTab` mounts nothing when the `file.read` fails), and
  only then `.plan-xlate` / `[data-act="xlate"]` / `.md-xlate-note` each count
  0. The byte-level proof that the module is gone from the served bundle is a
  Rust unit test, `lib.rs::translation_is_gone_from_the_served_ui` (#305).
- **Scenario 2** — a free console is parked mid-workspace first (a window
  against the origin would clamp, and the drags would prove the clamp rather
  than the anchored edge). A `-120px` west drag widens it while
  `left + width` is unchanged and `top` is untouched; a `-90px` north drag
  grows it while `top + height` is unchanged; an `se` corner drag grows BOTH
  axes while `left` and `top` hold.
- **Scenario 3** — the menu's vendor labels equal, in order, `label || id` for
  every row `GET /api/agents` returns (7 roster rows + the plain-console row =
  8 on this build), compared against the endpoint rather than a hardcoded list
  so the assertion survives onboarding a vendor (ADR-0040). The roster itself
  is floored at 5 unique ids, so a collapsed roster cannot satisfy the relation
  trivially; the exact count is pinned in Rust by
  `roster.rs::accelerators_are_unique_and_stable`. Before any agent launch only the
  plain-console row is live (scenario 2's free console); after clicking the
  claude row, `GET /api/sessions` carries a `kind: "agent"` claude session and
  the badge map reads `{console: "1 live", claude: "1 live"}` — claude is the
  only vendor row marked.
- **Scenario 4** — stubbed `board.list` calls are counted. With the board
  CLOSED, `_boardLoadedAt` is back-dated past the backstop window and
  `boardBackstopTick()` is driven directly: 0 folds — the one automatic trigger
  there is, refused, rather than a trigger that merely never fired. Opening the
  board: 1. `.kanban-refresh`: +1. `document.visibilityState`
  stubbed `hidden` + a real `visibilitychange`: +0. `delete
  document.visibilityState` + `visibilitychange`: +1, attributed to the
  `visible` trigger (a count delta alone cannot tell it from a stray push).
  Headless chromium does not background a page when a sibling takes the
  foreground, so the STATE — the predicate's own input — is stubbed; the
  listener, the predicate and the wiring stay real (#301).
- **Scenario 5** — clicking card `#72` opens the drawer with the fixture body
  sentence in `.kd-body`, `.kd-comments-head` case-folded reading
  `2 comments` (the head is `text-transform: uppercase`), `octocat` in the
  first `.kd-comment-head`, and a `.kd-comment-at` that is non-empty AND not
  the raw ISO timestamp — never a formatted-date literal, since `fmtDate`
  follows the browser locale (#302).
- **Scenario 6** — the Runs panel is opened over an EMPTY `.ralphy/runstate/`,
  then a SEPARATE OS process (`subprocess.Popen([sys.executable, "-c", …])`)
  writes the snapshot document and, 3 s later, replaces it with the advanced
  one. With no operator action the panel goes 0 → 1 runs, `runPhaseLabel`
  advances `planning #72` → `executing #72` (both `wait_for_function`, never a
  sleep), and `.run-select-btn .run-prog` reads `1/3`. Issue 72 starts
  `planning`, not `pending`: `issueState` renders the ACTIVE non-terminal issue
  as `executing` for any other status, so a `pending` fixture would satisfy the
  advance oracle before the run advanced (#300).
- **Scenario 7** — with the Runs panel closed, exactly one `.kanban-card` is
  `.running`, it is `#72`, its `.kc-run` pill has a rendered WIDTH > 0 (not
  `offsetParent`: the card's `running` class and the pill's `x-show` evaluate
  the SAME expression, so an `offsetParent` read could not fail independently
  of the class asserted beside it)
  and its `.kc-run-txt` reads `executing · opencode` — the snapshot's
  `exec_agent`. Clicking the pill opens the Runs panel with `.trail-node.focus`
  carrying `data-issue="72"`, and `kanbanSel` stays `null` (`@click.stop`: the
  pill goes to the run, never also to the drawer).
- **Scenario 8** — nothing is maximized (a maximized window's
  `offsetLeft/Top` read 0/0 and the rect comparison would go blind, #303), both
  rects are snapshotted, the daemon is stopped by its own subprocess handle and
  relaunched on the same `RALPHY_DAEMON_DIR` and port. `GET /api/sessions` is
  `[]`. After a reload: one `.session-window:not(.placeholder)` with one
  `.xterm`, one `.session-window.placeholder` with zero `.xterm` and one
  `.session-reconnect` whose title names `claude` and the fixture slug, both
  rects identical to the snapshot, no `kind: "agent"` row in
  `GET /api/sessions`, and exactly ONE `/ws/session` socket opened — carrying
  `console=1`, not `agent=` (`/ws` is the control channel and is filtered out,
  #303). The last two are negative, point-in-time reads, so they are taken only
  after every positive assertion above has landed plus one further settle
  window — a late vendor spawn would otherwise be invisible.
- **Scenario 9** — zero `pageerror` events over the whole pass, and each of the
  four screenshots exists with size > 0 AND an mtime from this run, so a
  previous run's committed bytes cannot stand in for one this run never took.

## Review-only (human judgment)

- The operator's own eye over the four screenshots (layout, clipping,
  look-and-feel) — every behavioural criterion above is machine-asserted; only
  the visual impression is not.

## Known gaps

- `empty_env()` blanks the claude / codex / opencode / kimi vendor stores but
  leaves `RALPHY_COPILOT_DB`, `RALPHY_CURSOR_DIR` and `RALPHY_GEMINI_DIR`
  inherited, so the scratch daemon could read the operator's live stores for
  those three. No scenario here reads usage data, and the omission is
  byte-identical in all six sibling `wb_*.py` scripts — a family-wide fix, not
  a #306 one.
- Nothing in CI runs `crates/ralphy-daemon/tests/wb_*.py`; this pass is a
  manual gate (unchanged from #303).

## Divergences

None. Every criterion of #306 is asserted by the script and green on both
Windows and Linux.
