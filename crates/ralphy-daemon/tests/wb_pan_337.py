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


def poll(page, read, ok, timeout=6.0, step=100):
    """Re-read until `ok` holds, then return the value; else the last read.

    Not a weakened assertion — the same oracle, with a bounded wait instead of
    one fixed sleep. A chained scroll is ANIMATED and a desk write is debounced
    then flushed over HTTP: both are "eventually", and a single sleep is a
    coin-flip on how long eventually took this time.
    """
    deadline = time.time() + timeout
    value = read()
    while not ok(value) and time.time() < deadline:
        page.wait_for_timeout(step)
        value = read()
    return value


def desk_rects():
    return {r["id"]: r["rect"] for r in json.loads(http("GET", "api/desk")[1])}


def centre(page, index, sel=None):
    """The client-space centre of a window, or of one part of its chrome."""
    return page.evaluate(
        "([i, s]) => { const w = document.querySelectorAll('.session-window')[i];"
        " const h = s ? w.querySelector(s) : w;"
        " const r = h.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        [index, sel],
    )


def visible_body_point(page, index):
    """The centre of the VISIBLE part of a window's body.

    The plain box centre is not enough: a scrolled plane can leave half a window
    outside the viewport, and a wheel dispatched there lands on the tab body.
    """
    return page.evaluate(
        "(i) => { const ws = document.getElementById('workspace').getBoundingClientRect();"
        " const b = document.querySelectorAll('.session-window')[i]"
        "   .querySelector('.session-body').getBoundingClientRect();"
        " const l = Math.max(ws.left, b.left), r = Math.min(ws.right, b.right);"
        " const t = Math.max(ws.top, b.top), o = Math.min(ws.bottom, b.bottom);"
        " return { x: (l + r) / 2, y: (t + o) / 2, w: r - l, h: o - t }; }",
        index,
    )


