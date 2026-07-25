"""#306 operator acceptance walkthrough (HITL, post-remediation, #300-#305).

One consolidated Playwright pass over a REAL daemon and ONE browser process,
asserting that none of the symptoms PRD #296 set out to fix reproduce anymore.
Every scenario asserts external behaviour an operator can see.

Scenario 1  the canvas tab reads "Consoles"; `window.WBTranslate` is undefined
            and `GET /wb-translate.js` 404s (#305)
Scenario 1b the DOM half of that sweep, DEFERRED until scenario 6 has put a run
            on disk: the plan pane is `x-if`-gated on a live run, so a count of
            0 taken earlier would prove nothing. Positive controls first.
Scenario 2  a console window resizes from the west and north EDGES holding the
            opposite edge, and from the se CORNER in both axes (#303)
Scenario 3  the console menu renders the daemon's own roster, in order, and
            marks the agent that has a live session (#304)
Scenario 4  the board refreshes on the explicit control and on the document
            becoming visible, and never while it is hidden (#301)
Scenario 5  the issue drawer shows a real body and a comment thread carrying
            author and a rendered date (#302)
Scenario 6  the Runs panel shows a run started from a TERMINAL — a separate OS
            process writes the snapshot — and advances live during it (#300)
Scenario 7  the worked issue's card is visibly marked, and one click on its
            pill reaches that run in the trail (#301)
Scenario 8  a DAEMON restart restores the desk: the free console comes back on
            its own, the agent console as a placeholder, both in their saved
            rectangles, with no agent session launched (#303)
Scenario 9  no uncaught page error over the whole pass, and all four dated
            screenshots exist non-empty AND were written by THIS run

Stub / real boundary. `board.list`, `label.set` and `issue.show` are stubbed at
`WBDaemon.observe`, which FALLS THROUGH to the real transport for every other
verb: the board fold spawns a CLI making tracker calls a throwaway fixture repo
cannot answer. Everything else is real — `runs.list`/`runs.watch`, `tree.*`,
`GET /api/agents`, `GET /api/sessions`, the PTY websockets, the daemon process
itself and its restart. The refresh POLICY, the navigation and the drawer
RENDER under test here are all client-side and untouched by the stub; the folds
themselves ride #198's and #302's own tests.

Scenario 6's run is a snapshot WRITER, not a real `ralphy run`: a real run needs
a GitHub tracker, a vendor CLI and quota, all out of scope for PRD #296. The
half this proves is the half the criterion names — the browser did not spawn the
run and learns of it from the on-disk snapshot contract, written by a separate
OS process. That a real vendor run writes this shape stays carried by
`runstate/capture.rs`'s unit pins.

Boots a Localhost daemon on 7396 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. Agent launches
resolve to `session_test_child` via `RALPHY_DAEMON_AGENT_OVERRIDE`, so no vendor
CLI is required and no quota is spent. The daemon is stopped by its own
subprocess handle, NEVER by name (`ralphy.exe` doubles as the orchestrator on
this host).

Writes docs/screenshots/306-{consoles-desk,agent-menu,board-run,runs-live}-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_accept_306.py            (exit 0 = all pass)
Linux: RALPHY_WB_TARGET=/w/target/linux/debug python crates/ralphy-daemon/tests/wb_accept_306.py
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

# The Windows console's default codepage (cp1252 here) cannot encode the glyphs
# this script prints in its detail strings; force utf-8 stdout so a PASSING
# assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7396
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_accept_306.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
WIN = os.name == "nt"
# The Linux leg's browser container has playwright but no cargo, and its binaries
# live in a separate target dir — so the paths are overridable and `build()` is
# skipped when they are overridden.
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if WIN else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if WIN else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The account dropdown reuses `.dropdown-item`, so every menu query is scoped (#304).
MENU = ".console-menu"

SHOTS = [
    "306-consoles-desk-2026-07-25.png",
    "306-agent-menu-2026-07-25.png",
    "306-board-run-2026-07-25.png",
    "306-runs-live-2026-07-25.png",
]

RUN_ID = "20260725T100000-306"
BODY = "The #306 fixture body sentence, long enough to be a real issue body."
RAW_AT = "2026-07-23T12:34:56Z"

PLAN_MD = """# Plan for #72: the fixture issue

