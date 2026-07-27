"""#343 browser acceptance: THE FENCE LIST IS THE MAP.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7438, so this can run
beside #340/#341/#342's suites without any daemon stealing another's port.

The fixture `desk.toml` is written BEFORE the daemon starts: three fences in
different parts of the plane, and the window records that make alpha hold two
members and beta one. GAMMA sits far enough out (left 2200) that it starts
entirely OFF-view in a 1400x900 viewport — without that, the jump's oracle is
vacuous. Every member box clears its fence's head band AND its 14 px SE grip:
both handles sit at `z-index: 1`, BELOW every window, so a member parked on one
makes the fence ungrabbable from a script (#341's covered-handle trap).

Scenario 1  the toolbar list reads every fence's name, the repos its members
            belong to, and its console count — including the EMPTY one
Scenario 2  clicking a row slides the viewport onto that fence: gamma is proven
            off-view first, is fully in view after, is the focused fence, and
            NOT ONE rect changed (`/api/desk` byte-identical)
Scenario 3  a console opened while a fence is focused is BORN inside it, the
            list's count follows, and the record persists there across a reload
Scenario 4  with no fence focused, a console lands on the plain cascade
Scenario 5  the list is live without a reload: create, rename (the jump is what
            puts gamma's input in reach at all), a membership change by dragging
            a window IN, a fence MOVE whose row survives intact — #341's
            anchoring carries a fence's members with it, so a moved fence keeps
            its count by design — a membership change by dragging the window
            OUT, and a removal
Scenario 6  two browser contexts read the same fence names and counts while each
            keeps its own viewport position — this is where the evidence
            screenshot is taken

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/343-the-fence-list-is-the-map-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_fence_343.py   (exit 0 = all pass)
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

PORT = 7438
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "343-the-fence-list-is-the-map-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The fixture geometry, in stage coordinates. Gamma is the JUMP target: it must
# start beyond the viewport's right edge, which is what makes scenario 2's
# "fully in view afterwards" a real assertion rather than a tautology.
FENCE_A = {"left": 40, "top": 40, "width": 600, "height": 460}
FENCE_B = {"left": 700, "top": 40, "width": 320, "height": 300}
FENCE_G = {"left": 2200, "top": 900, "width": 400, "height": 320}

# Two members in alpha, one in beta. Boxes are 260x160; every one starts below
# the head band (top >= 100) and stops short of the SE grip.
MEM_A1 = {"left": 60, "top": 100, "width": 260, "height": 160}
MEM_A2 = {"left": 340, "top": 100, "width": 260, "height": 160}
MEM_B1 = {"left": 720, "top": 110, "width": 260, "height": 160}

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
    empty = tempfile.mkdtemp(prefix="wb343_empty_")
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


def make_fixture_repo(tag):
    d = tempfile.mkdtemp(prefix=f"wb343_{tag}_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture {tag}\n\nThe #343 fence-list fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb343@example.com"],
        ["git", "config", "user.name", "wb343"],
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


def rect_toml(r):
    return "rect = { left = %(left)s, top = %(top)s, width = %(width)s, height = %(height)s }" % r


def window_toml(wid, repo, rect, ts):
    return (
        "[[windows]]\n"
        f'id = "{wid}"\n'
        f'repo = "{repo}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def fence_toml(fid, name, rect, ts):
    return (
        "[[fences]]\n"
        f'id = "{fid}"\n'
        f'name = "{name}"\n'
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def write_fixture_desk(daemon_dir, slug_a, slug_b):
    """Three fences and the three members that populate two of them.

    `kind = "agent"` restores as a PLACEHOLDER: full chrome, deterministic
    geometry, no PTY — the counts and the repos readout need a window record,
    not a terminal.
    """
    Path(daemon_dir, "desk.toml").write_text(
        window_toml("w-a1", slug_a, MEM_A1, 100)
        + window_toml("w-a2", slug_a, MEM_A2, 101)
        + window_toml("w-b1", slug_b, MEM_B1, 102)
        + fence_toml("f-alpha", "alpha", FENCE_A, 110)
        + fence_toml("f-beta", "beta", FENCE_B, 111)
        + fence_toml("f-gamma", "gamma", FENCE_G, 112),
        encoding="utf-8",
    )


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


def quiet(desk_file, still=1.6, timeout=15):
    """Block until `desk.toml` has not changed for `still` seconds.

    A fixed sleep is the wrong synchroniser for "nothing was written": under
    load the shell's 250 ms flush can land AFTER the sleep (#341).
    """
    deadline = time.time() + timeout
    last = None
    since = time.time()
    while time.time() < deadline:
        try:
            now = (desk_file.stat().st_mtime_ns, desk_file.stat().st_size)
        except OSError:
            now = None
        if now != last:
            last = now
            since = time.time()
        elif time.time() - since >= still:
            return
        time.sleep(0.15)


def settle_windows(page, want):
    """Wait for the restored windows to be REAL boxes.

    KNOWLEDGE: an `x-show` flip is not visible to the next evaluate, and a
    still-hidden box measures 0x0 — which passes a geometry assertion vacuously.
    """
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=25000,
    )
    page.wait_for_timeout(500)


def desk_page(ctx, fences=3, windows=3):
    page = ctx.new_page()
    page.set_viewport_size(dict(VIEW))
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    # `activate` and not a raw `active =` write: only the former reaches
    # `refitAll()`, the path that re-applies a stored offset after `display:none`
    # threw the scroll position away (KNOWLEDGE, #339).
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_timeout(1800)
    settle_windows(page, windows)
    page.wait_for_function(
        "(n) => document.querySelectorAll('.fence').length === n", arg=fences, timeout=15000
    )
    return page


def open_fence_list(page):
    """Open the toolbar's fence picker and return its rows, in DOM order.

    The wait is gated on `offsetParent`, never on text: an Alpine `x-show` flip
    is not visible to the next evaluate, and a still-hidden row measures 0x0.
    """
    close_menus(page)
    page.locator("button[title='jump to a fence']").click()
    page.wait_for_function(
        "() => { const m = document.querySelector('.fence-menu');"
        " return m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=8000,
    )
    return page.evaluate(
        "() => [...document.querySelectorAll('.fence-item')].map((r) => ({"
        "  name: r.querySelector('.row-name').textContent.trim(),"
        "  repos: r.querySelector('.row-repos').offsetParent === null"
        "    ? '' : r.querySelector('.row-repos').textContent.trim(),"
        "  count: r.querySelector('.row-count').textContent.trim() }))"
    )


def close_menus(page):
    page.evaluate(f"() => {{ const s = {SH}; s.fenceMenu = false; s.agentMenu = false; s.windowMenu = false; }}")
    page.wait_for_timeout(150)


def click_fence_row(page, name):
    """Click the row whose name matches — a REAL click, so the jump runs through
    Alpine's own handler rather than through the module's exported function.

    Opens the picker itself: the rows are a SNAPSHOT taken when the menu opens,
    so a click must always follow a fresh open, never a stale one.
    """
    open_fence_list(page)
    page.locator(".fence-item:visible", has=page.locator(f".row-name:text-is('{name}')")).click()
    page.wait_for_timeout(600)


def view_state(page, fence_id):
    return page.evaluate(
        "(id) => { const ws = document.getElementById('workspace');"
        " const f = document.querySelector(`[data-fence-id='${id}']`);"
        " return { scrollLeft: ws.scrollLeft, scrollTop: ws.scrollTop,"
        "   clientWidth: ws.clientWidth, clientHeight: ws.clientHeight,"
        "   left: f.offsetLeft, top: f.offsetTop,"
        "   right: f.offsetLeft + f.offsetWidth, bottom: f.offsetTop + f.offsetHeight,"
        "   focused: window.WBConsole.focusedFence() }; }",
        fence_id,
    )


def boxes(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) => ({"
        "  id: w._deskId, repo: w._deskRepo,"
        "  box: { left: w.offsetLeft, top: w.offsetTop,"
        "    width: w.offsetWidth, height: w.offsetHeight } }))"
    )


def by_id(rows):
    return {r["id"]: r for r in rows}


def open_plain_console(page):
    """Open a console through the REAL New-console control, and return its id.

    A real click, not `WBConsole.open(...)`: the birth path is the thing under
    test, and the sidebar route is how an operator reaches it.
    """
    before = page.locator(".session-window").count()
    close_menus(page)
    page.locator("button:has-text('New console')").click()
    page.locator(".dropdown-item.is-console:visible").click()
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=15000
    )
    page.locator(".session-window").nth(before).locator(".xterm").wait_for(timeout=25000)
    page.wait_for_timeout(600)
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._deskId", before
    )


def unscroll(page):
    """Pin the plane at 0,0 so a stage rect and a client rect differ only by the
    workspace's own origin."""
    page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " ws.scrollLeft = 0; ws.scrollTop = 0; }"
    )
    page.wait_for_timeout(200)