def drag_from(page, point, dx, dy, hold=0):
    page.mouse.move(point["x"], point["y"])
    page.mouse.down()
    for f in (0.34, 0.67, 1.0):
        page.mouse.move(point["x"] + dx * f, point["y"] + dy * f, steps=5)
    if hold:
        page.wait_for_timeout(hold)
    page.mouse.up()
    page.wait_for_timeout(400)


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

            # ===== scenario 2: a window still drags, a handle still resizes ====
            # The floor is the pan surface — the windows are NOT. Same gesture,
            # different target, opposite outcome.
            drag_from(page, centre(page, 0, ".session-titlebar"), 120, 90)
            moved = rects(page)[0]
            sc = scroll(page)
            check(
                "a TITLEBAR drag still moves the window, not the view",
                (moved["left"], moved["top"]) == (160, 130),
                f"want left=160 top=130 got={moved}",
            )
            check(
                "…leaving the plane exactly where it was",
                (sc["left"], sc["top"]) == (0, 0),
                f"got={sc}",
            )
            drag_from(page, centre(page, 0, ".session-handle.h-e"), 80, 0)
            resized = rects(page)[0]
            check(
                "a RESIZE handle still resizes",
                resized["width"] == 680,
                f"want width=680 got={resized}",
            )
            check(
                "…anchoring the opposite edge instead of sliding the window",
                resized["left"] == 160,
                f"want left=160 got={resized}",
            )
            restore_fixture(page)
            check(
                "the fixture rects are restored for the next scenario",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )

            # ===== scenario 3: auto-pan while dragging a window ================
            # Holding a window against the viewport edge scrolls the plane under
            # it — and the DROP position must be right in STAGE coordinates, so
            # the window has to keep following the cursor while the plane moves.
            wsr = ws_rect(page)
            press = centre(page, 0, ".session-titlebar")
            off_x = page.evaluate(
                "(px) => px - document.querySelectorAll('.session-window')[0]"
                ".getBoundingClientRect().left",
                press["x"],
            )
            hold_x = wsr["right"] - 4
            page.mouse.move(press["x"], press["y"])
            page.mouse.down()
            page.mouse.move((press["x"] + hold_x) / 2, press["y"], steps=6)
            page.mouse.move(hold_x, press["y"], steps=6)
            # Jitter inside the band: rAF only runs while the page is alive, and
            # a single move would prove one tick, not a loop.
            for i in range(12):
                page.mouse.move(hold_x - (i % 2), press["y"])
                page.wait_for_timeout(60)
            held = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " const w = document.querySelectorAll('.session-window')[0];"
                " return { scrollLeft: ws.scrollLeft, left: w.offsetLeft }; }"
            )
            pointer_x = hold_x - (11 % 2)
            want_left = round(held["scrollLeft"] + pointer_x - wsr["left"] - off_x)
            check(
                "holding a window at the viewport edge PANS the plane",
                held["scrollLeft"] > 0,
                f"scrollLeft={held['scrollLeft']}",
            )
            check(
                "…while the window keeps following the cursor in stage coordinates",
                held["left"] == want_left,
                f"want={want_left} got={held['left']} (scrollLeft={held['scrollLeft']}"
                f" pointerX={pointer_x} wsLeft={wsr['left']} offX={off_x})",
            )
            page.mouse.up()
            page.wait_for_timeout(400)
            dropped = page.evaluate(
                "() => document.querySelectorAll('.session-window')[0].offsetLeft"
            )
            check(
                "…and releasing leaves it exactly where it was dropped",
                dropped == held["left"],
                f"pre-mouseup={held['left']} post-mouseup={dropped}",
            )
            settled_scroll = scroll(page)["left"]
            page.wait_for_timeout(500)
            check(
                "…with the rAF loop cancelled on mouseup, so the plane stops moving",
                scroll(page)["left"] == settled_scroll,
                f"scrollLeft {settled_scroll} -> {scroll(page)['left']}",
            )
            persisted = poll(
                page,
                desk_rects,
                lambda d: (d.get("w-fixture-a") or {}).get("left") == float(dropped),
            )
            check(
                "…and the daemon persisted that same stage coordinate",
                (persisted.get("w-fixture-a") or {}).get("left") == float(dropped),
                f"screen={dropped} desk={persisted.get('w-fixture-a')}",
            )
            restore_fixture(page)

            # ===== scenario 4: the wheel reaches both axes =====================
            wsr = ws_rect(page)
            floor = {"x": wsr["left"] + wsr["width"] * 0.6, "y": wsr["bottom"] - 8}
            page.mouse.move(floor["x"], floor["y"])
            page.mouse.wheel(0, 300)
            page.wait_for_timeout(400)
            down = scroll(page)
            check(
                "a wheel over the floor scrolls the plane vertically",
                down["top"] > 0,
                f"scrollTop={down['top']}",
            )
            check(
                "…and only vertically",
                down["left"] == 0,
                f"scrollLeft={down['left']}",
            )
            reset_scroll(page)
            page.mouse.move(floor["x"], floor["y"])
            page.keyboard.down("Shift")
            page.mouse.wheel(0, 300)
            page.keyboard.up("Shift")
            page.wait_for_timeout(400)
            side = scroll(page)
            # Whether the platform converts shift-wheel itself or the handler
            # does it, the OUTCOME is the same — which is what "consistent with
            # the platform" has to mean. Without either, the plane scrolls DOWN.
            check(
                "shift + wheel reaches the horizontal axis",
                side["left"] > 0,
                f"scrollLeft={side['left']}",
            )
            check(
                "…without also scrolling down",
                side["top"] == 0,
                f"scrollTop={side['top']}",
            )
            reset_scroll(page)

            # ===== scenario 5: the wheel inside a console is the TERMINAL's ====
            page.evaluate(
                "() => { document.querySelectorAll('.session-window')[0]"
                "._term.term.write('wb337 line\\r\\n'.repeat(400)); }"
            )
            # The terminal's scroll position is `buffer.active.viewportY`, NOT
            # `.xterm-viewport.scrollTop`: this xterm ships a monaco-style
            # `.xterm-scrollable-element` whose viewport never scrolls natively
            # (`scrollHeight === clientHeight` with 400 lines of scrollback in
            # the buffer), so a scrollTop oracle would read 0 forever and pass
            # both directions vacuously.
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window')[0]"
                "  ._term.term.buffer.active.baseY > 20",
                timeout=15000,
            )
            page.evaluate(
                "() => { document.querySelectorAll('.session-window')[0]._term.term.scrollToTop();"
                " document.getElementById('workspace').scrollTop = 200; }"
            )
            page.wait_for_timeout(400)
            term_view = (
                "() => { const t = document.querySelectorAll('.session-window')[0]._term.term;"
                " return { plane: document.getElementById('workspace').scrollTop,"
                "   term: t.buffer.active.viewportY, base: t.buffer.active.baseY }; }"
            )
            body = visible_body_point(page, 0)
            check(
                "the console body is really on screen to wheel over",
                body["w"] > 40 and body["h"] > 40,
                f"visible body {body['w']}x{body['h']}",
            )
            check(
                "…with real scrollback behind it, so the wheel has somewhere to go",
                page.evaluate(term_view)["base"] > 20,
                f"buffer baseY={page.evaluate(term_view)['base']}",
            )
            page.mouse.move(body["x"], body["y"])
            page.mouse.wheel(0, -200)
            page.wait_for_timeout(400)
            up_in_term = page.evaluate(term_view)
            check(
                "a wheel UP at the terminal's scroll limit does NOT chain out to the plane",
                up_in_term["plane"] == 200,
                f"#workspace.scrollTop={up_in_term['plane']} (want 200)",
            )
            check(
                "…and leaves the terminal at the top of its own scrollback",
                up_in_term["term"] == 0,
                f"terminal viewportY={up_in_term['term']}",
            )
            page.mouse.wheel(0, 200)
            page.wait_for_timeout(400)
            down_in_term = page.evaluate(term_view)
            check(
                "a wheel DOWN over a console body scrolls the TERMINAL",
                down_in_term["term"] > 0,
                f"terminal viewportY={down_in_term['term']}",
            )
            check(
                "…and still not the plane",
                down_in_term["plane"] == 200,
                f"#workspace.scrollTop={down_in_term['plane']} (want 200)",
            )
            # NEGATIVE CONTROL for the two checks above: without
            # `overscroll-behavior: contain` the wheel-up at the terminal's top
            # DOES chain out and pans the plane (measured: 200 -> 0). A guard
            # that has never rejected an input is not a guard.
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " w._term.term.scrollToTop();"
                " w.style.overscrollBehavior = 'auto';"
                " w.querySelectorAll('.xterm-viewport').forEach((v) =>"
                "   { v.style.overscrollBehavior = 'auto'; });"
                " document.getElementById('workspace').scrollTop = 200; }"
            )
            page.wait_for_timeout(400)
            page.mouse.wheel(0, -200)
            chained = poll(
                page,
                lambda: page.evaluate(term_view),
                lambda v: v["plane"] < 200,
            )
            check(
                "…and `overscroll-behavior: contain` is what holds it in —"
                " forced to `auto`, the same wheel DOES pan the plane",
                chained["plane"] < 200,
                f"#workspace.scrollTop={chained['plane']} (was 200 before the wheel)",
            )
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " w.style.overscrollBehavior = '';"
                " w.querySelectorAll('.xterm-viewport').forEach((v) =>"
                "   { v.style.overscrollBehavior = ''; }); }"
            )
            reset_scroll(page)

            # ===== scenario 6: no refit storm, no lost keystroke ===============
            wrapped = page.evaluate(
                "() => { window.__fits = 0; window.__sent = [];"
                " const wins = [...document.querySelectorAll('.session-window')];"
                " for (const w of wins) { const f = w._term.fit;"
                "   const orig = f.fit.bind(f);"
                "   f.fit = function () { window.__fits += 1; return orig(); }; }"
                " const sock = wins[0]._term.ws; window.__sock = sock;"
                " const send = sock.send.bind(sock);"
                " sock.send = function (d) { try { const u = new Uint8Array(d);"
                "     if (u[0] === 1) window.__sent.push(new TextDecoder().decode(u.subarray(9)));"
                "   } catch (e) {} return send(d); };"
                " return { dims: wins.map((w) => ({ rows: w._term.term.rows, cols: w._term.term.cols })),"
                "   socket: sock.readyState }; }"
            )
            check(
                "both terminals are live sockets to instrument",
                wrapped["socket"] == 1 and len(wrapped["dims"]) == 2,
                f"got={wrapped}",
            )
            bp_a = visible_body_point(page, 0)
            page.mouse.click(bp_a["x"], bp_a["y"])
            page.wait_for_timeout(200)
            page.keyboard.type("before")
            page.wait_for_timeout(400)
            check(
                "a keystroke before the pan reached the session's socket",
                "before" in "".join(page.evaluate("() => window.__sent")),
                f"sent={page.evaluate('() => window.__sent')}",
            )
            wsr = ws_rect(page)
            drag_from(
                page,
                {"x": wsr["left"] + wsr["width"] * 0.6, "y": wsr["bottom"] - 8},
                -200,
                -150,
            )
            after_pan = page.evaluate(
                "() => ({ fits: window.__fits,"
                " dims: [...document.querySelectorAll('.session-window')]"
                "   .map((w) => ({ rows: w._term.term.rows, cols: w._term.term.cols })),"
                " sameSocket: document.querySelectorAll('.session-window')[0]._term.ws === window.__sock })"
            )
            check(
                "panning refits NO terminal",
                after_pan["fits"] == 0,
                f"fit.fit() calls during the pan = {after_pan['fits']}",
            )
            check(
                "…and leaves every terminal's rows/cols untouched",
                after_pan["dims"] == wrapped["dims"],
                f"before={wrapped['dims']} after={after_pan['dims']}",
            )
            page.keyboard.type("after")
            page.wait_for_timeout(400)
            sent = "".join(page.evaluate("() => window.__sent"))
            check(
                "a keystroke after the pan reaches the SAME socket, in order",
                0 <= sent.index("before") < sent.index("after"),
                f"sent={sent!r}",
            )
            check(
                "…and that socket never silently reconnected under the wrapper",
                after_pan["sameSocket"],
                "a reconnect would have voided the spy and made the check vacuous",
            )
            reset_scroll(page)
            restore_fixture(page)

            # ===== scenario 7: ONE action reaches an off-view window ===========
            wsr = ws_rect(page)
            far = page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[1];"
                " const r = w.getBoundingClientRect();"
                " return { left: r.left, top: r.top,"
                "   bring: typeof window.WBConsole.bringIntoView }; }"
            )
            check(
                "the far window is entirely OUT of the frame at 0,0",
                far["left"] >= wsr["right"],
                f"window left={far['left']} viewport right={wsr['right']}",
            )
            check(
                "`bringIntoView` is exported as a pure function",
                far["bring"] == "function",
                f"typeof = {far['bring']}",
            )
            page.click(".canvas-tools button:has-text('Go to')")
            page.wait_for_function(
                "() => [...document.querySelectorAll('.window-menu .window-item')]"
                "  .filter((e) => e.offsetParent !== null).length === 2",
                timeout=8000,
            )
            check("the Go-to picker lists both open consoles", True, "")
            page.locator(".window-menu .window-item:visible").nth(1).click()
            page.wait_for_timeout(500)
            revealed = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " const st = document.getElementById('stage');"
                " const w = document.querySelectorAll('.session-window')[1];"
                " const got = { left: ws.scrollLeft, top: ws.scrollTop };"
                " const want = window.WBConsole.bringIntoView("
                "   { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight },"
                "   { width: ws.clientWidth, height: ws.clientHeight },"
                "   { width: st.offsetWidth, height: st.offsetHeight });"
                # Write the pure answer back and re-read: both sides then go
                # through the browser's own scroll snapping, so this compares the
                # OFFSETS themselves and not two float spellings of them.
                " ws.scrollLeft = want.left; ws.scrollTop = want.top;"
                " const echo = { left: ws.scrollLeft, top: ws.scrollTop };"
                " const r = w.getBoundingClientRect(); const b = ws.getBoundingClientRect();"
                " return { got, want, echo,"
                "   inside: r.left >= b.left && r.right <= b.right"
                "     && r.top >= b.top && r.bottom <= b.bottom }; }"
            )
            check(
                "ONE click brings the far window fully inside the viewport",
                revealed["inside"],
                f"scroll={revealed['got']}",
            )
            check(
                "…at exactly the offsets `bringIntoView` computes",
                revealed["got"] == revealed["echo"],
                f"reveal={revealed['got']} bringIntoView={revealed['want']} -> {revealed['echo']}",
            )
            page.locator(".session-window").nth(1).locator(".session-titlebar").click()
            page.wait_for_timeout(300)
            focused = page.evaluate(
                "() => document.querySelectorAll('.session-window')[1].classList.contains('focused')"
            )
            check("…and it is focusable once it is on screen", focused, "")
            page.evaluate(
                "() => { window.__sentB = [];"
                " const sock = document.querySelectorAll('.session-window')[1]._term.ws;"
                " const send = sock.send.bind(sock);"
                " sock.send = function (d) { try { const u = new Uint8Array(d);"
                "     if (u[0] === 1) window.__sentB.push(new TextDecoder().decode(u.subarray(9)));"
                "   } catch (e) {} return send(d); }; }"
            )
            bp = visible_body_point(page, 1)
            page.mouse.click(bp["x"], bp["y"])
            page.wait_for_timeout(200)
            page.keyboard.type("live")
            page.wait_for_timeout(400)
            check(
                "…and INTERACTIVE afterwards — typing reaches its own session",
                "live" in "".join(page.evaluate("() => window.__sentB")),
                f"sent={page.evaluate('() => window.__sentB')}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "337-plane-navigation-2026-07-27.png"))

            # ===== scenario 8: the desk agrees with the screen =================
            # A real gesture first: `restore_fixture` moves windows by writing
            # inline styles, which the product never sees — comparing the desk to
            # a screen the test itself rearranged behind its back would compare
            # the fixture against nothing.
            reset_scroll(page)
            drag_from(page, centre(page, 0, ".session-titlebar"), 10, 10)
            desk = poll(
                page,
                desk_rects,
                lambda d: (d.get("w-fixture-a") or {}).get("left") == 50.0,
            )
            screen = page.evaluate(
                "() => Object.fromEntries([...document.querySelectorAll('.session-window')]"
                "  .map((w) => [w._deskId, { left: w.offsetLeft, top: w.offsetTop,"
                "     width: w.offsetWidth, height: w.offsetHeight }]))"
            )
            check(
                "the daemon's desk agrees with the rects on screen",
                all(
                    desk.get(k) == {m: float(v) for m, v in r.items()} for k, r in screen.items()
                ),
                f"desk={desk} screen={screen}",
            )
            check(
                "…and no window ended up past the pinned origin",
                all(r["left"] >= 0 and r["top"] >= 0 for r in desk.values()),
                f"desk={desk}",
            )

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