## Steps
- [x] the done step
- [ ] the open step
"""

# The spy: count every `board.list` call and answer it from a fixed fold, answer
# `label.set` OK, answer `issue.show` with the structured detail the CLI emits
# (`comments[] = {author, at, body}`) — and DELEGATE every other verb to the real
# transport. The fall-through is load-bearing: scenario 6's live `runs.*` pushes
# ride this same `observe`.
SPY_JS = """
(k) => {
  window.__boardCalls = [];
  window.__triggers = [];
  const real = window.WBDaemon.observe.bind(window.WBDaemon);
  const row = (n, title) => ({
    number: n, title, state: "open", labels: ["ready-for-agent"],
    assignees: [], blocked_by: [], created: "2026-07-20T10:00:00Z", updated: "2026-07-24T10:00:00Z",
  });
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === "board.list") {
      window.__boardCalls.push(payload);
      return Promise.resolve({
        status: "ok",
        board: {
          issues: [row(71, "the done one"), row(72, "the active one"), row(73, "the pending one")],
          labels: [{ name: "ready-for-agent", color: "0E8A16" }],
        },
      });
    }
    if (verb === "label.set") return Promise.resolve({ status: "ok" });
    if (verb === "issue.show") {
      return Promise.resolve({
        status: "ok",
        issue: {
          number: payload.number,
          body: k.body,
          comments: [
            { author: "octocat", at: k.rawAt, body: "first comment" },
            { author: "paulocorcino", at: "2026-07-24T09:00:00Z", body: "second comment" },
          ],
          blocked_by: [],
        },
      });
    }
    return real(verb, payload);
  };
  // Attribute each ACCEPTED load to the trigger that asked for it: the recorded
  // payload is `{repo}` for every trigger, so a count delta alone cannot tell a
  // `visible` refresh from a stray `runs` push.
  const d = Alpine.$data(document.querySelector('[x-data]'));
  const realMaybe = d.maybeRefreshBoard.bind(d);
  d.maybeRefreshBoard = (trigger) => {
    const before = window.__boardCalls.length;
    const out = realMaybe(trigger);
    if (window.__boardCalls.length > before) window.__triggers.push(trigger);
    return out;
  };
}
"""

# Scenario 6's writer, run as a SEPARATE OS process: it copies a pre-rendered
# snapshot document into the fixture repo's runstate dir, waits, then replaces it
# with the advanced one. Nothing in the browser spawns or drives it.
WRITER = """
import shutil, sys, time
v1, v2, dst, hold = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
shutil.copyfile(v1, dst)
time.sleep(hold)
shutil.copyfile(v2, dst)
"""


def snapshot(phase, status_72):
    return {
        "v": 1,
        "runid": RUN_ID,
        "pid": os.getpid(),  # a LIVE pid, so the reader never sweeps it as an orphan
        "title": "the #306 fixture run",
        "repo": "owner/accept306",
        "branch": "afk/accept-306",
        "plan_agent": "claude",
        "exec_agent": "opencode",
        "started_at": "2026-07-25T10:00:00-03:00",
        "plan_path": ".ralphy/plan.md",
        "queue": {"total": 3, "order": [71, 72, 73], "stop_before": None},
        # Issue 72 starts `planning`, NOT `pending`: `issueState` renders the
        # ACTIVE non-terminal issue as `executing` for any status other than
        # literally `planning`, so a `pending` fixture would satisfy the advance
        # oracle before the run ever advanced (#300).
        "issues": [
            {"number": 71, "title": "the done one", "status": "done", "blocked_by": []},
            {"number": 72, "title": "the active one", "status": status_72, "blocked_by": []},
            {"number": 73, "title": "the pending one", "status": "pending", "blocked_by": []},
        ],
        "phase": {"active": 72, "state": phase, "sleep": None, "final_summary": None},
    }


results = []
# Every scratch dir this pass creates, swept in `main()`'s `finally` — a daemon
# registry, a git fixture repo, a snapshot staging dir and one empty vendor-store
# dir per daemon launch would otherwise leak per run.
temp_dirs = []


def scratch(prefix):
    d = tempfile.mkdtemp(prefix=prefix)
    temp_dirs.append(d)
    return d


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def stop(proc):
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    """A scratch registry + empty vendor stores, and every agent launch pointed at
    the session test child: the operator's daemon dir is never touched and no
    vendor CLI is required to prove the launch path (#304)."""
    empty = scratch("wb306_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo():
    """A throwaway git repo with a real plan.md and an EMPTY runstate dir — the
    Runs panel starts at zero runs and scenario 6's documents all arrive while it
    is open. `.gitignore` hides `.ralphy/`, so the watcher's exemption is live."""
    d = scratch("wb306_fixture_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / ".ralphy").mkdir()
    (p / ".ralphy" / "plan.md").write_text(PLAN_MD, encoding="utf-8")
    (p / ".ralphy" / "runstate").mkdir()
    (p / "README.md").write_text("# fixture\n\nThe #306 acceptance fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb306@example.com"],
        ["git", "config", "user.name", "wb306"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's bundle. The
    # helper child is the stand-in every agent launch resolves to.
    if os.environ.get("RALPHY_WB_TARGET"):
        return  # pre-built elsewhere (the Linux leg's browser container has no cargo)
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bins"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def board_calls(page):
    return page.evaluate("() => window.__boardCalls.length")


def open_menu(page):
    """Open the New console dropdown and return its rendered rows."""
    if not page.evaluate(f"() => {SH}.agentMenu"):
        page.evaluate(f"() => {{ {SH}.agentMenu = true; }}")
    page.wait_for_timeout(250)
    return page.locator(f"{MENU} .dropdown-item")


def close_menu(page):
    page.evaluate(f"() => {{ {SH}.agentMenu = false; }}")
    page.wait_for_timeout(150)


def row_by_label(page, label):
    return page.locator(f"{MENU} .dropdown-item", has=page.locator(f"span:text-is('{label}')")).first


def menu_live(page):
    """`{row label: live text}` for the menu rows showing a VISIBLE live badge.
    `.row-live` is `x-show`-gated (display:none, not removed), so a bare count
    would report every row as live."""
    return page.evaluate(
        f"""() => Object.fromEntries(
             [...document.querySelectorAll('{MENU} .dropdown-item')].map((b) => {{
               const lab = b.querySelector('span:not(.row-live):not(.row-new)')?.textContent.trim();
               const el = b.querySelector('.row-live');
               return [lab, el && el.offsetParent !== null ? el.textContent.trim() : null];
             }}).filter(([, v]) => v !== null))"""
    )


def open_console(page, slug):
    """Open a free console and wait for its live terminal."""
    before = page.locator(".session-window").count()
    page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000
    )
    win = page.locator(".session-window").nth(before)
    win.locator(".xterm").wait_for(timeout=15000)
    page.wait_for_timeout(400)
    return win


def rect_of(page, index):
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " return { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight }; }",
        index,
    )


def drag_handle(page, index, dir_, dx, dy):
    """Press the given window's `dir` handle and drag it by (dx, dy)."""
    box = page.evaluate(
        "([i, d]) => { const w = document.querySelectorAll('.session-window')[i];"
        " const h = w.querySelector('.h-' + d); const r = h.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        [index, dir_],
    )
    page.mouse.move(box["x"], box["y"])
    page.mouse.down()
    page.mouse.move(box["x"] + dx / 2, box["y"] + dy / 2, steps=3)
    page.mouse.move(box["x"] + dx, box["y"] + dy, steps=3)
    page.mouse.up()
    page.wait_for_timeout(350)


def main():
    started = time.time() - 1  # -1s of slack against filesystem mtime granularity
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = scratch("wb306_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md). `--no-sandbox`
            # is required to run chromium as root inside the Linux container.
            browser = p.chromium.launch(
                headless=True, args=["--disable-webgl", "--disable-gpu", "--no-sandbox"]
            )
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page_errors = []
            page.on("pageerror", lambda exc: page_errors.append(str(exc)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(SPY_JS, {"body": BODY, "rawAt": RAW_AT})
            page.wait_for_timeout(300)

            # =============== scenario 1: the Consoles tab, no translation =====
            tabs = page.locator(".tabstrip .tab")
            check("the tab strip renders exactly one fixed tab", tabs.count() == 1, f"got={tabs.count()}")
            title = tabs.first.locator(".tab-title").inner_text()
            check("…and its title reads exactly Consoles", title == "Consoles", f"got={title!r}")
            # `.tab-close` is `x-show`-gated (display:none, not removed), so a
            # bare `.count()` would pass on a hidden element — assert visibility.
            check(
                "…rendering no VISIBLE close button",
                tabs.first.locator(".tab-close:visible").count() == 0,
                "",
            )

            page.evaluate(f"() => {SH}.toggle('{slug}')")
            page.wait_for_timeout(400)
            page.evaluate(
                f"""() => {{
                    {SH}.openTab({{ project: '{slug}', path: 'README.md',
                                    title: 'README.md', ftype: 'markdown' }});
                }}"""
            )
            # POSITIVE CONTROL for the viewer half of the sweep below: `openTab`
            # defers through `$nextTick` and a `file.read`, and mounts NOTHING when
            # the read fails — so a viewer that never appeared would report 0
            # `[data-act="xlate"]` for a reason unrelated to #305's removal.
            page.wait_for_selector('[data-act="reload"]', timeout=10000)
            tab_state = page.evaluate(f"() => {SH}.tabs.map((t) => ({{ id: t.id, closable: t.closable }}))")
            check(
                "a file tab rides in AFTER the fixed consoles tab",
                len(tab_state) == 2
                and tab_state[0] == {"id": "consoles", "closable": False}
                and tab_state[1]["closable"] is True,
                f"got={tab_state}",
            )
            check(
                "…and IT renders a visible close button",
                page.locator(".tabstrip .tab").nth(1).locator(".tab-close:visible").count() > 0,
                "",
            )

            check("window.WBTranslate is undefined", page.evaluate("() => typeof window.WBTranslate") == "undefined")
            resp = page.request.get(BASE + "wb-translate.js")
            check("GET /wb-translate.js returns 404", resp.status == 404, f"got={resp.status}")
            # The DOM sweep for `.plan-xlate` is DEFERRED to scenario 6: the plan
            # pane lives inside `x-if="openSlug && projectRuns().length"`
            # (index.html:496), so with no run on disk the whole subtree is absent
            # and a count of 0 here would prove nothing about the removal.

            # =============== scenario 2: resize from an edge and a corner =====
            # The Consoles tab must be in view or every terminal measures 0x0 —
            # opening README.md above made the file tab the active one (#303).
            page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            page.wait_for_timeout(300)
            open_console(page, slug)
            # Park it mid-workspace first: a window already against the origin
            # would clamp, and every drag below would prove the CLAMP rather than
            # the anchored edge it exists to pin (#303).
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " w.style.left = '200px'; w.style.top = '140px';"
                " w.style.width = '460px'; w.style.height = '320px'; }"
            )
            page.wait_for_timeout(250)
            r0 = rect_of(page, 0)
            check(
                "the console under test sits clear of the workspace origin",
                r0["left"] >= 120 and r0["top"] >= 100,
                f"got={r0}",
            )

            drag_handle(page, 0, "w", -120, 0)
            r1 = rect_of(page, 0)
            check("a west-edge drag widens the window", r1["width"] > r0["width"], f"{r0['width']} -> {r1['width']}")
            check(
                "…holding the right edge in place",
                r1["left"] + r1["width"] == r0["left"] + r0["width"],
                f"{r0['left'] + r0['width']} -> {r1['left'] + r1['width']}",
            )
            check("…and the top edge untouched", r1["top"] == r0["top"], f"{r0['top']} -> {r1['top']}")

            drag_handle(page, 0, "n", 0, -90)
            r2 = rect_of(page, 0)
            check(
                "a north-edge drag grows the window upward",
                r2["height"] > r1["height"],
                f"{r1['height']} -> {r2['height']}",
            )
            check(
                "…holding the bottom edge in place",
                r2["top"] + r2["height"] == r1["top"] + r1["height"],
                f"{r1['top'] + r1['height']} -> {r2['top'] + r2['height']}",
            )

            drag_handle(page, 0, "se", 80, 60)
            r3 = rect_of(page, 0)
            check(
                "an se-corner drag grows BOTH axes",
                r3["width"] > r2["width"] and r3["height"] > r2["height"],
                f"{r2['width']}x{r2['height']} -> {r3['width']}x{r3['height']}",
            )
            check(
                "…moving neither the left nor the top edge",
                r3["left"] == r2["left"] and r3["top"] == r2["top"],
                f"{(r2['left'], r2['top'])} -> {(r3['left'], r3['top'])}",
            )

            # =============== scenario 3: the console menu IS the roster =======
            # Compared against the daemon's own `GET /api/agents`, never a
            # hardcoded vendor list: hardcoding would re-create in the test the
            # second enumeration site ADR-0040 and #304 removed from the frontend.
            roster = page.request.get(BASE + "api/agents").json()
            ids = [r["id"] for r in roster]
            # The exact count is pinned in Rust (`roster.rs::accelerators_are_unique_and_stable`);
            # here a floor + uniqueness is what keeps a COLLAPSED roster from
            # satisfying the relation below trivially, without re-enumerating
            # vendors in a second place (ADR-0040).
            check(
                "GET /api/agents serves the full roster, ids unique",
                len(roster) >= 5 and len(set(ids)) == len(ids),
                f"got={ids}",
            )
            rows = open_menu(page)
            check(
                "the menu renders one row per roster entry, plus the plain console",
                rows.count() == len(roster) + 1,
                f"rows={rows.count()} roster={len(roster)}",
            )
            labels = rows.locator("span:not(.row-live):not(.row-new)").all_inner_texts()
            # The row renders `label || id` (wb-agents.js), so compare against the
            # SAME fallback — an adapter whose label differs from its id is the
            # onboarding case this relation exists to survive.
            check(
                "…carrying the daemon's labels, in the daemon's order, console LAST",
                labels == [(r.get("label") or r["id"]) for r in roster] + ["console"],
                f"got={labels}",
            )
            # Scenario 2 left ONE free console running, so the plain-console row
            # is legitimately live here — no VENDOR row is, and that is the state
            # the launch below has to change.
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(400)
            before_live = menu_live(page)
            check(
                "before any agent launch only the plain-console row is marked live",
                before_live == {"console": "1 live"},
                f"got={before_live}",
            )

            row_by_label(page, "claude").click()
            page.wait_for_timeout(300)
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window .xterm').length === 2", timeout=20000
            )
            page.wait_for_timeout(800)
            live_sessions = page.request.get(BASE + "api/sessions").json()
            check(
                "clicking the claude row launches an agent session",
                any(s["agent"] == "claude" and s["kind"] == "agent" for s in live_sessions),
                f"got={live_sessions}",
            )
            # The presence tick feeds the fold; ask for it now rather than waiting.
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(400)
            open_menu(page)
            claude_live = row_by_label(page, "claude").locator(".row-live").inner_text()
            check("…and the claude row now reads its live session", claude_live == "1 live", f"got={claude_live!r}")
            after_live = menu_live(page)
            check(
                "…and it is the ONLY vendor row marked live",
                after_live == {"console": "1 live", "claude": "1 live"},
                f"got={after_live}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "306-agent-menu-2026-07-25.png"))
            close_menu(page)

            # =============== scenario 4: when the board refreshes =============
            # Drive the one automatic trigger there is, rather than trusting that
            # nothing happened to fire: the 30s backstop is gated on a 120s gap,
            # so an undriven "0 folds" is only true because no tick was eligible.
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 130000; {SH}.boardBackstopTick(); }}")
            page.wait_for_timeout(500)
            check("a backstop tick with the board CLOSED spawns no fold", board_calls(page) == 0, f"got={board_calls(page)}")
            page.evaluate(f"() => {SH}.toggleKanban()")
            page.wait_for_timeout(600)
            check("opening the board loads it once", board_calls(page) == 1, f"got={board_calls(page)}")

            n = board_calls(page)
            page.click(".kanban-refresh")
            page.wait_for_timeout(500)
            check("the explicit refresh control reloads the board", board_calls(page) == n + 1, f"{n} -> {board_calls(page)}")

            # Headless chromium does NOT background a page when a sibling takes
            # the foreground (#301), so the STATE — the predicate's own input —
            # is stubbed; the listener, the predicate and the wiring stay real.
            page.evaluate(
                "() => { Object.defineProperty(document, 'visibilityState',"
                " { get: () => 'hidden', configurable: true }); }"
            )
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            n = board_calls(page)
            page.evaluate("() => document.dispatchEvent(new Event('visibilitychange'))")
            page.wait_for_timeout(700)
            check("a HIDDEN document never refreshes the board", board_calls(page) == n, f"{n} -> {board_calls(page)}")

            page.evaluate("() => { delete document.visibilityState; }")
            page.evaluate(f"() => {{ {SH}._boardLoadedAt = Date.now() - 10000; }}")
            n = board_calls(page)
            page.evaluate("() => document.dispatchEvent(new Event('visibilitychange'))")
            page.wait_for_timeout(700)
            check(
                "becoming visible again refreshes it",
                board_calls(page) == n + 1,
                f"{n} -> {board_calls(page)}",
            )
            trig = page.evaluate("() => window.__triggers")
            check(
                "…attributed to the `visible` trigger, not a stray push",
                (trig[-1] if trig else None) == "visible",
                f"got={trig[-3:]}",
            )

            # =============== scenario 5: the issue drawer =====================
            cards = page.locator(".kanban-card")
            check("the board renders the fixture rows", cards.count() == 3, f"got={cards.count()}")
            page.locator('.kanban-card:has(.kc-num:text-is("#72"))').first.click()
            page.wait_for_timeout(700)
            drawer = page.locator(".kanban-detail.open")
            check("clicking a card opens the detail drawer", drawer.is_visible())
            body_txt = drawer.locator(".kd-body").inner_text().strip()
            check("…showing the issue's real body", BODY in body_txt, f"got={body_txt[:80]!r}")
            # The head is `text-transform: uppercase` in styles.css, so
            # `inner_text()` returns "2 COMMENTS" — compare case-folded (#302).
            head = drawer.locator(".kd-comments-head").inner_text().strip()
            check("…and counting the comment thread", head.lower() == "2 comments", f"got={head!r}")
            first = drawer.locator(".kd-comment").first
            head0 = first.locator(".kd-comment-head").inner_text().strip()
            check("the first comment names its author", "octocat" in head0, f"got={head0!r}")
            at0 = first.locator(".kd-comment-at").inner_text().strip()
            # Never a formatted-date LITERAL: `fmtDate` follows the browser locale
            # (#302). Non-empty AND != the raw ISO fails both a dropped `at` and a
            # wire shape that dumped it unformatted.
            check(
                "…and a rendered date, neither blank nor the raw ISO timestamp",
                at0 != "" and at0 != RAW_AT,
                f"got={at0!r}",
            )
            body0 = first.locator(".kd-comment-body").inner_text().strip()
            check("…and its body", "first comment" in body0, f"got={body0[:60]!r}")
            page.evaluate(f"() => {SH}.closeIssue()")
            page.wait_for_timeout(300)

            # =============== scenario 6: a run started from a TERMINAL ========
            page.evaluate(f"() => {SH}.toggleRuns()")
            page.wait_for_function(f"() => {SH}.projectRuns().length === 0", timeout=10000)
            check("the Runs panel opens empty — no run document on disk yet", True)

            staging = Path(scratch("wb306_snap_"))
            v1, v2 = staging / "v1.json", staging / "v2.json"
            v1.write_text(json.dumps(snapshot("planning", "planning")), encoding="utf-8")
            v2.write_text(json.dumps(snapshot("executing", "executing")), encoding="utf-8")
            doc = Path(fixture_dir, ".ralphy", "runstate", f"{RUN_ID}.json")
            writer = subprocess.Popen([sys.executable, "-c", WRITER, str(v1), str(v2), str(doc), "3.0"])

            page.wait_for_function(f"() => {SH}.projectRuns().length === 1", timeout=20000)
            check("a run started outside the browser appears with NO operator action", True)
            page.wait_for_function(
                f"() => {SH}.runPhaseLabel({SH}.currentRun()) === 'planning #72'", timeout=20000
            )
            check("…reading its planning phase", True)
            # `wait_for_function`, never a sleep: the point is the panel advances
            # on its own while the writer is still running.
            page.wait_for_function(
                f"() => {SH}.runPhaseLabel({SH}.currentRun()) === 'executing #72'", timeout=25000
            )
            label = page.evaluate(f"() => {SH}.runPhaseLabel({SH}.currentRun())")
            check("…and advances live to the executing issue", label == "executing #72", f"got={label!r}")
            prog = page.locator(".run-select-btn .run-prog").inner_text().strip()
            check("the progress counter reads completed/queue-total", prog == "1/3", f"got={prog!r}")
            page.screenshot(path=os.path.join(SHOT_DIR, "306-runs-live-2026-07-25.png"))
            try:
                writer.wait(timeout=30)
            finally:
                if writer.poll() is None:  # never leave it rewriting the snapshot
                    writer.kill()
            check("the out-of-process writer exited cleanly", writer.returncode == 0, f"rc={writer.returncode}")

            # =============== scenario 1b: the translation sweep, for real =====
            # NOW the plan pane exists: `.runs-body` is `x-if`-gated on a live run
            # (index.html:496), and the markdown viewer from scenario 1 is still
            # mounted. Both positive controls are asserted FIRST, so a count of 0
            # below can only mean the affordance is gone.
            check(
                "the plan pane is rendered (positive control for the sweep)",
                page.locator(".plan-wrap").count() == 1,
                f"got={page.locator('.plan-wrap').count()}",
            )
            reload_btns = page.locator('[data-act="reload"]').count()
            check("…and the markdown viewer toolbar is still mounted", reload_btns >= 1, f"got={reload_btns}")
            for sel in (".plan-xlate", '[data-act="xlate"]', ".md-xlate-note"):
                check(f"{sel} count is 0 anywhere in the interface", page.locator(sel).count() == 0)

            # =============== scenario 7: the running card, and back to the run =
            page.evaluate(f"() => {{ if ({SH}.runsOpen) {SH}.toggleRuns(); }}")
            page.wait_for_timeout(400)
            check("the Runs panel is closed for the board->run leg", page.evaluate(f"() => {SH}.runsOpen") is False)
            marked = page.evaluate(
                """() => {
                     const c = document.querySelector('.kanban-card.running');
                     const pill = c && c.querySelector('.kc-run');
                     return {
                       num: c ? c.querySelector('.kc-num').textContent.trim() : null,
                       // A rendered WIDTH, not `offsetParent`: the card's
                       // `running` class and the pill's `x-show` evaluate the
                       // SAME expression, so an offsetParent read cannot fail
                       // independently of the class asserted beside it.
                       shown: !!pill && pill.getBoundingClientRect().width > 0,
                       txt: (c?.querySelector('.kc-run .kc-run-txt')?.textContent || '').trim(),
                       others: document.querySelectorAll('.kanban-card.running').length,
                     };
                   }"""
            )
            check("the worked issue's card is marked running", marked["num"] == "#72", f"got={marked}")
            check("…with a VISIBLE run pill", marked["shown"] is True, f"got={marked}")
            check("…naming the phase and the agent", marked["txt"] == "executing · opencode", f"got={marked['txt']!r}")
            check("…and no other card is marked", marked["others"] == 1, f"got={marked['others']}")
            page.screenshot(path=os.path.join(SHOT_DIR, "306-board-run-2026-07-25.png"))

            page.click(".kanban-card.running .kc-run")
            page.wait_for_timeout(700)
            check("clicking the pill opens the Runs panel", page.evaluate(f"() => {SH}.runsOpen") is True)
            focused = page.evaluate(
                "() => Array.from(document.querySelectorAll('.trail-node.focus'))"
                ".map((e) => e.getAttribute('data-issue'))"
            )
            check("…on that issue, marked in the trail", focused == ["72"], f"got={focused}")
            # `@click.stop` — the pill goes to the run, never to the drawer too.
            check(
                "…without falling through to the card's detail drawer",
                page.evaluate(f"() => {SH}.kanbanSel") is None,
                f"kanbanSel={page.evaluate(f'() => {SH}.kanbanSel')!r}",
            )

            # =============== scenario 8: the desk survives a DAEMON restart ===
            page.evaluate(f"() => {{ if ({SH}.runsOpen) {SH}.toggleRuns(); }}")
            page.evaluate(f"() => {{ if ({SH}.kanbanOpen) {SH}.toggleKanban(); }}")
            page.wait_for_timeout(400)
            # NOTHING is maximized here on purpose: `.maximized` pins all four
            # offsets via `!important`, so a snapshot of one reads 0/0 and the
            # comparison below would go blind (#303).
            check(
                "no window under test is maximized (the rect comparison stays honest)",
                page.evaluate(
                    "() => [...document.querySelectorAll('.session-window')]"
                    ".every((w) => !w.classList.contains('maximized'))"
                )
                is True,
            )
            pre = [rect_of(page, 0), rect_of(page, 1)]
            kinds = page.evaluate(
                "() => JSON.parse(localStorage.getItem('wb.desk.v1')).map((r) => r.kind)"
            )
            check("the desk holds one free console and one agent console", kinds == ["console", "agent"], f"got={kinds}")

            stop(proc)
            proc = launch(daemon_dir)
            check("the daemon restarted on the same port", wait_listening(BASE))
            check(
                "…with no sessions at all (the restart really invalidated them)",
                page.request.get(BASE + "api/sessions").json() == [],
                "",
            )

            sockets = []
            page.on("websocket", lambda ws: sockets.append(ws.url))
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            # A background tab measures 0x0, and every rect below is an offset read.
            page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window').length === 2", timeout=20000
            )
            page.wait_for_timeout(1500)

            live = page.locator(".session-window:not(.placeholder)")
            ph = page.locator(".session-window.placeholder")
            check("the free console comes back on its own", live.count() == 1, f"got={live.count()}")
            check("the agent console comes back as a placeholder", ph.count() == 1, f"got={ph.count()}")
            check(
                "…with a live terminal in the free console only",
                live.locator(".xterm").count() == 1 and ph.locator(".xterm").count() == 0,
                "",
            )
            check("…offering one click to reconnect", ph.locator(".session-reconnect").count() == 1, "")
            ph_title = ph.locator(".session-title").inner_text()
            check(
                "the placeholder keeps its agent and its repo",
                "claude" in ph_title and slug in ph_title,
                f"title={ph_title!r}",
            )
            post = [rect_of(page, 0), rect_of(page, 1)]
            check("both windows return to their saved rectangles", post == pre, f"{pre} -> {post}")

            # The two NEGATIVE assertions below are point-in-time reads in the
            # false-pass direction — a vendor spawn arriving late would be
            # invisible. Take them only AFTER every positive assertion above has
            # landed, plus one more settle window.
            page.wait_for_timeout(2500)
            sessions = page.request.get(BASE + "api/sessions").json()
            check(
                "restoring the desk launched NO agent session",
                all(s["kind"] != "agent" for s in sessions),
                f"got={sessions}",
            )
            # `/ws` is the daemon's control channel, always opened; only
            # `/ws/session` sockets launch or attach a PTY (#303).
            session_sockets = [u for u in sockets if "/ws/session" in u]
            check(
                "…and opened exactly one session socket, for the free console",
                len(session_sockets) == 1
                and "console=1" in session_sockets[0]
                and "agent=" not in session_sockets[0],
                f"sockets={sockets}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "306-consoles-desk-2026-07-25.png"))

            ctx.close()
            browser.close()

            # =============== scenario 9: no uncaught error, real screenshots ==
            check("zero pageerror events captured over the whole pass", page_errors == [], f"got={page_errors}")
            # `mtime >= started`, not just "exists": a previous run's committed
            # bytes would otherwise stand in for a screenshot THIS run never took.
            for name in SHOTS:
                path = os.path.join(SHOT_DIR, name)
                size = os.path.getsize(path) if os.path.exists(path) else 0
                fresh = os.path.exists(path) and os.path.getmtime(path) >= started
                check(f"{name} was written by THIS run, non-empty", size > 0 and fresh, f"bytes={size} fresh={fresh}")
    finally:
        stop(proc)
        for d in temp_dirs:
            shutil.rmtree(d, ignore_errors=True)  # git objects are read-only on Windows

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks. Pinned at
    # the REAL count, not a loose lower bound — at 30 the last five scenarios
    # could be deleted wholesale and the script would still report success.
    ok = all(results) and len(results) >= 72
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("ALL SYMPTOMS NOT REPRODUCIBLE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
