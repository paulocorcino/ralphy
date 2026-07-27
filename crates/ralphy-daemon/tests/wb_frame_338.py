"""#338 browser acceptance: the canvas chrome is FRAME chrome; maximize fills the frame.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The `desk.toml` fixture is
the same pre-#336 shape `wb_stage_336.py` uses (bbox 1300x680 + 200 margin =
1500x880 at viewport 1400x900), with `max = false` on BOTH records: a pre-seeded
full-bleed window covers its neighbour's chrome and makes every later click
unhittable, so the persisted-maximize criterion is proved by a REAL maximize +
reload + on-disk read instead.

Scenario 1   `.canvas-foot` and `.canvas-empty` resolve inside `.consoles-tab`
             and are NOT descendants of `#stage`; the pills read the live state
Scenario 2   a 250px pan leaves the foot's client rect byte-identical while a
             window's client rect moves by exactly -250 (the negative control)
Scenario 3   maximize fills the VIEWPORT at a scrolled offset — the terminal
             refits wider — without growing the stage extent
Scenario 4   Go-to another window WHILE maximized pans the plane and the pin
             follows it, so the full-bleed never desyncs from the frame
Scenario 5   Arrange leaves a maximized window's restore rect alone, and
             restoring puts it back on its stage-coordinate box (screen + desk)
Scenario 6   a persisted `max = true` comes back maximized across a reload and
             still restores to the right box
Scenario 7   closing both consoles reveals the empty-stage hint and the pill
             falls to `0 consoles`

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/338-frame-chrome-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_frame_338.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7433
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "338-frame-chrome-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

FIX_A = {"left": 40, "top": 40, "width": 600, "height": 380}
FIX_B = {"left": 700, "top": 300, "width": 600, "height": 380}
STAGE_W = 1500
STAGE_H = 880

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
    empty = tempfile.mkdtemp(prefix="wb338_empty_")
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
    d = tempfile.mkdtemp(prefix="wb338_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #338 frame-chrome fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb338@example.com"],
        ["git", "config", "user.name", "wb338"],
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


def write_legacy_desk(daemon_dir, slug):
    """The pre-#336 `desk.toml`, written by hand rather than by this build."""
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
    page.wait_for_timeout(500)


def desk_page(ctx, viewport):
    page = ctx.new_page()
    page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(1800)
    return page


def chrome_placement(page):
    """Where the two chrome elements RESOLVE — the whole point of this issue."""
    return page.evaluate(
        "() => { const q = (s) => document.querySelector(s);"
        "  const info = (el) => el && ({ inTab: el.closest('.consoles-tab') !== null,"
        "    inStage: el.closest('#stage') !== null,"
        "    parent: el.parentElement && (el.parentElement.id || el.parentElement.className) });"
        "  return { foot: info(q('.canvas-foot')), empty: info(q('.canvas-empty')) }; }"
    )


def bare_floor(page):
    """A viewport point whose hit test lands on the STAGE itself.

    `onFloorDown` tests element IDENTITY (`e.target !== st` bails), so a press
    that lands on a window — or on chrome that forgot `pointer-events:none` —
    starts no pan at all and the whole scenario passes vacuously at scrollLeft 0.
    """
    return page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        "  const st = document.getElementById('stage');"
        "  const r = ws.getBoundingClientRect();"
        "  const x = r.left + r.width / 2, y = r.bottom - 12;"
        "  const el = document.elementFromPoint(x, y);"
        "  return { x, y, onStage: el === st, hit: el && (el.id || el.className) }; }"
    )


def pan_floor(page, dx, want):
    """Pan the plane by a REAL floor drag, then wait for the offset to land."""
    at = bare_floor(page)
    check("the drag starts on the bare floor, not on a window or on chrome", at["onStage"], f"hit={at['hit']}")
    page.mouse.move(at["x"], at["y"])
    page.mouse.down()
    for f in (0.34, 0.67, 1.0):
        page.mouse.move(at["x"] - dx * f, at["y"], steps=4)
    page.mouse.up()
    page.wait_for_function(
        "(n) => document.getElementById('workspace').scrollLeft === n",
        arg=want,
        timeout=8000,
    )


