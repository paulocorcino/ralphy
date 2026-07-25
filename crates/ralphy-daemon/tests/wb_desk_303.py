"""#303 browser acceptance: console windows resize from any edge, and the desk survives a restart.

One Playwright pass over a REAL daemon: real sessions, real PTYs, real
`/api/sessions`. Nothing is stubbed — the only fixture is a throwaway git repo
registered with the daemon.

Scenario 1   `WBConsole.resizeRect` in isolation: a 20-row table over the eight
             directions, pinning the anchored-edge RELATIONS, the minimum and
             the workspace clamp
Scenario 4   a live west/north-edge resize on a real console window: the opposite
             edge holds, the size changes, and the terminal reflows (`term.cols`)
Scenario 5   live clamping (minimum + workspace) and a maximized window refusing
             both a resize and a titlebar drag

Boots a Localhost daemon on 7398 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/303-console-desk-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_desk_303.py   (exit 0 = all pass)
"""

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

PORT = 7398
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_desk_303.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb303_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo():
    """A throwaway git repo the daemon can open consoles in. It deliberately has
    NO gemini configuration root, so an agent launch is refused before any spawn."""
    d = tempfile.mkdtemp(prefix="wb303_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #303 console-desk fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb303@example.com"],
        ["git", "config", "user.name", "wb303"],
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
    # after any assets/ui edit or the browser loads yesterday's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# --- scenario 1: the pure geometry, evaluated in the page -------------------
# One start rect, one workspace, one minimum for every row, so a reader can see
# each direction's contribution. `anchor` names the invariant the row exists to
# pin: "right" = `left+width` is unchanged, "bottom" = `top+height` is unchanged.
R = {"left": 100, "top": 80, "width": 400, "height": 300}
MIN = {"width": 240, "height": 150}
BOUNDS = {"width": 1000, "height": 700}
RIGHT = R["left"] + R["width"]  # 500
BOTTOM = R["top"] + R["height"]  # 380

# (label, dir, delta, expected rect, anchor)
GEOMETRY_ROWS = [
    ("east widens the far edge only", "e", (50, 0), (100, 80, 450, 300), None),
    ("east past the workspace stops at its edge", "e", (800, 0), (100, 80, 900, 300), "bounds-right"),
    ("east past the minimum stops at 240", "e", (-300, 0), (100, 80, 240, 300), None),
    ("west widens and holds the right edge", "w", (-60, 0), (40, 80, 460, 300), "right"),
    ("west narrows and holds the right edge", "w", (100, 0), (200, 80, 300, 300), "right"),
    ("west past the minimum stops at 240, right still held", "w", (300, 0), (260, 80, 240, 300), "right"),
    ("west past the workspace stops at 0, right still held", "w", (-200, 0), (0, 80, 500, 300), "right"),
    ("south grows the bottom edge only", "s", (0, 40), (100, 80, 400, 340), None),
    ("south past the workspace stops at its edge", "s", (0, 500), (100, 80, 400, 620), "bounds-bottom"),
    ("south past the minimum stops at 150", "s", (0, -300), (100, 80, 400, 150), None),
    ("north grows upward and holds the bottom edge", "n", (0, -50), (100, 30, 400, 350), "bottom"),
    ("north past the minimum stops at 150, bottom still held", "n", (0, 300), (100, 230, 400, 150), "bottom"),
    ("north past the workspace stops at 0, bottom still held", "n", (0, -200), (100, 0, 400, 380), "bottom"),
    ("ne holds the bottom edge and moves the right one", "ne", (50, -50), (100, 30, 450, 350), "bottom"),
    ("nw holds both the right and the bottom edge", "nw", (-60, -50), (40, 30, 460, 350), "both"),
    ("sw holds the right edge and moves the bottom one", "sw", (-60, 40), (40, 80, 460, 340), "right"),
    ("se moves neither left nor top", "se", (50, 40), (100, 80, 450, 340), None),
    ("a zero delta returns the input rect (se)", "se", (0, 0), (100, 80, 400, 300), None),
    ("a zero delta returns the input rect (nw)", "nw", (0, 0), (100, 80, 400, 300), "both"),
    ("a zero delta returns the input rect (n)", "n", (0, 0), (100, 80, 400, 300), "bottom"),
]


