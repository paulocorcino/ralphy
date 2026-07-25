"""#306 operator acceptance walkthrough (HITL, post-remediation, #300-#305).

One consolidated Playwright pass over a REAL daemon and ONE browser process,
asserting that none of the symptoms PRD #296 set out to fix reproduce anymore.
Every scenario asserts external behaviour an operator can see.

Scenario 1  the canvas tab reads "Consoles", and no translation affordance is
            reachable anywhere (#305)
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
            screenshots exist non-empty

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

results = []


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
    empty = tempfile.mkdtemp(prefix="wb306_empty_")
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
    d = tempfile.mkdtemp(prefix="wb306_fixture_")
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
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb306_reg_")
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
            page.wait_for_timeout(500)
            tab_state = page.evaluate(f"() => {SH}.tabs.map((t) => ({{ id: t.id, closable: t.closable }}))")
            check(
                "a file tab rides in AFTER the fixed consoles tab",
                len(tab_state) == 2
                and tab_state[0] == {"id": "consoles", "closable": False}
                and tab_state[1]["closable"] is True,
                f"got={tab_state}",
            )

            check("window.WBTranslate is undefined", page.evaluate("() => typeof window.WBTranslate") == "undefined")
            # With the Runs panel AND a markdown tab open — the two surfaces the
            # removed feature used to hang its affordances off.
            page.evaluate(f"() => {SH}.toggleRuns()")
            page.wait_for_timeout(400)
            check("the Runs panel is open for the translation sweep", page.evaluate(f"() => {SH}.runsOpen") is True)
            for sel in (".plan-xlate", '[data-act="xlate"]', ".md-xlate-note"):
                check(f"{sel} count is 0 anywhere in the interface", page.locator(sel).count() == 0)
            resp = page.request.get(BASE + "wb-translate.js")
            check("GET /wb-translate.js returns 404", resp.status == 404, f"got={resp.status}")
            page.evaluate(f"() => {SH}.toggleRuns()")
            page.wait_for_timeout(300)

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
            check("GET /api/agents serves a non-empty roster", len(roster) >= 1, f"got={len(roster)} rows")
            rows = open_menu(page)
            check(
                "the menu renders one row per roster entry, plus the plain console",
                rows.count() == len(roster) + 1,
                f"rows={rows.count()} roster={len(roster)}",
            )
            labels = rows.locator("span:not(.row-live):not(.row-new)").all_inner_texts()
            check(
                "…carrying the daemon's ids, in the daemon's order, console LAST",
                labels == [r["id"] for r in roster] + ["console"],
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

            ctx.close()
            browser.close()

            # =============== scenario 9a: no uncaught error ===================
            check("zero pageerror events captured over the whole pass", page_errors == [], f"got={page_errors}")
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 30
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("ALL SYMPTOMS NOT REPRODUCIBLE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