def frame_fill(page, index):
    """How exactly window `index` covers the viewport, edge by edge."""
    return page.evaluate(
        "(i) => { const ws = document.getElementById('workspace');"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const a = w.getBoundingClientRect(), b = ws.getBoundingClientRect();"
        "  return { left: a.left - b.left, top: a.top - b.top,"
        "    right: a.right - b.right, bottom: a.bottom - b.bottom,"
        "    maximized: w.classList.contains('maximized'),"
        "    scrollLeft: ws.scrollLeft, scrollTop: ws.scrollTop }; }",
        index,
    )


def fills_frame(f):
    return all(abs(f[edge]) <= 1 for edge in ("left", "top", "right", "bottom"))


def poll_desk(want_id, predicate, timeout=8):
    """Read `GET /api/desk` until the record satisfies `predicate`.

    A desk write is debounced-then-HTTP, so a single fixed sleep before reading
    is a coin flip (#337 handoff). Same assertion, bounded wait.
    """
    deadline = time.time() + timeout
    rec = None
    while time.time() < deadline:
        try:
            rec = next(
                (r for r in json.loads(http("GET", "api/desk")[1]) if r["id"] == want_id),
                None,
            )
        except Exception:
            rec = None
        if rec and predicate(rec):
            return rec
        time.sleep(0.3)
    return rec


def press_chrome(page, index, selector):
    """Click a window's own chrome button from inside the page.

    A full-bleed console covers its neighbours' chrome and Arrange fills the
    frame, so `locator.click()` on anything but the topmost window times out
    with "intercepts pointer events" (#336 handoff). This is still the real
    button and the real handler.
    """
    page.evaluate(
        "([i, sel]) => document.querySelectorAll('.session-window')[i].querySelector(sel).click()",
        [index, selector],
    )