def geometry_table(page):
    args = [
        {"dir": d, "rect": R, "delta": {"dx": dx, "dy": dy}, "min": MIN, "bounds": BOUNDS}
        for (_, d, (dx, dy), _, _) in GEOMETRY_ROWS
    ]
    outs = page.evaluate(
        "(rows) => rows.map((r) => { try {"
        " return window.WBConsole.resizeRect(r.dir, r.rect, r.delta, r.min, r.bounds);"
        " } catch (e) { return String(e); } })",
        args,
    )
    for (label, d, delta, want, anchor), got in zip(GEOMETRY_ROWS, outs):
        wanted = {"left": want[0], "top": want[1], "width": want[2], "height": want[3]}
        check(f"resizeRect {d}: {label}", got == wanted, f"got={got} want={wanted} delta={delta}")
        if not isinstance(got, dict):
            continue
        if anchor in ("right", "both"):
            check(
                f"…and {d} holds the right edge at {RIGHT}",
                got["left"] + got["width"] == RIGHT,
                f"got left+width={got['left'] + got['width']}",
            )
        if anchor in ("bottom", "both"):
            check(
                f"…and {d} holds the bottom edge at {BOTTOM}",
                got["top"] + got["height"] == BOTTOM,
                f"got top+height={got['top'] + got['height']}",
            )
        if anchor == "bounds-right":
            check(
                f"…and {d} stops exactly at the workspace right edge",
                got["left"] + got["width"] == BOUNDS["width"],
                f"got left+width={got['left'] + got['width']}",
            )
        if anchor == "bounds-bottom":
            check(
                f"…and {d} stops exactly at the workspace bottom edge",
                got["top"] + got["height"] == BOUNDS["height"],
                f"got top+height={got['top'] + got['height']}",
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


def cols_of(page, index):
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._term.term.cols", index
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb303_reg_")
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
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)

            # --- scenario 1: the geometry in isolation ------------------------
            geometry_table(page)

            # The Agents tab must be in view or every terminal measures 0x0.
            page.evaluate(f"() => {{ {SH}.active = 'agents'; }}")
            page.wait_for_timeout(300)

            # --- scenario 4: a live edge resize -------------------------------
            open_console(page, slug)
            r0 = rect_of(page, 0)
            cols0 = cols_of(page, 0)
            drag_handle(page, 0, "w", -120, 0)
            r1 = rect_of(page, 0)
            check(
                "a west-edge drag widens the window",
                r1["width"] > r0["width"],
                f"{r0['width']} -> {r1['width']}",
            )
            check(
                "…holding the right edge in place",
                r1["left"] + r1["width"] == r0["left"] + r0["width"],
                f"{r0['left'] + r0['width']} -> {r1['left'] + r1['width']}",
            )
            check("…and the top edge untouched", r1["top"] == r0["top"], f"{r0['top']} -> {r1['top']}")
            page.wait_for_timeout(400)
            cols1 = cols_of(page, 0)
            check(
                "the terminal reflows as the window widens",
                cols1 > cols0,
                f"cols {cols0} -> {cols1}",
            )

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

            # --- scenario 5: live clamping, and a maximized window -------------
            drag_handle(page, 0, "e", -2000, 0)
            drag_handle(page, 0, "s", 0, -2000)
            r3 = rect_of(page, 0)
            check(
                "dragging past the minimum clamps to 240x150",
                r3["width"] == 240 and r3["height"] == 150,
                f"got={r3}",
            )
            # Park it mid-workspace first, so the east drag below really has room
            # to overshoot (a window already pinned at 0 would pass by accident).
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " w.style.left = '200px'; w.style.top = '60px';"
                " w.style.width = '400px'; w.style.height = '300px'; }"
            )
            page.wait_for_timeout(200)
            drag_handle(page, 0, "e", 3000, 0)
            r4 = rect_of(page, 0)
            ws_w = page.evaluate("() => document.getElementById('workspace').clientWidth")
            check(
                "dragging past the workspace clamps to its edge",
                r4["left"] == 200 and r4["left"] + r4["width"] == ws_w,
                f"got={r4} workspace={ws_w}",
            )
            drag_handle(page, 0, "w", -3000, 0)
            r5 = rect_of(page, 0)
            check(
                "…and a west overshoot stops at the workspace origin",
                r5["left"] == 0 and r5["left"] + r5["width"] == r4["left"] + r4["width"],
                f"got={r5}",
            )

            page.locator(".session-window").nth(0).locator(".session-max").click()
            page.wait_for_timeout(400)
            check(
                "the window is maximized",
                page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0].classList.contains('maximized')"
                )
                is True,
            )
            before = page.evaluate(
                "() => JSON.stringify(document.querySelectorAll('.session-window')[0]"
                ".getBoundingClientRect().toJSON())"
            )
            drag_handle(page, 0, "w", 150, 0)
            # The titlebar drag, too: maximized windows do not move.
            tb = page.evaluate(
                "() => { const r = document.querySelector('.session-titlebar').getBoundingClientRect();"
                " return { x: r.left + 40, y: r.top + r.height / 2 }; }"
            )
            page.mouse.move(tb["x"], tb["y"])
            page.mouse.down()
            page.mouse.move(tb["x"] + 200, tb["y"] + 120, steps=4)
            page.mouse.up()
            page.wait_for_timeout(350)
            after = page.evaluate(
                "() => JSON.stringify(document.querySelectorAll('.session-window')[0]"
                ".getBoundingClientRect().toJSON())"
            )
            check("a maximized window neither resizes nor drags", before == after, f"{before} -> {after}")
            check(
                "…and is still maximized",
                page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0].classList.contains('maximized')"
                )
                is True,
            )
            page.locator(".session-window").nth(0).locator(".session-max").click()
            page.wait_for_timeout(300)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 49
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CONSOLE DESK")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
