# #306 operator acceptance walkthrough — 2026-07-25

Daemon: real, one process, one browser process, Localhost policy on port 7396
over a scratch `RALPHY_DAEMON_DIR`. Produced by
`crates/ralphy-daemon/tests/wb_accept_306.py`, 70/70 checks passed,
`ALL SYMPTOMS NOT REPRODUCIBLE`.

The consolidated human-in-the-loop gate for PRD #296 (#300–#305). Nine scenarios
in ONE browser process against ONE daemon, each asserting external behaviour an
operator can see.

| # | Slice | Criterion it closes | Result | Screenshot |
|---|-------|---------------------|--------|------------|
| 1 | #305 | The canvas tab reads Consoles · No translation affordance is present anywhere | PASS | none (tab-strip + absence asserts; the desk shot below shows the strip) |
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
  sh -c "git --version || (apt-get update && apt-get install -y git); \
         python crates/ralphy-daemon/tests/wb_accept_306.py"
```

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
  `{id: "consoles", closable: false}`. Translation: `window.WBTranslate` is
  `undefined`, `.plan-xlate` / `[data-act="xlate"]` / `.md-xlate-note` each
  count 0 with the Runs panel AND a markdown tab open, and
  `GET /wb-translate.js` returns 404.
- **Scenario 2** — a free console is parked mid-workspace first (a window
  against the origin would clamp, and the drags would prove the clamp rather
  than the anchored edge). A `-120px` west drag widens it while
  `left + width` is unchanged and `top` is untouched; a `-90px` north drag
  grows it while `top + height` is unchanged; an `se` corner drag grows BOTH
  axes while `left` and `top` hold.
- **Scenario 3** — the menu's vendor labels equal, in order, the `id`s
  returned by `GET /api/agents` (7 roster rows + the plain-console row = 8),
  compared against the endpoint rather than a hardcoded list so the assertion
  survives onboarding a vendor (ADR-0040). Before any agent launch only the
  plain-console row is live (scenario 2's free console); after clicking the
  claude row, `GET /api/sessions` carries a `kind: "agent"` claude session and
  the badge map reads `{console: "1 live", claude: "1 live"}` — claude is the
  only vendor row marked.
- **Scenario 4** — stubbed `board.list` calls are counted. The board closed:
  0 folds. Opening it: 1. `.kanban-refresh`: +1. `document.visibilityState`
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
  `.running`, it is `#72`, its `.kc-run` pill is VISIBLE
  (`offsetParent !== null`, since an Alpine-hidden pill is still in the DOM)
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
  #303).
- **Scenario 9** — zero `pageerror` events over the whole pass, and each of the
  four screenshots exists with size > 0 AND an mtime from this run, so a
  previous run's committed bytes cannot stand in for one this run never took.

## Review-only (human judgment)

- The operator's own eye over the four screenshots (layout, clipping,
  look-and-feel) — every behavioural criterion above is machine-asserted; only
  the visual impression is not.

## Divergences

None. Every criterion of #306 is asserted by the script and green on both
Windows and Linux.