def index_of(page, desk_id):
    """Where `desk_id` sits in DOM order — a reload need not preserve it."""
    return page.evaluate(
        "(id) => [...document.querySelectorAll('.session-window')]"
        ".findIndex((w) => w._deskId === id)",
        desk_id,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb338_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_legacy_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})

            # ===== scenario 1: the chrome belongs to the FRAME =================
            page = desk_page(ctx, {"width": 1400, "height": 900})
            settle(page)
            # #339: the view now LANDS on the bounding box of the restored
            # windows instead of at the plane's corner. Every offset below is
            # measured RELATIVE to a known origin, so put the origin back — the
            # landing has its own suite (`wb_view_339.py`).
            page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                "  ws.scrollLeft = 0; ws.scrollTop = 0; }"
            )
            page.wait_for_timeout(600)
            check(
                "the fixture desk restores verbatim",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )

            place = chrome_placement(page)
            check(
                "`.canvas-foot` exists in the DOM at all",
                place["foot"] is not None,
                "ADR-0051 §5's footer pills were dead CSS before this issue",
            )
            check(
                "`.canvas-empty` exists in the DOM at all",
                place["empty"] is not None,
                "ADR-0051 §5's empty-stage hint was dead CSS before this issue",
            )
            check(
                "the foot resolves inside `.consoles-tab` (the frame)",
                bool(place["foot"] and place["foot"]["inTab"]),
                f"got={place['foot']}",
            )
            check(
                "…and is NOT a descendant of `#stage` (the plane)",
                bool(place["foot"] and not place["foot"]["inStage"]),
                f"got={place['foot']}",
            )
            check(
                "the empty hint resolves inside `.consoles-tab` (the frame)",
                bool(place["empty"] and place["empty"]["inTab"]),
                f"got={place['empty']}",
            )
            check(
                "…and is NOT a descendant of `#stage` (the plane)",
                bool(place["empty"] and not place["empty"]["inStage"]),
                f"got={place['empty']}",
            )

            # The pills are the plane made legible: both read live shell state, so
            # a literal here is an oracle over `consoleCount` AND `stageExtent`.
            pills = page.evaluate(
                "() => { const f = document.querySelector('.canvas-foot');"
                "  const e = document.querySelector('.canvas-empty');"
                "  return { pills: [...f.querySelectorAll('.pill')].map((s) => s.textContent.trim()),"
                "    footVisible: f.offsetParent !== null && f.clientWidth > 0,"
                "    emptyShown: e.offsetParent !== null }; }"
            )
            check(
                "the first pill counts the open consoles",
                pills["pills"][:1] == ["2 consoles"],
                f"got={pills['pills']}",
            )
            check(
                f"…and the second reads the stage extent ({STAGE_W} × {STAGE_H})",
                pills["pills"][1:2] == [f"stage {STAGE_W} × {STAGE_H}"],
                f"got={pills['pills']}",
            )
            check("…on a foot that is really on screen", pills["footVisible"], f"got={pills}")
            check(
                "the empty-stage hint is hidden while consoles are open",
                not pills["emptyShown"],
                f"got={pills}",
            )

            # ===== scenario 2: the plane pans UNDER the chrome =================
            before = page.evaluate(
                "() => { const f = document.querySelector('.canvas-foot').getBoundingClientRect();"
                "  const w = document.querySelectorAll('.session-window')[0].getBoundingClientRect();"
                "  return { foot: { left: f.left, top: f.top }, win: { left: w.left } }; }"
            )
            pan_floor(page, 250, 250)
            after = page.evaluate(
                "() => { const f = document.querySelector('.canvas-foot').getBoundingClientRect();"
                "  const w = document.querySelectorAll('.session-window')[0].getBoundingClientRect();"
                "  return { foot: { left: f.left, top: f.top }, win: { left: w.left },"
                "    pills: [...document.querySelectorAll('.canvas-foot .pill')].map((s) => s.textContent.trim()) }; }"
            )
            # The NEGATIVE control: without it every "unchanged" assertion below
            # would also pass on a plane that never moved.
            check(
                "a 250px floor drag really pans the plane under window 0",
                round(after["win"]["left"] - before["win"]["left"]) == -250,
                f"window moved by {after['win']['left'] - before['win']['left']}",
            )
            check(
                "…while the footer's client `left` does not move a pixel",
                after["foot"]["left"] == before["foot"]["left"],
                f"{before['foot']['left']} -> {after['foot']['left']}",
            )
            check(
                "…nor its `top`",
                after["foot"]["top"] == before["foot"]["top"],
                f"{before['foot']['top']} -> {after['foot']['top']}",
            )
            check(
                "…and the pan grew no extent, so the pills still read the same stage",
                after["pills"][1:2] == [f"stage {STAGE_W} × {STAGE_H}"],
                f"got={after['pills']}",
            )

            # ===== scenario 3: maximize fills the FRAME, not the plane =========
            pre = page.evaluate(
                "() => { const st = document.getElementById('stage');"
                "  const w = document.querySelectorAll('.session-window')[0];"
                "  return { cols: w._term.term.cols, rows: w._term.term.rows,"
                "    stageW: st.offsetWidth, stageH: st.offsetHeight }; }"
            )
            check(
                "the stage measures the fixture bbox + margin before the maximize",
                (pre["stageW"], pre["stageH"]) == (STAGE_W, STAGE_H),
                f"got={pre['stageW']}x{pre['stageH']}",
            )
            page.locator(".session-window").nth(0).locator(".session-max").click()
            page.wait_for_timeout(700)
            f3 = frame_fill(page, 0)
            post = page.evaluate(
                "() => { const st = document.getElementById('stage');"
                "  const w = document.querySelectorAll('.session-window')[0];"
                "  return { cols: w._term.term.cols, stageW: st.offsetWidth, stageH: st.offsetHeight,"
                "    pills: [...document.querySelectorAll('.canvas-foot .pill')].map((s) => s.textContent.trim()) }; }"
            )
            check("the maximize is really on", f3["maximized"], f"got={f3}")
            check(
                "the maximized console fills the VIEWPORT it was scrolled to",
                fills_frame(f3),
                f"edges (l,t,r,b) = {f3['left']},{f3['top']},{f3['right']},{f3['bottom']}",
            )
            check(
                "…without losing the scroll offsets the pin is derived from",
                f3["scrollLeft"] == 250,
                f"scrollLeft={f3['scrollLeft']}",
            )
            check(
                "…and the terminal refits WIDER, so the full bleed is readable",
                post["cols"] > pre["cols"],
                f"cols {pre['cols']} -> {post['cols']}",
            )
            check(
                "a maximized window does not grow the stage extent",
                (post["stageW"], post["stageH"]) == (STAGE_W, STAGE_H),
                f"{pre['stageW']}x{pre['stageH']} -> {post['stageW']}x{post['stageH']}",
            )
            check(
                "…which the footer pill still reports over the full bleed",
                post["pills"][1:2] == [f"stage {STAGE_W} × {STAGE_H}"],
                f"got={post['pills']}",
            )

            # The two properties the CSS half of this issue turns on. Text alone
            # reads the same whether the pills paint above the bleed or under it,
            # so assert the cascade AND the hit test.
            stack = page.evaluate(
                "() => { const f = document.querySelector('.canvas-foot');"
                "  const pill = f.querySelector('.pill');"
                "  const w = document.querySelector('.session-window.maximized');"
                "  const r = pill.getBoundingClientRect();"
                "  const under = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                "  return { footZ: getComputedStyle(f).zIndex,"
                "    winZ: parseInt(w.style.zIndex, 10) || 0,"
                "    events: getComputedStyle(f).pointerEvents,"
                "    underIsChrome: !!(under && under.closest('.canvas-foot')),"
                "    underInWindow: !!(under && under.closest('.session-window')) }; }"
            )
            check(
                "the footer paints ABOVE the full bleed, by the cascade",
                stack["footZ"] == "130" and stack["winZ"] < 130,
                f"foot z={stack['footZ']} window z={stack['winZ']}",
            )
            check(
                "…and is inert to the pointer, so a pill cannot eat a click",
                stack["events"] == "none"
                and not stack["underIsChrome"]
                and stack["underInWindow"],
                f"pointer-events={stack['events']} hit-through-to-window={stack['underInWindow']}",
            )

            # ===== scenario 4: Go-to pans the plane WHILE maximized ============
            # The only product path that moves the viewport under a full bleed.
            # Before #338 `reveal` refused outright while `maxlock` was on, so the
            # "restore after a pan while maximized" criterion was unreachable.
            page.locator("button", has_text="Go to").first.click()
            page.wait_for_timeout(300)
            rows_seen = page.locator(".window-menu .window-item:visible").count()
            check("the Go-to menu lists both windows", rows_seen == 2, f"got={rows_seen}")
            page.locator(".window-menu .window-item:visible").nth(1).click()
            page.wait_for_function(
                "() => document.getElementById('workspace').scrollLeft !== 250",
                timeout=8000,
            )
            page.wait_for_timeout(400)
            f4 = frame_fill(page, 0)
            pin = page.evaluate(
                "() => { const w = document.querySelector('.session-window.maximized');"
                "  return { left: w.style.getPropertyValue('--max-left'),"
                "    top: w.style.getPropertyValue('--max-top') }; }"
            )
            # NOT the same statement as the wait above: the wait proves the plane
            # moved, this proves the PIN was re-derived to the new offsets rather
            # than the class merely surviving.
            check(
                "the viewport pin is re-derived to the offsets Go-to landed on",
                pin["left"] == f"{f4['scrollLeft']}px" and pin["top"] == f"{f4['scrollTop']}px",
                f"--max-left/top = {pin['left']}/{pin['top']} at scroll"
                f" {f4['scrollLeft']}/{f4['scrollTop']}",
            )
            check(
                "…and the full bleed follows the frame instead of desyncing from it",
                f4["maximized"] and fills_frame(f4),
                f"edges (l,t,r,b) = {f4['left']},{f4['top']},{f4['right']},{f4['bottom']}",
            )

            # The OTHER branch of the new `reveal()`: a Go-to whose target is
            # itself maximized already fills the frame, so it must not slide the
            # plane out from under the operator.
            held = f4["scrollLeft"]
            page.locator("button", has_text="Go to").first.click()
            page.wait_for_timeout(300)
            page.locator(".window-menu .window-item:visible").nth(0).click()
            page.wait_for_timeout(600)
            f4b = frame_fill(page, 0)
            check(
                "Go-to on the MAXIMIZED window itself leaves the plane where it is",
                f4b["scrollLeft"] == held and fills_frame(f4b),
                f"scrollLeft {held} -> {f4b['scrollLeft']}",
            )

            # ===== scenario 5b: Arrange leaves a maximized window alone ========
            # `.maximized` overrides all four offsets with `!important`, so a
            # tile rect written onto it is invisible on screen AND silently
            # replaces the restore rect this issue owns (#336 residue L2).
            page.locator("button", has_text="Arrange").first.click()
            page.wait_for_timeout(700)
            inline = page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                "  return { left: w.style.left, top: w.style.top,"
                "    width: w.style.width, height: w.style.height,"
                "    maximized: w.classList.contains('maximized'),"
                "    other: document.querySelectorAll('.session-window')[1].style.left }; }"
            )
            check(
                "Arrange does not tile the maximized console…",
                (inline["left"], inline["top"]) == ("40px", "40px"),
                f"got left={inline['left']} top={inline['top']}",
            )
            check(
                "…nor overwrite the size it must restore to",
                (inline["width"], inline["height"]) == ("600px", "380px"),
                f"got width={inline['width']} height={inline['height']}",
            )
            check(
                "…while still tiling the window that is NOT maximized",
                inline["other"] and inline["other"] != "700px",
                f"the other window's inline left = {inline['other']!r}",
            )
            # The tile must not BURY the full bleed either: filtering it out of
            # the grid also drops it from `focusWin`, and `maxlock` leaves no way
            # to scroll away from a maximized console whose titlebar is covered.
            top_at_max = page.evaluate(
                "() => { const w = document.querySelector('.session-window.maximized');"
                "  const b = w.querySelector('.session-max').getBoundingClientRect();"
                "  const el = document.elementFromPoint(b.left + b.width / 2, b.top + b.height / 2);"
                "  return { own: !!(el && w.contains(el)), tag: el && el.className }; }"
            )
            check(
                "…and leaves the maximized console's own restore button hittable",
                top_at_max["own"],
                f"the point over `.session-max` hit {top_at_max['tag']!r}",
            )
            arranged = poll_desk("w-fixture-b", lambda r: True, timeout=3)
            print(f"[note] after Arrange, /api/desk holds w-fixture-b at {arranged and arranged['rect']}")

            # ===== scenario 5: restore lands on the STAGE-coordinate box =======
            # A REAL click: the check above just proved the button is on top.
            page.locator(".session-window.maximized").locator(".session-max").click()
            page.wait_for_timeout(600)
            restored = page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                "  return { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth,"
                "    height: w.offsetHeight, maximized: w.classList.contains('maximized'),"
                "    lock: document.getElementById('workspace').classList.contains('maxlock') }; }"
            )
            check(
                "restoring after a pan puts the window back on its pre-maximize rect",
                restored == {
                    "left": 40, "top": 40, "width": 600, "height": 380,
                    "maximized": False, "lock": False,
                },
                f"got={restored}",
            )
            rec = poll_desk(
                "w-fixture-a",
                lambda r: r["rect"] == FIX_A and r["max"] is False,
            )
            check(
                "…and the daemon's desk agrees with the screen",
                bool(rec) and rec["rect"] == FIX_A and rec["max"] is False,
                f"got={rec}",
            )

            # ===== scenario 6: a persisted maximize survives a reload ==========
            # At `scrollLeft = 0`, or the viewport PIN (`--max-left`) becomes the
            # window's `offsetLeft` and every cross-reload number shifts with it
            # (#336 handoff, cost: one red in `wb_desk_303.py`).
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")
            page.wait_for_timeout(300)
            press_chrome(page, index_of(page, "w-fixture-a"), ".session-max")
            page.wait_for_timeout(600)
            # WHICH record carries it, over the API — a bare `"max = true" in
            # <the whole file>` would be satisfied by any other window.
            flagged = poll_desk("w-fixture-a", lambda r: r["max"] is True)
            check(
                "the maximized flag is on the fixture's own desk record",
                bool(flagged) and flagged["max"] is True,
                f"got={flagged}",
            )
            # …and that it reached DISK, not just the in-memory desk. The daemon
            # rewrites this file, so a read can land mid-write on Windows.
            desk_file = Path(daemon_dir, "desk.toml")
            deadline = time.time() + 8
            on_disk = ""
            while time.time() < deadline:
                try:
                    on_disk = desk_file.read_text(encoding="utf-8")
                except OSError:
                    on_disk = ""
                if "max = true" in on_disk:
                    break
                time.sleep(0.3)
            check(
                "…and reaches desk.toml on disk, so a reload can read it back",
                "max = true" in on_disk,
                f"desk.toml={on_disk[:160]!r}",
            )

            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            page.wait_for_timeout(1800)
            settle(page, 2)
            back = index_of(page, "w-fixture-a")
            check("the reloaded desk still carries the fixture window", back >= 0, f"index={back}")
            f6 = frame_fill(page, back)
            check(
                "a persisted maximized console comes back MAXIMIZED",
                f6["maximized"],
                f"got={f6}",
            )
            check(
                "…filling the frame it was reloaded into",
                fills_frame(f6),
                f"edges (l,t,r,b) = {f6['left']},{f6['top']},{f6['right']},{f6['bottom']}",
            )
            # Against the LIVE stage, not a literal: whatever rect Arrange left
            # persisted, the pill's job is to mirror the extent that is actually
            # laid out. A literal here would have to be re-derived — and would
            # red — the day `arrange()` persists the rect it tiles to.
            mirror = page.evaluate(
                "() => { const st = document.getElementById('stage');"
                "  return { pills: [...document.querySelectorAll('.canvas-foot .pill')]"
                "      .map((s) => s.textContent.trim()),"
                "    w: st.offsetWidth, h: st.offsetHeight }; }"
            )
            check(
                "…and the footer pills survive the reload, still mirroring the live stage",
                mirror["pills"] == ["2 consoles", f"stage {mirror['w']} × {mirror['h']}"],
                f"got={mirror['pills']} against a {mirror['w']}x{mirror['h']} stage",
            )
            # The evidence PNG, taken at the asserting moment: a full-bleed
            # terminal with the frame's pills still legible at the bottom-left.
            page.screenshot(path=SHOT)

            press_chrome(page, back, ".session-max")
            page.wait_for_timeout(600)
            after_reload = page.evaluate(
                "(i) => { const w = document.querySelectorAll('.session-window')[i];"
                "  return { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth,"
                "    height: w.offsetHeight }; }",
                back,
            )
            check(
                "…and restoring it across the reload still lands on the right box",
                after_reload == FIX_A,
                f"want={FIX_A} got={after_reload}",
            )

            # ===== scenario 7: the empty-stage hint ============================
            press_chrome(page, 0, ".session-close")
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window').length === 1",
                timeout=10000,
            )
            page.wait_for_timeout(300)
            check(
                "the count pill takes the SINGULAR at one console",
                page.evaluate(
                    "() => document.querySelector('.canvas-foot .pill').textContent.trim()"
                ) == "1 console",
                "a plural-only pill would read `1 consoles`",
            )
            press_chrome(page, 0, ".session-close")
            page.wait_for_function(
                "() => { const e = document.querySelector('.canvas-empty');"
                "  return document.querySelectorAll('.session-window').length === 0"
                "    && e && e.offsetParent !== null && e.clientWidth > 0; }",
                timeout=10000,
            )
            empty = page.evaluate(
                "() => { const e = document.querySelector('.canvas-empty');"
                "  return { text: e.textContent.replace(/\\s+/g, ' ').trim(),"
                "    shown: e.offsetParent !== null, cw: e.clientWidth,"
                "    pills: [...document.querySelectorAll('.canvas-foot .pill')]"
                "      .map((s) => s.textContent.trim()) }; }"
            )
            check(
                "closing the last console reveals the empty-stage hint",
                empty["shown"] and empty["cw"] > 0 and "No consoles open" in empty["text"],
                f"got={empty['text']!r} shown={empty['shown']} cw={empty['cw']}",
            )
            check(
                "…and the count pill falls to zero, singular-aware",
                empty["pills"][:1] == ["0 consoles"],
                f"got={empty['pills']}",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor matches the real count: set loosely, a scenario that stopped
    # running would leave the suite green.
    ok = all(results) and len(results) >= 46
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE CHROME IS IN THE FRAME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