def stage_origin(page):
    return page.evaluate(
        "() => { const r = document.getElementById('stage').getBoundingClientRect();"
        " return { x: r.left, y: r.top }; }"
    )


def press_floor(page, x, y):
    """Press the BARE floor at a stage-coordinate point.

    `onFloorDown` hit-tests on element IDENTITY, so this only reaches the floor
    when no window covers the point — the caller picks one that none does.
    """
    origin = stage_origin(page)
    page.mouse.move(origin["x"] + x, origin["y"] + y)
    page.mouse.down()
    page.mouse.up()
    page.wait_for_timeout(300)


def drag(page, start, dx, dy):
    page.mouse.move(start["x"], start["y"])
    page.mouse.down()
    page.mouse.move(start["x"] + dx / 3, start["y"] + dy / 3, steps=5)
    page.mouse.move(start["x"] + dx * 2 / 3, start["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(start["x"] + dx, start["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(500)


def client_centre(page, selector):
    """The CLIENT centre of an element, measured NOW.

    Re-measured before every gesture on purpose: a `.fence-grab` travels with
    its fence, and #341 measured that it lands under whatever window happens to
    be there — a cached handle box presses the window instead, and the gesture
    silently never starts.
    """
    return page.evaluate(
        "(sel) => { const el = document.querySelector(sel);"
        " if (!el) return null;"
        " const r = el.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2,"
        "   w: r.width, h: r.height }; }",
        selector,
    )


def fence_rect(page, fence_id):
    return page.evaluate(
        "(id) => { const f = document.querySelector(`[data-fence-id='${id}']`);"
        " if (!f) return null;"
        " return { left: f.offsetLeft, top: f.offsetTop,"
        "   width: f.offsetWidth, height: f.offsetHeight,"
        "   head: f.querySelector('.fence-head').offsetHeight }; }",
        fence_id,
    )


def drag_window_to(page, wid, cx, cy):
    """Drag a window by its titlebar so its CENTRE lands on a stage point."""
    here = page.evaluate(
        "(id) => { const w = [...document.querySelectorAll('.session-window')]"
        "   .find((x) => x._deskId === id);"
        " return { x: w.offsetLeft + w.offsetWidth / 2, y: w.offsetTop + w.offsetHeight / 2 }; }",
        wid,
    )
    bar = page.evaluate(
        "(id) => { const w = [...document.querySelectorAll('.session-window')]"
        "   .find((x) => x._deskId === id);"
        " const r = w.querySelector('.session-titlebar').getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        wid,
    )
    drag(page, bar, cx - here["x"], cy - here["y"])


def row_named(rows, name):
    for r in rows:
        if r["name"] == name:
            return r
    return None


def free_point(page):
    """A visible stage point held by NO fence, measured now.

    Computed rather than hardcoded: scenario 5 moves a fence around, so a
    literal "outside" point can silently end up inside one and make a
    membership assertion fail against correct code.
    """
    rects = page.evaluate(
        "() => [...document.querySelectorAll('.fence')].map((f) => ({"
        "  left: f.offsetLeft, top: f.offsetTop,"
        "  width: f.offsetWidth, height: f.offsetHeight }))"
    )
    view = page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        " return { w: ws.clientWidth, h: ws.clientHeight,"
        "   sl: ws.scrollLeft, st: ws.scrollTop }; }"
    )
    for y in range(int(view["st"]) + 80, int(view["st"] + view["h"]) - 80, 40):
        for x in range(int(view["sl"]) + 80, int(view["sl"] + view["w"]) - 80, 40):
            if not any(
                r["left"] <= x < r["left"] + r["width"] and r["top"] <= y < r["top"] + r["height"]
                for r in rects
            ):
                return (x, y)
    raise AssertionError("no free floor point in the viewport")


def desk_record(wid):
    for w in json.loads(http("GET", "api/desk")[1]).get("windows", []):
        if w.get("id") == wid:
            return w
    return None


def inside(r, fence, slack=0.0):
    return (
        r["left"] >= fence["left"] - slack
        and r["top"] >= fence["top"] - slack
        and r["left"] + r["width"] <= fence["left"] + fence["width"] + slack
        and r["top"] + r["height"] <= fence["top"] + fence["height"] + slack
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb343_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    slug_a = register_fixture(daemon_dir, make_fixture_repo("one"))
    slug_b = register_fixture(daemon_dir, make_fixture_repo("two"))
    write_fixture_desk(daemon_dir, slug_a, slug_b)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport=dict(VIEW))
            errors = []
            page = desk_page(ctx)
            page.on("pageerror", lambda e: errors.append(str(e)))

            # ===== scenario 1: the toolbar list IS the fence roster ===========
            rows = open_fence_list(page)
            check(
                "the toolbar lists every fence on the plane, in order",
                [r["name"] for r in rows] == ["alpha", "beta", "gamma"],
                f"got={[r['name'] for r in rows]}",
            )
            check(
                "…alpha reads its repo and its two consoles",
                rows[0]["repos"] == slug_a and rows[0]["count"] == "2 consoles",
                f"got={rows[0]}",
            )
            check(
                "…beta reads its own repo and the SINGULAR console",
                rows[1]["repos"] == slug_b and rows[1]["count"] == "1 console",
                f"got={rows[1]}",
            )
            check(
                "…and an EMPTY fence is still listed, at zero and with no repos",
                rows[2]["repos"] == "" and rows[2]["count"] == "0 consoles",
                f"got={rows[2]}",
            )

            # ===== scenario 2: a row click slides the viewport ================
            before = view_state(page, "f-gamma")
            check(
                "gamma starts entirely OFF-view — without this the jump's oracle is vacuous",
                before["left"] - before["scrollLeft"] > before["clientWidth"],
                f"left={before['left']} scrollLeft={before['scrollLeft']} cw={before['clientWidth']}",
            )
            check(
                "…and nothing is focused before the click",
                before["focused"] is None,
                f"focused={before['focused']!r}",
            )
            quiet(desk_file)
            snapshot = http("GET", "api/desk")[1]

            click_fence_row(page, "gamma")
            after = view_state(page, "f-gamma")
            check(
                "clicking the row brings gamma fully into view on X",
                after["left"] - after["scrollLeft"] >= 0
                and after["right"] - after["scrollLeft"] <= after["clientWidth"],
                f"left={after['left']} right={after['right']} scrollLeft={after['scrollLeft']}"
                f" cw={after['clientWidth']}",
            )
            check(
                "…and fully into view on Y",
                after["top"] - after["scrollTop"] >= 0
                and after["bottom"] - after["scrollTop"] <= after["clientHeight"],
                f"top={after['top']} bottom={after['bottom']} scrollTop={after['scrollTop']}"
                f" ch={after['clientHeight']}",
            )
            check(
                "…the view really MOVED — a jump that did nothing cannot pass containment",
                after["scrollLeft"] != before["scrollLeft"],
                f"before={before['scrollLeft']} after={after['scrollLeft']}",
            )
            check(
                "…gamma is now the focused fence",
                after["focused"] == "f-gamma",
                f"focused={after['focused']!r}",
            )
            focus_ring = page.evaluate(
                "() => [...document.querySelectorAll('.fence.is-focused')]"
                "  .map((f) => f.dataset.fenceId)"
            )
            check(
                "…and exactly that fence carries the focus ring",
                focus_ring == ["f-gamma"],
                f"ring={focus_ring}",
            )
            quiet(desk_file)
            check(
                "…while NOT ONE rect changed: the desk is byte-identical after the jump",
                http("GET", "api/desk")[1] == snapshot,
                "GET /api/desk differs" if http("GET", "api/desk")[1] != snapshot else "",
            )

            # ===== scenario 3: a console is BORN inside the focused fence =====
            click_fence_row(page, "beta")
            born = open_plain_console(page)
            box = by_id(boxes(page))[born]["box"]
            check(
                "a console opened while beta is focused is born INSIDE beta",
                inside(box, FENCE_B),
                f"box={box} fence={FENCE_B}",
            )
            check(
                "…and below beta's head band, not over it",
                box["top"] >= FENCE_B["top"],
                f"box={box}",
            )
            rows = open_fence_list(page)
            check(
                "…and beta's row follows without a reload",
                rows[1]["name"] == "beta" and rows[1]["count"] == "2 consoles",
                f"got={rows[1]}",
            )
            close_menus(page)
            quiet(desk_file)
            rec = desk_record(born)
            # The MEASURED box, never the inline style: `restoreRect` reads
            # integer offsets, so a 0.5 px tolerance is exactly the wrong one
            # (#342).
            check(
                "…the daemon stores the born rect, and it is inside beta too",
                rec is not None and inside(rec["rect"], FENCE_B) and rec["rect"] == box,
                f"served={rec and rec['rect']} measured={box}",
            )
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(1800)
            settle_windows(page, 4)
            reloaded = by_id(boxes(page)).get(born)
            check(
                "…and a RELOAD reproduces it box for box",
                reloaded is not None and reloaded["box"] == box,
                f"after reload={reloaded and reloaded['box']} before={box}",
            )

            # ===== scenario 4: with no fence focused, the plain cascade =======
            click_fence_row(page, "alpha")
            check(
                "alpha is focused before the floor press — the clearing must have something to clear",
                page.evaluate("() => window.WBConsole.focusedFence()") == "f-alpha",
                "",
            )
            # Pin the plane at 0,0 first: a stage point is only pressable while
            # it is ON SCREEN, and the jump above left the viewport elsewhere.
            # (850, 600) is then visible AND held by no fence — alpha ends at
            # x=640, beta at y=340, gamma starts at (2200, 900) — and no window
            # covers it, so the press reaches the bare floor.
            unscroll(page)
            press_floor(page, 850, 600)
            check(
                "a floor press outside the focused fence clears the focus",
                page.evaluate("() => window.WBConsole.focusedFence()") is None,
                f"focused={page.evaluate('() => window.WBConsole.focusedFence()')!r}",
            )
            plain = open_plain_console(page)
            pbox = by_id(boxes(page))[plain]["box"]
            # Today's rule is the origin-relative 8-slot cascade: `left = 30 +
            # k*24`, `top = 20 + k*24`, the SAME k on both axes. That last part
            # is what discriminates: a fence-relative birth is `fenceLeft + 12 +
            # …` / `fenceTop + head + 12 + …`, which lands on no common slot.
            # Deliberately NOT "in no fence" — the plain cascade is anchored at
            # the plane's origin and alpha sits over it in this fixture, so a
            # containment oracle here would fail correct code.
            slot_x = (pbox["left"] - 30) / 24
            slot_y = (pbox["top"] - 20) / 24
            check(
                "…and the next console lands on the plain 8-slot cascade, exactly as it does today",
                slot_x == slot_y and slot_x == int(slot_x) and 0 <= slot_x <= 7,
                f"box={pbox} slot_x={slot_x} slot_y={slot_y}",
            )
            check(
                "…and NOT on the box a birth into alpha would have produced",
                (pbox["left"], pbox["top"]) != (FENCE_A["left"] + 12, FENCE_A["top"] + 12),
                f"box={pbox}",
            )

            # ===== scenario 5: the list is LIVE, all in one page, no reload ===
            # AC6's five verbs — create, rename, move, a membership change, and
            # a removal — each followed by a fresh open of the picker. NOTE the
            # move leg: #341's anchoring carries a fence's members WITH it, so a
            # moved fence keeps its count by design; what the list must show is
            # that the row survives the move intact.
            close_menus(page)
            unscroll(page)
            page.locator("button[title='draw a named fence on the plane']").click()
            page.wait_for_timeout(600)
            rows = open_fence_list(page)
            new_fence = page.evaluate(
                "() => { const ids = ['f-alpha','f-beta','f-gamma'];"
                " return [...document.querySelectorAll('.fence')]"
                "   .map((f) => f.dataset.fenceId).find((id) => !ids.includes(id)) || null; }"
            )
            check(
                "creating a fence adds its row, without a reload",
                len(rows) == 4 and new_fence is not None,
                f"rows={[r['name'] for r in rows]} new={new_fence!r}",
            )

            # --- rename: gamma is off-view, so the jump is how its input is
            #     reachable at all. The list is what put it in reach.
            click_fence_row(page, "gamma")
            page.locator("[data-fence-id='f-gamma'] .fence-name").fill("delta")
            page.locator("[data-fence-id='f-gamma'] .fence-name").press("Enter")
            page.wait_for_timeout(500)
            rows = open_fence_list(page)
            check(
                "renaming a fence renames its row, without a reload",
                row_named(rows, "delta") is not None and row_named(rows, "gamma") is None,
                f"rows={[r['name'] for r in rows]}",
            )

            # --- a membership change by dragging a window IN
            close_menus(page)
            unscroll(page)
            fr = fence_rect(page, new_fence)
            target = (fr["left"] + fr["width"] / 2, fr["top"] + fr["head"] + fr["height"] / 2)
            drag_window_to(page, plain, target[0], target[1])
            rows = open_fence_list(page)
            new_name = page.evaluate(
                "(id) => document.querySelector(`[data-fence-id='${id}'] .fence-name`).value",
                new_fence,
            )
            check(
                "dragging a window into a fence moves its row to one console, without a reload",
                row_named(rows, new_name)["count"] == "1 console"
                and row_named(rows, new_name)["repos"] == "home",
                f"row={row_named(rows, new_name)}",
            )

            # --- the MOVE leg: re-measure the handle AFTER the membership
            #     change — it now sits under the window that just arrived.
            close_menus(page)
            before_rect = fence_rect(page, new_fence)
            grab = client_centre(page, f"[data-fence-id='{new_fence}'] .fence-grab")
            drag(page, grab, 0, 120)
            after_rect = fence_rect(page, new_fence)
            rows = open_fence_list(page)
            check(
                "a moved fence really moved…",
                after_rect["top"] != before_rect["top"],
                f"before={before_rect['top']} after={after_rect['top']}",
            )
            check(
                "…and its row survives the move intact — #341's anchoring carries the member along",
                row_named(rows, new_name)["count"] == "1 console",
                f"row={row_named(rows, new_name)}",
            )

            # --- a membership change by dragging the window OUT
            close_menus(page)
            out = free_point(page)
            drag_window_to(page, plain, out[0], out[1])
            rows = open_fence_list(page)
            check(
                "dragging the window out drops the row back to zero, without a reload",
                row_named(rows, new_name)["count"] == "0 consoles"
                and row_named(rows, new_name)["repos"] == "",
                f"row={row_named(rows, new_name)}",
            )

            # --- the removal
            close_menus(page)
            page.locator(f"[data-fence-id='{new_fence}'] .fence-drop").click()
            page.wait_for_timeout(500)
            rows = open_fence_list(page)
            check(
                "removing a fence removes its row, without a reload",
                len(rows) == 3 and row_named(rows, new_name) is None,
                f"rows={[r['name'] for r in rows]}",
            )
            close_menus(page)

            # ===== scenario 6: two contexts, one plane, two viewports ========
            # B gets a FRESH `localStorage`, so it has its own stored view: the
            # fences are shared desk state, the viewport position is not
            # (ADR-0051 §8, #339). There is no live cross-tab desk push, so B is
            # opened only after A's 250 ms flush has gone quiet.
            click_fence_row(page, "delta")
            quiet(desk_file)
            a_rows = open_fence_list(page)
            a_scroll = page.evaluate("() => document.getElementById('workspace').scrollLeft")

            ctx_b = browser.new_context(viewport=dict(VIEW))
            page_b = desk_page(ctx_b, fences=3, windows=5)
            b_rows = open_fence_list(page_b)
            b_scroll = page_b.evaluate("() => document.getElementById('workspace').scrollLeft")
            check(
                "a second context reads the SAME fence names and counts",
                [(r["name"], r["count"]) for r in a_rows]
                == [(r["name"], r["count"]) for r in b_rows],
                f"A={[(r['name'], r['count']) for r in a_rows]} B={[(r['name'], r['count']) for r in b_rows]}",
            )
            check(
                "…while each keeps its OWN viewport position",
                a_scroll != b_scroll,
                f"A.scrollLeft={a_scroll} B.scrollLeft={b_scroll}",
            )
            ctx_b.close()

            # The evidence: context A, the fence list open over the fence it
            # just jumped to and focused.
            open_fence_list(page)
            ring = page.evaluate(
                "() => { const f = document.querySelector('.fence.is-focused');"
                " if (!f) return null;"
                " const s = getComputedStyle(f);"
                " const accent = getComputedStyle(document.documentElement)"
                "   .getPropertyValue('--accent').trim();"
                " return { id: f.dataset.fenceId, width: s.outlineWidth,"
                "   style: s.outlineStyle, colour: s.outlineColor, accent }; }"
            )
            # The ring is MEASURED, not eyeballed off the screenshot: a rule that
            # never applied would still leave a plausible-looking image.
            check(
                "the fence in the shot carries a real accent ring, not just the class",
                ring is not None
                and ring["width"] == "2px"
                and ring["style"] == "solid"
                and ring["colour"] not in ("", "rgba(0, 0, 0, 0)"),
                f"ring={ring}",
            )
            page.screenshot(path=SHOT)
            check("the evidence screenshot is on disk", os.path.exists(SHOT), SHOT)

            check("no page error was raised by the whole pass", errors == [], f"pageerrors={errors}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 34
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE FENCE LIST IS THE MAP")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
