"""#337 browser acceptance: the plane is NAVIGABLE.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The two-window fixture desk
puts window B at `1600,1200` — entirely off-view at any viewport this test uses,
which is the whole point: since #336 nothing is clamped into the frame.

Scenario 1   dragging the BARE FLOOR pans the plane and moves no rect; every
             listener comes off on mouseup
Scenario 2   dragging a TITLEBAR still moves the window and dragging a resize
             handle still resizes — the floor is the pan surface, not the windows
Scenario 3   holding a window against the viewport edge auto-pans, and releasing
             leaves it where it was dropped in STAGE coordinates (persisted)
Scenario 4   the wheel scrolls the plane vertically, and shift-wheel horizontally
Scenario 5   the wheel inside a console body belongs to the TERMINAL: the plane
             does not move in either direction
Scenario 6   panning refits no terminal and loses no keystroke typed before or
             after it
Scenario 7   a window entirely off-view is brought into view in ONE click on the
             Go-to picker, at exactly `bringIntoView`'s offsets, and is
             interactive afterwards
Scenario 8   the daemon's desk agrees with the screen and holds no negative origin

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/337-plane-navigation-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_pan_337.py   (exit 0 = all pass)
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

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7432
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The fixture desk. B sits at 1600,1200 — off-view at 1400x900, which is what
# scenario 7 has to reach. bbox = 2200 x 1580, so the stage measures
# 2200+200 x 1580+200 at every viewport this test uses.
FIX_A = {"left": 40, "top": 40, "width": 600, "height": 380}
FIX_B = {"left": 1600, "top": 1200, "width": 600, "height": 380}
STAGE_W = 2400
STAGE_H = 1780

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
    empty = tempfile.mkdtemp(prefix="wb337_empty_")
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
    d = tempfile.mkdtemp(prefix="wb337_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #337 navigation fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb337@example.com"],
        ["git", "config", "user.name", "wb337"],
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
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def write_desk(daemon_dir, slug):
    rows = []
    for wid, r, ts in (("w-fixture-a", FIX_A, 100), ("w-fixture-b", FIX_B, 101)):
        rect = (
            "rect = { left = %(left)s, top = %(top)s,"
            " width = %(width)s, height = %(height)s }" % r
        )
        rows.append(
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            "max = false\n"
            f"ts = {ts}\n"
            f"{rect}\n"
        )
    Path(daemon_dir, "desk.toml").write_text("\n".join(rows), encoding="utf-8")


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def http(method, path, body=None):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def rects(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) =>"
        " ({ left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight }))"
    )


def scroll(page):
    return page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " return { left: ws.scrollLeft, top: ws.scrollTop }; }"
    )


def reset_scroll(page):
    page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " ws.scrollLeft = 0; ws.scrollTop = 0; }"
    )
    page.wait_for_timeout(150)


def ws_rect(page):
    return page.evaluate(
        "() => { const r = document.getElementById('workspace').getBoundingClientRect();"
        " return { left: r.left, top: r.top, right: r.right, bottom: r.bottom,"
        "   width: r.width, height: r.height }; }"
    )


def settle(page, want=2):
    """Wait for the restored windows to be REAL boxes.

    KNOWLEDGE: an `x-show` flip is not visible to the next evaluate, and a
    still-hidden box measures 0x0 — which passes a geometry assertion vacuously.
    """
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=20000,
    )
    page.wait_for_timeout(600)


def desk_page(ctx, viewport):
    page = ctx.new_page()
    page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(1800)
    return page


def restore_fixture(page):
    """Put both windows back on their fixture rects, at scroll 0,0."""
    page.evaluate(
        "(fix) => { const w = document.querySelectorAll('.session-window');"
        " fix.forEach((r, i) => { if (!w[i]) return;"
        "   w[i].style.left = r.left + 'px'; w[i].style.top = r.top + 'px';"
        "   w[i].style.width = r.width + 'px'; w[i].style.height = r.height + 'px'; });"
        " window.WBConsole.refitAll();"
        " const ws = document.getElementById('workspace');"
        " ws.scrollLeft = 0; ws.scrollTop = 0; }",
        [FIX_A, FIX_B],
    )
    page.wait_for_timeout(400)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb337_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900})
            settle(page)

            # ===== scenario 1: the bare floor is the PAN surface ===============
            stage = page.evaluate(
                "() => { const st = document.getElementById('stage');"
                " return { w: st.offsetWidth, h: st.offsetHeight }; }"
            )
            check(
                f"the stage measures the fixture bbox + margin ({STAGE_W}x{STAGE_H})",
                (stage["w"], stage["h"]) == (STAGE_W, STAGE_H),
                f"got={stage['w']}x{stage['h']}",
            )
            before = rects(page)
            check(
                "the fixture desk restored on its own rects",
                before == [FIX_A, FIX_B],
                f"want={[FIX_A, FIX_B]} got={before}",
            )

            wsr = ws_rect(page)
            # 60% across and 8px up from the bottom: below every window's box at
            # scroll 0,0, and far enough from the left edge that the -200px drag
            # stays inside the page.
            floor = {"x": wsr["left"] + wsr["width"] * 0.6, "y": wsr["bottom"] - 8}
            hit = page.evaluate(
                "(pt) => { const el = document.elementFromPoint(pt.x, pt.y);"
                " return { id: el && el.id, cls: el && el.className }; }",
                floor,
            )
            check(
                "the drag point hit-tests as the BARE FLOOR",
                hit["id"] == "stage",
                f"elementFromPoint gave id={hit['id']} class={hit['cls']}",
            )

            page.mouse.move(floor["x"], floor["y"])
            page.mouse.down()
            page.mouse.move(floor["x"] - 70, floor["y"] - 50, steps=5)
            page.mouse.move(floor["x"] - 140, floor["y"] - 100, steps=5)
            page.mouse.move(floor["x"] - 200, floor["y"] - 150, steps=5)
            mid = page.evaluate(
                "() => document.getElementById('stage').classList.contains('panning')"
            )
            page.mouse.up()
            page.wait_for_timeout(300)

            check("the stage carries `.panning` while the drag is held", mid, "")
            panned = scroll(page)
            check(
                "a floor drag of (-200,-150) pans the plane on X",
                panned["left"] == 200,
                f"scrollLeft={panned['left']}",
            )
            check(
                "…and on Y",
                panned["top"] == 150,
                f"scrollTop={panned['top']}",
            )
            check(
                "…moving NO window rect — the floor pans the view, not the layout",
                rects(page) == before,
                f"want={before} got={rects(page)}",
            )
            check(
                "…and drops `.panning` on mouseup",
                not page.evaluate(
                    "() => document.getElementById('stage').classList.contains('panning')"
                ),
                "",
            )
            # The listeners really came off: a further move with no button held
            # must not pan.
            page.mouse.move(floor["x"] - 300, floor["y"] - 250, steps=4)
            page.wait_for_timeout(200)
            after_up = scroll(page)
            check(
                "…and every listener came off, so a button-less move pans nothing",
                after_up == panned,
                f"was={panned} now={after_up}",
            )
            reset_scroll(page)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 40
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE PLANE IS NAVIGABLE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
