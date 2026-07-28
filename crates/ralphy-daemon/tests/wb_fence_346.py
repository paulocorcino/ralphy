"""#346 browser acceptance: A FENCE DETACHES INTO ITS OWN WINDOW, AND COMES HOME.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7439, so this can run
beside #340-#343's suites without any daemon stealing another's port.

The fixture `desk.toml` is written BEFORE the daemon starts: `f-alpha` holds two
`console` records (real PTYs, so "the consoles are live in the popup" and the
typing scenario are not vacuous), and four more empty fences give the cap
scenario its fifth detach. Every member box clears alpha's head band AND its
14 px SE grip: both handles sit at `z-index: 1`, BELOW every window, so a member
parked on one makes the fence ungrabbable from a script (#341's covered-handle
trap). No two fences overlap.

Scenario 1  the fence chrome carries detach BETWEEN arrange and close, in the
            top-right corner, and the button is really hittable there
Scenario 2  the round trip: the popup carries the workbench's styling and the
            fence's name, holds the consoles LIVE, the origin fence keeps its
            name/rect/list-row while rendering no member and showing its glyph,
            and closing the popup returns every console to the exact box it was
            detached from
Scenario 3  the glyph's intents: with the popup alive a click FOCUSES it (the
            consoles stay away and the popup survives); with it gone the click
            is the safe no-op the fold's idempotence guarantees
Scenario 4  a popup-local drag is discarded on re-attach, and NOT ONE
            `PUT /api/desk` is issued from the popup context
Scenario 5  the popup exposes no way to open a new console
Scenario 6  a popup with no opener renders the empty state and no window
Scenario 7  four fences detach; the FIFTH is refused, said on the fence, and
            nothing of the refused fence is torn down
Scenario 8  a popup the browser blocks tells the operator and changes nothing
Scenario 9  the detached console is DRIVEABLE — a typed line reaches the child
Scenario 10 a session another client drives arrives in the popup parked, with
            the existing explicit *take over*
Scenario 11 a detached fence still moves and resizes on the plane, and arrange
            on it writes nothing and moves no member it can still see
Scenario 12 a console born while a detached fence is focused is born IN it, on
            the plane, and Alt+Shift+→ still reaches that fence
Scenario 13 a second browser context renders the fence with its consoles inside
            it and no glyph — the detach is per-tab (the screenshot is taken here)

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_fence_346.py   (exit 0 = all pass)
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

PORT = 7439
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
# The deterministic echo child (#334): every console becomes a
# `session_test_child`, whose `GOT:<line>` reply is a machine-readable oracle for
# "this keystroke reached the PTY".
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "346-a-fence-detaches-into-its-own-window-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The fixture geometry, in stage coordinates. NO TWO FENCES OVERLAP — an
# overlapping pair would make `fenceMembership`'s tie-break, not the detach,
# decide which fence a member belongs to.
# Alpha is given ROOM on both axes, because scenario 11 moves it by +40/+150 and
# then grows it by +60/+40 — a drop that overlaps another fence is REFUSED and
# reverted, which reads exactly like a broken gesture against correct code
# (measured: beta at x 700 refused the grown alpha silently).
FENCE_A = {"left": 40, "top": 40, "width": 600, "height": 460}
FENCE_B = {"left": 800, "top": 40, "width": 320, "height": 300}
FENCE_G = {"left": 800, "top": 400, "width": 320, "height": 260}
FENCE_D = {"left": 40, "top": 760, "width": 320, "height": 200}
FENCE_E = {"left": 420, "top": 760, "width": 320, "height": 200}

# Three members in alpha. All start below the head band (top >= 100) and stop
# well short of the SE grip at (626..640, 486..500).
#
# MEM_3 is an `agent` record, which restores as a PLACEHOLDER — chrome, no PTY,
# and a `.session-reconnect` button. It is what makes scenario 5 discriminating:
# with two live consoles alone, NOTHING in the popup ever renders that button, so
# deleting `canLaunch: false` (the one real guard) left the assertion green.
MEM_1 = {"left": 60, "top": 100, "width": 260, "height": 160}
MEM_2 = {"left": 340, "top": 100, "width": 260, "height": 160}
MEM_3 = {"left": 60, "top": 280, "width": 260, "height": 160}

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
    empty = tempfile.mkdtemp(prefix="wb346_empty_")
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


def make_fixture_repo(tag):
    d = tempfile.mkdtemp(prefix=f"wb346_{tag}_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture {tag}\n\nThe #346 detach fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb346@example.com"],
        ["git", "config", "user.name", "wb346"],
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
    # `kind = "console"` restores as a RELAUNCH — a real PTY, which is what makes
    # "the consoles are live in the popup" and the typing scenario mean anything.
    return (
        "[[windows]]\n"
        f'id = "{wid}"\n'
        f'repo = "{repo}"\n'
        'agent = "console"\n'
        'kind = "console"\n'
        "max = false\n"
        f"ts = {ts}\n"
        f"{rect_toml(rect)}\n\n"
    )


def placeholder_toml(wid, repo, rect, ts):
    """An `agent` record with no live session — restores as a PLACEHOLDER."""
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
    return "[[fences]]\n" f'id = "{fid}"\n' f'name = "{name}"\n' f"ts = {ts}\n" f"{rect_toml(rect)}\n\n"


def write_fixture_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_text(
        window_toml("w-m1", slug, MEM_1, 100)
        + window_toml("w-m2", slug, MEM_2, 101)
        + placeholder_toml("w-m3", slug, MEM_3, 102)
        + fence_toml("f-alpha", "alpha", FENCE_A, 110)
        + fence_toml("f-beta", "beta", FENCE_B, 111)
        + fence_toml("f-gamma", "gamma", FENCE_G, 112)
        + fence_toml("f-delta", "delta", FENCE_D, 113)
        + fence_toml("f-epsilon", "epsilon", FENCE_E, 114),
        encoding="utf-8",
    )


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"],
        cwd=REPO_ROOT,
        check=True,
    )


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


def settle_windows(page, want, timeout=30000):
    """Wait for the windows to be REAL boxes.

    KNOWLEDGE: an `x-show` flip is not visible to the next evaluate, and a
    still-hidden box measures 0x0 — which passes a geometry assertion vacuously.
    """
    page.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=timeout,
    )
    page.wait_for_timeout(500)


def desk_page(ctx, fences=5, windows=3):
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


def click_sel(page, sel):
    """Click a control through the element itself.

    KNOWLEDGE (#309-#338): only the TOPMOST element is hittable, and a fence's
    chrome can sit under a console or off-viewport — `locator.click()` then
    times out or presses the wrong thing. The fence verbs are all driven this way.
    """
    page.evaluate(
        "(s) => { const el = document.querySelector(s);"
        " if (!el) throw new Error('no element for ' + s); el.click(); }",
        sel,
    )


def fence_box(page, fid):
    return page.evaluate(
        "(id) => { const f = document.querySelector(`[data-fence-id='${id}']`);"
        " if (!f) return null;"
        " return { left: f.offsetLeft, top: f.offsetTop,"
        "   width: f.offsetWidth, height: f.offsetHeight,"
        "   name: f.querySelector('.fence-name').value }; }",
        fid,
    )


def boxes(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) => ({"
        "  id: w._deskId,"
        "  left: w.offsetLeft, top: w.offsetTop,"
        "  width: w.offsetWidth, height: w.offsetHeight }))"
    )


def by_id(rows):
    return {r["id"]: r for r in rows}


def members_of(page, fid):
    """The windows whose box lies inside a fence, measured on the PLANE."""
    return page.evaluate(
        "(id) => { const f = document.querySelector(`[data-fence-id='${id}']`);"
        " if (!f) return -1;"
        " const fl = f.offsetLeft, ft = f.offsetTop,"
        "   fr = fl + f.offsetWidth, fb = ft + f.offsetHeight;"
        " return [...document.querySelectorAll('.session-window')].filter((w) =>"
        "   w.offsetLeft >= fl && w.offsetTop >= ft &&"
        "   w.offsetLeft + w.offsetWidth <= fr &&"
        "   w.offsetTop + w.offsetHeight <= fb).length; }",
        fid,
    )


def glyph_visible(page, fid):
    """Gated on geometry, never on presence: a hidden box measures 0 everywhere
    and would satisfy a bare `querySelector` check vacuously (KNOWLEDGE)."""
    return page.evaluate(
        "(id) => { const g = document.querySelector(`[data-fence-id='${id}'] .fence-detached`);"
        " return !!g && !g.hidden && g.offsetParent !== null && g.clientWidth > 0; }",
        fid,
    )


def notice_of(page, fid):
    return page.evaluate(
        "(id) => { const n = document.querySelector(`[data-fence-id='${id}'] .fence-notice`);"
        " return n ? n.textContent.trim() : null; }",
        fid,
    )


def open_fence_list(page):
    close_menus(page)
    page.locator("button[title='draw a fence, or jump to one']").click()
    page.wait_for_function(
        "() => { const m = document.querySelector('.fence-menu');"
        " return m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=8000,
    )
    return page.evaluate(
        "() => [...document.querySelectorAll('.fence-item:not(.fence-new)')].map((r) =>"
        "  r.querySelector('.row-name').textContent.trim())"
    )


def close_menus(page):
    page.evaluate(
        f"() => {{ const s = {SH}; s.fenceMenu = false; s.agentMenu = false; s.windowMenu = false; }}"
    )
    page.wait_for_timeout(150)


def detach(page, fid):
    """Detach a fence and return its popup Page."""
    with page.expect_popup(timeout=15000) as info:
        click_sel(page, f"[data-fence-id='{fid}'] .fence-detach")
    popup = info.value
    popup.wait_for_load_state()
    return popup


def popup_windows(popup, want, timeout=30000):
    popup.wait_for_function(
        "(n) => { const ws = [...document.querySelectorAll('.session-window')];"
        " return ws.length === n && ws.every((w) => w.offsetParent !== null && w.clientWidth > 0); }",
        arg=want,
        timeout=timeout,
    )
    popup.wait_for_timeout(400)


def screen(page, i=0):
    """The i-th console window's whole terminal buffer as text."""
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w._term && w._term.term && w._term.term.buffer.active;"
        " if (!b) return '';"
        " let out = '';"
        " for (let y = 0; y < b.length; y++) {"
        "   const line = b.getLine(y);"
        "   if (line) out += line.translateToString(true) + '\\n';"
        " }"
        " return out; }",
        i,
    )


def type_line(page, i, text):
    """Feed one line through xterm's own data path, as ONE onData event."""
    page.locator(".session-window").nth(i).locator(".xterm").click()
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def reached_child(page, i, token, timeout=20000):
    """A `GOT:` line CONTAINING the token — never equality.

    KNOWLEDGE (#334): after a client reattaches it sends a resize and ConPTY
    repaints its cooked-mode buffer with the PREVIOUS line still in it, so the
    next line typed arrives as `<previous><token>`. Newlines are stripped
    because xterm hard-wraps at the terminal width.
    """
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        buf = screen(page, i)
        if any(token in l for l in buf.split("\n") if l.startswith("GOT:")):
            return True
        if token in buf.replace("\n", "") and "GOT:" in buf:
            return True
        page.wait_for_timeout(300)
    return False


def detached_popups(ctx):
    return [pg for pg in ctx.pages if "detached-fence.html" in pg.url]


def served_fence(fid):
    for f in json.loads(http("GET", "api/desk")[1]).get("fences", []):
        if f.get("id") == fid:
            return f.get("rect")
    return None


def centre_of(page, sel):
    return page.evaluate(
        "(s) => { const e = document.querySelector(s); if (!e) return null;"
        " const r = e.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        sel,
    )


def drag(page, start, dx, dy):
    page.mouse.move(start["x"], start["y"])
    page.mouse.down()
    page.mouse.move(start["x"] + dx / 3, start["y"] + dy / 3, steps=5)
    page.mouse.move(start["x"] + dx * 2 / 3, start["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(start["x"] + dx, start["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(600)


def open_plain_console(page):
    """Open a console through the REAL New-console control, and return its id."""
    before = page.locator(".session-window").count()
    close_menus(page)
    page.locator("button:has-text('New console')").click()
    page.locator(".dropdown-item.is-console:visible").click()
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=20000
    )
    page.locator(".session-window").nth(before).locator(".xterm").wait_for(timeout=25000)
    page.wait_for_timeout(700)
    return page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._deskId", before)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb346_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    slug = register_fixture(daemon_dir, make_fixture_repo("one"))
    write_fixture_desk(daemon_dir, slug)

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
            ctx.on("weberror", lambda e: errors.append(str(e.error)))

            # Every PUT /api/desk this context issues, tagged with the page that
            # issued it — scenario 4's oracle for "the popup writes nothing".
            puts = []

            def on_request(req):
                if req.method == "PUT" and "/api/desk" in req.url:
                    try:
                        who = req.frame.page
                    except Exception:
                        who = None
                    puts.append(who)

            ctx.on("request", on_request)

            page = desk_page(ctx)

            # ---- scenario 1: the fence chrome carries the detach verb ---------
            geom = page.evaluate(
                "() => { const f = document.querySelector(\"[data-fence-id='f-alpha']\");"
                " const q = (c) => { const e = f.querySelector(c);"
                "   if (!e) return null; const r = e.getBoundingClientRect();"
                "   return { left: r.left, right: r.right, top: r.top,"
                "     cx: r.left + r.width / 2, cy: r.top + r.height / 2, w: r.width }; };"
                " const fr = f.getBoundingClientRect();"
                " return { arrange: q('.fence-arrange'), detach: q('.fence-detach'),"
                "   drop: q('.fence-drop'), fence: { top: fr.top, right: fr.right } }; }"
            )
            check(
                "the fence chrome carries a detach control",
                geom["detach"] is not None and geom["detach"]["w"] > 0,
                f"detach={geom['detach']}",
            )
            check(
                "…placed BETWEEN arrange and close (close stays the outermost, #342)",
                geom["arrange"]["right"] <= geom["detach"]["left"] + 1
                and geom["detach"]["right"] <= geom["drop"]["left"] + 1,
                f"arrangeR={geom['arrange']['right']:.0f} detachL={geom['detach']['left']:.0f}"
                f" detachR={geom['detach']['right']:.0f} dropL={geom['drop']['left']:.0f}",
            )
            check(
                "…in the fence's TOP-RIGHT corner",
                geom["detach"]["top"] - geom["fence"]["top"] < 40
                and geom["fence"]["right"] - geom["detach"]["right"] < 60,
                f"dTop-fTop={geom['detach']['top'] - geom['fence']['top']:.0f}"
                f" fRight-dRight={geom['fence']['right'] - geom['detach']['right']:.0f}",
            )
            # `pointer-events` is the trap: `.fence-tools` is transparent to
            # them, so a control that does not opt back IN is drawn and
            # unclickable while every source pin stays green.
            hit = page.evaluate(
                "() => { const e = document.querySelector(\"[data-fence-id='f-alpha'] .fence-detach\");"
                " const r = e.getBoundingClientRect();"
                " const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                " return at ? (at.closest('.fence-detach') ? 'fence-detach' : at.className) : null; }"
            )
            check(
                "…and it is really HITTABLE there — elementFromPoint answers the button",
                hit == "fence-detach",
                f"elementFromPoint={hit!r}",
            )

            # ---- scenario 2: the detach round trip ---------------------------
            before_boxes = by_id(boxes(page))
            before_fence = fence_box(page, "f-alpha")
            # The control for scenario 5: on the SHELL this placeholder does
            # carry `.session-reconnect`. Sampled here, because once the fence is
            # detached the shell holds no member at all and a 0 there would be
            # trivially true.
            shell_reconnect_before = page.evaluate(
                "() => document.querySelectorAll('.session-reconnect').length"
            )
            before_list = open_fence_list(page)
            close_menus(page)
            quiet(desk_file)
            desk_before = desk_file.read_bytes()
            page_bg = page.evaluate("() => getComputedStyle(document.body).backgroundColor")

            popup = detach(page, "f-alpha")
            popup_windows(popup, 3)

            check(
                "the popup carries the fence's name in its title",
                popup.title() == "alpha · ralphy",
                f"title={popup.title()!r}",
            )
            check(
                "…and the workbench's own styling",
                popup.evaluate("() => getComputedStyle(document.body).backgroundColor") == page_bg,
                f"popup={popup.evaluate('() => getComputedStyle(document.body).backgroundColor')!r}"
                f" shell={page_bg!r}",
            )
            live = popup.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .filter((w) => w.querySelector('.xterm-screen')).length"
            )
            placed = popup.evaluate(
                "() => { const ws = [...document.querySelectorAll('.session-window')];"
                " const vw = document.documentElement.clientWidth,"
                "   vh = document.documentElement.clientHeight;"
                " return { onscreen: ws.filter((w) => w.offsetLeft >= 0 && w.offsetTop >= 0"
                "     && w.offsetLeft < vw && w.offsetTop < vh).length,"
                "   distinct: new Set(ws.map((w) => w.offsetLeft + ',' + w.offsetTop)).size }; }"
            )
            check(
                "mountDetached translates every member INTO the popup's own viewport,"
                " without collapsing them onto one point",
                placed == {"onscreen": 3, "distinct": 3},
                f"got={placed}",
            )
            check(
                "…holding this fence's consoles, LIVE",
                live == 2,
                f"windows-with-a-terminal={live} (of 3 members; w-m3 is a placeholder)",
            )
            after_fence = fence_box(page, "f-alpha")
            check(
                "the origin fence keeps its name and its exact rect",
                after_fence == before_fence,
                f"before={before_fence} after={after_fence}",
            )
            check(
                "…renders no member window",
                members_of(page, "f-alpha") == 0,
                f"members={members_of(page, 'f-alpha')}",
            )
            check(
                "…and shows the detach glyph in its middle",
                glyph_visible(page, "f-alpha"),
                "hidden or zero-sized" if not glyph_visible(page, "f-alpha") else "",
            )
            after_list = open_fence_list(page)
            close_menus(page)
            check(
                "…while keeping its place in the fence list",
                after_list == before_list and "alpha" in after_list,
                f"before={before_list} after={after_list}",
            )

            # ---- scenario 4 (measured here, asserted below): popup-local drag -
            # Driven BEFORE the close, on the same popup: the discard is only
            # meaningful against a layout the popup actually changed.
            was = popup.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " return { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth }; }"
            )
            moved = popup.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " w.style.left = (w.offsetLeft + 120) + 'px';"
                " w.style.top = (w.offsetTop + 80) + 'px';"
                " w.style.width = (w.offsetWidth + 60) + 'px';"
                " return { left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth }; }"
            )
            popup.wait_for_timeout(400)
            drag_bar = popup.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[1];"
                " const r = w.querySelector('.session-titlebar').getBoundingClientRect();"
                " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }"
            )
            popup.mouse.move(drag_bar["x"], drag_bar["y"])
            popup.mouse.down()
            popup.mouse.move(drag_bar["x"] + 40, drag_bar["y"] + 50, steps=8)
            popup.mouse.move(drag_bar["x"] + 80, drag_bar["y"] + 90, steps=8)
            popup.mouse.up()
            popup.wait_for_timeout(600)
            check(
                "a console can be moved and resized inside the popup",
                (moved["left"] - was["left"], moved["top"] - was["top"], moved["width"] - was["width"])
                == (120, 80, 60),
                f"was={was} moved={moved}",
            )
            quiet(desk_file)
            desk_after_popup = desk_file.read_bytes()
            check(
                "NOT ONE PUT /api/desk is issued from the popup context",
                all(w is not popup for w in puts) and None not in puts,
                f"puts-from-popup={sum(1 for w in puts if w is popup)}"
                f" unattributable={puts.count(None)} total={len(puts)}",
            )
            check(
                "…and desk.toml is byte-identical across the detach and the popup's own layout",
                desk_after_popup == desk_before,
                f"{len(desk_before)}B -> {len(desk_after_popup)}B",
            )

            # ---- scenario 5: the popup offers no way to open a console -------
            # DISCRIMINATING because of MEM_3: the popup renders a PLACEHOLDER,
            # which is the one window that carries `.session-reconnect` — the
            # single control `canLaunch: false` actually gates. The shell's own
            # placeholder is asserted to still HAVE the button, so this measures
            # the guard rather than the absence of the markup.
            popup_placeholders = popup.evaluate(
                "() => document.querySelectorAll('.session-window.placeholder').length"
            )
            launchers = popup.evaluate(
                "() => ({"
                "  reconnect: document.querySelectorAll('.session-reconnect').length,"
                "  newConsole: document.querySelectorAll('[data-act=\"new-console\"]').length,"
                "  agents: document.querySelectorAll('.agent-item').length,"
                "  byText: [...document.querySelectorAll('button')]"
                "    .filter((b) => /new console/i.test(b.textContent)).length })"
            )
            check(
                "the popup really renders the placeholder that carries the relaunch button",
                popup_placeholders == 1,
                f"placeholders-in-popup={popup_placeholders}",
            )
            check(
                "…and the SHELL renders that same placeholder WITH its relaunch button"
                " — the negative control, sampled before the detach",
                shell_reconnect_before == 1,
                f"shell-reconnect-before-detach={shell_reconnect_before}",
            )
            check(
                "the popup exposes no way to open a new console",
                launchers == {"reconnect": 0, "newConsole": 0, "agents": 0, "byText": 0},
                f"got={launchers}",
            )

            # ---- scenario 3a: the glyph FOCUSES a living popup ----------------
            page.bring_to_front()
            click_sel(page, "[data-fence-id='f-alpha'] .fence-detached")
            page.wait_for_timeout(600)
            check(
                "clicking the glyph while the popup lives does NOT bring the consoles home",
                (not popup.is_closed()) and members_of(page, "f-alpha") == 0,
                f"popup-closed={popup.is_closed()} members={members_of(page, 'f-alpha')}",
            )
            check(
                "…and the fence still shows its glyph",
                glyph_visible(page, "f-alpha"),
                "",
            )

            # ---- scenario 2 (cont.): closing the popup brings them home ------
            popup.close()
            settle_windows(page, 3)
            page.wait_for_timeout(600)
            check(
                "closing the popup re-attaches automatically",
                members_of(page, "f-alpha") == 3,
                f"members={members_of(page, 'f-alpha')}",
            )
            check(
                "…and the glyph is gone",
                not glyph_visible(page, "f-alpha"),
                "",
            )
            back = by_id(boxes(page))
            same = {k: back.get(k) == before_boxes.get(k) for k in before_boxes}
            check(
                "every console returns to the EXACT box it was detached from —"
                " the popup's own layout is discarded",
                all(same.values()),
                f"before={before_boxes} after={back}",
            )
            # GEOMETRY IS NOT LIVENESS. `settle_windows` is satisfied by a
            # placeholder or a permanently parked window, so without these three
            # a `reattachFence` that lost `m.session` and respawned placeholders,
            # or one whose consoles never re-acquire the writer slot, passes the
            # whole round trip. "…and comes home" is about terminals, not rects.
            home = page.evaluate(
                "() => ({"
                "  terms: [...document.querySelectorAll('.session-window')]"
                "    .filter((w) => w.querySelector('.xterm-screen')).length,"
                "  placeholders: document.querySelectorAll('.session-window.placeholder').length,"
                "  parked: [...document.querySelectorAll('.session-parked')]"
                "    .filter((e) => e.offsetParent !== null && e.clientWidth > 0).length })"
            )
            check(
                "the consoles come home ALIVE — two live terminals, one placeholder, none parked",
                home == {"terms": 2, "placeholders": 1, "parked": 0},
                f"got={home}",
            )
            # By IDENTITY, not by index: the placeholder carries no `.xterm`, and
            # the spawn order is the membership fold's, not the fixture's.
            live_i = page.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .findIndex((w) => w.querySelector('.xterm-screen'))"
            )
            type_line(page, live_i, "probe-home-346")
            check(
                "…and a returned console still reaches its child",
                live_i >= 0 and reached_child(page, live_i, "probe-home-346"),
                f"i={live_i} buffer={screen(page, max(live_i, 0))[-160:]!r}",
            )

            # ---- scenario 3b: the glyph with no popup left is a safe no-op ----
            # `reattachFence` is the ONE function both the glyph and the
            # opener's closed-poll call, and the fold makes the second call
            # inert — this is that idempotence, observed in the browser.
            click_sel(page, "[data-fence-id='f-alpha'] .fence-detached")
            page.wait_for_timeout(500)
            after_noop = by_id(boxes(page))
            check(
                "a glyph click with no popup left leaves the consoles exactly where they are",
                members_of(page, "f-alpha") == 3 and after_noop == back,
                f"members={members_of(page, 'f-alpha')} boxes-changed={after_noop != back}",
            )

            # The rects survived the whole round trip. NOT a byte comparison:
            # `persistWin` stamps `ts: Date.now()` on a re-attached window, so
            # the file legitimately differs while every rect is unchanged.
            served = {
                w["id"]: w["rect"]
                for w in json.loads(http("GET", "api/desk")[1]).get("windows", [])
            }
            check(
                "the served desk still holds both members at their original rects",
                served.get("w-m1") == MEM_1 and served.get("w-m2") == MEM_2,
                f"served={served}",
            )

            # ---- scenario 6: a popup with no valid opener renders nothing -----
            orphan = ctx.new_page()
            orphan.goto(BASE + "detached-fence.html")
            orphan.wait_for_selector(".detached-empty", timeout=8000)
            text = orphan.locator(".detached-empty").inner_text()
            check(
                "a popup with no opener says why it is empty",
                "detach a fence from the workbench" in text,
                f"text={text!r}",
            )
            check(
                "…and renders not one console",
                orphan.locator(".session-window").count() == 0,
                f"windows={orphan.locator('.session-window').count()}",
            )
            orphan.close()

            # ---- scenario 6b: the opener's guards reject a message it did not earn -
            # A synthetic `message` event, because a separate TAB shares no handle
            # with the shell and so cannot reach this listener at all. Both legs
            # carry the re-attach verb for a fence that IS detached, so a guard
            # that let them through would visibly bring the consoles home.
            popup2 = detach(page, "f-alpha")
            popup_windows(popup2, 3)
            page.evaluate(
                "() => { window.dispatchEvent(new MessageEvent('message', {"
                "  origin: location.origin, source: null,"
                "  data: { type: 'wb-fence-reattach', fenceId: 'f-alpha' } })); }"
            )
            page.evaluate(
                "() => { window.dispatchEvent(new MessageEvent('message', {"
                "  origin: 'https://evil.example', source: window,"
                "  data: { type: 'wb-fence-reattach', fenceId: 'f-alpha' } })); }"
            )
            page.wait_for_timeout(800)
            check(
                "a message from an unknown source, or a foreign origin, drives no re-attach",
                (not popup2.is_closed())
                and members_of(page, "f-alpha") == 0
                and glyph_visible(page, "f-alpha"),
                f"popup-closed={popup2.is_closed()} members={members_of(page, 'f-alpha')}",
            )
            popup2.close()
            settle_windows(page, 3)
            page.wait_for_timeout(600)

            # ---- scenario 7: at most four popups ------------------------------
            capped = [detach(page, fid) for fid in ("f-beta", "f-gamma", "f-delta", "f-epsilon")]
            page.wait_for_timeout(600)
            check(
                "four fences detach into four popups",
                len(detached_popups(ctx)) == 4 and all(not q.is_closed() for q in capped),
                f"popups={len(detached_popups(ctx))}",
            )
            # The FIFTH. No popup is expected, so this is a bare click — an
            # `expect_popup` here would time out on correct code.
            click_sel(page, "[data-fence-id='f-alpha'] .fence-detach")
            page.wait_for_timeout(900)
            check(
                "…and the fifth is refused rather than opened",
                len(detached_popups(ctx)) == 4,
                f"popups={len(detached_popups(ctx))}",
            )
            check(
                "…with the refusal said ON the fence",
                notice_of(page, "f-alpha") == "at most 4 detached fences",
                f"notice={notice_of(page, 'f-alpha')!r}",
            )
            check(
                "…and NOTHING of the refused fence is torn down",
                members_of(page, "f-alpha") == 3 and not glyph_visible(page, "f-alpha"),
                f"members={members_of(page, 'f-alpha')} glyph={glyph_visible(page, 'f-alpha')}",
            )
            for q in capped:
                q.close()
            page.wait_for_function(
                "() => [...document.querySelectorAll('.fence-detached')]"
                "  .every((g) => g.hidden)",
                timeout=15000,
            )
            page.wait_for_timeout(400)

            # ---- scenario 8: a popup the BROWSER blocks ----------------------
            page.evaluate("() => { window.__realOpen = window.open; window.open = () => null; }")
            click_sel(page, "[data-fence-id='f-beta'] .fence-detach")
            page.wait_for_timeout(900)
            check(
                "a popup the browser blocks tells the operator",
                notice_of(page, "f-beta") == "the browser blocked the popup",
                f"notice={notice_of(page, 'f-beta')!r}",
            )
            check(
                "…and leaves the fence exactly as it was",
                not glyph_visible(page, "f-beta") and len(detached_popups(ctx)) == 0,
                f"glyph={glyph_visible(page, 'f-beta')} popups={len(detached_popups(ctx))}",
            )
            page.evaluate("() => { window.open = window.__realOpen; }")

            # ---- scenario 9: the detached console is DRIVEABLE ----------------
            popup = detach(page, "f-alpha")
            popup_windows(popup, 3)
            type_line(popup, 0, "probe-346")
            check(
                "a session the origin tab was driving is driveable in the popup",
                reached_child(popup, 0, "probe-346"),
                f"buffer={screen(popup, 0)[-200:]!r}",
            )

            # ---- scenario 10: a session ANOTHER client drives ------------------
            # A second browser context takes the writer slot; the popup must then
            # show the existing explicit take-over rather than stealing it back.
            ctx_b = browser.new_context(viewport=dict(VIEW))
            page_b = desk_page(ctx_b, windows=3)
            page_b.wait_for_timeout(1500)
            if page_b.locator('[data-act="take-over"]').count():
                page_b.locator('[data-act="take-over"]').first.click()
                page_b.wait_for_timeout(2500)
            popup.wait_for_function(
                "() => [...document.querySelectorAll('.session-window')]"
                "  .some((w) => { const p = w.querySelector('.session-parked');"
                "    return p && p.offsetParent !== null && p.clientWidth > 0; })",
                timeout=25000,
            )
            took = popup.evaluate(
                "() => { const b = document.querySelector('[data-act=\"take-over\"]');"
                " return !!b && b.offsetParent !== null && b.clientWidth > 0"
                "   && /take over/i.test(b.textContent); }"
            )
            check(
                "a session another client drives arrives in the popup PARKED,"
                " with the explicit take-over",
                took,
                f"take-over-visible={took}",
            )

            # ---- scenario 13: a second client renders the fence normally -------
            # The detach is per-tab: another browser sees the fence with its
            # consoles inside it, and no glyph.
            inside_b = page_b.evaluate(
                "() => { const f = document.querySelector(\"[data-fence-id='f-alpha']\");"
                " const fl = f.offsetLeft, ft = f.offsetTop,"
                "   fr = fl + f.offsetWidth, fb = ft + f.offsetHeight;"
                " return [...document.querySelectorAll('.session-window')].filter((w) => {"
                "   const cx = w.offsetLeft + w.offsetWidth / 2,"
                "     cy = w.offsetTop + w.offsetHeight / 2;"
                "   return cx >= fl && cx <= fr && cy >= ft && cy <= fb; }).length; }"
            )
            glyphs_b = page_b.evaluate(
                "() => [...document.querySelectorAll('.fence-detached')]"
                "  .filter((g) => !g.hidden).length"
            )
            check(
                "a second browser context renders the fence WITH its consoles inside it",
                inside_b == 3 and glyphs_b == 0,
                f"inside={inside_b} visible-glyphs={glyphs_b}",
            )
            page_b.screenshot(path=SHOT, full_page=False)

            # ---- scenario 12: a console born into the focused detached fence ---
            page.bring_to_front()
            walked = []
            for _ in range(6):
                if page.evaluate("() => window.WBConsole.focusedFence()") == "f-alpha":
                    break
                page.keyboard.press("Alt+Shift+ArrowRight")
                page.wait_for_timeout(500)
                walked.append(page.evaluate("() => window.WBConsole.focusedFence()"))
            check(
                "Alt+Shift+→ still reaches a DETACHED fence",
                page.evaluate("() => window.WBConsole.focusedFence()") == "f-alpha",
                f"walk={walked}",
            )
            check(
                "…and it is still listed in the toolbar's fence map while detached",
                "alpha" in open_fence_list(page),
                "",
            )
            close_menus(page)
            born = open_plain_console(page)
            fb = fence_box(page, "f-alpha")
            born_box = by_id(boxes(page)).get(born)
            check(
                "a console born while a detached fence is focused is born IN it, on the plane",
                born_box is not None
                and fb["left"] <= born_box["left"] + born_box["width"] / 2 <= fb["left"] + fb["width"]
                and fb["top"] <= born_box["top"] + born_box["height"] / 2 <= fb["top"] + fb["height"],
                f"born={born_box} fence={fb}",
            )

            # ---- scenario 11: the detached fence still moves and resizes -------
            before_rect = served_fence("f-alpha")
            grab = centre_of(page, "[data-fence-id='f-alpha'] .fence-grab")
            drag(page, grab, 40, 150)
            quiet(desk_file)
            after_rect = served_fence("f-alpha")
            check(
                "a detached fence still MOVES on the plane, by exactly the drag's delta",
                after_rect
                and (
                    round(after_rect["left"] - before_rect["left"]),
                    round(after_rect["top"] - before_rect["top"]),
                )
                == (40, 150),
                f"before={before_rect} after={after_rect}",
            )
            grip = centre_of(page, "[data-fence-id='f-alpha'] .fence-grip")
            grip_diag = page.evaluate(
                "() => { const g = document.querySelector(\"[data-fence-id='f-alpha'] .fence-grip\");"
                " if (!g) return 'no grip';"
                " const r = g.getBoundingClientRect();"
                " const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                " return { rect: { l: r.left, t: r.top, w: r.width, h: r.height },"
                "   hit: at ? (at.className && at.className.baseVal !== undefined"
                "     ? 'svg' : String(at.className)) : null,"
                "   inner: window.innerHeight, dir: g.dataset.dir }; }"
            )
            drag(page, grip, 60, 40)
            quiet(desk_file)
            grown = served_fence("f-alpha")
            check(
                "…and still RESIZES",
                grown
                and grown["width"] > after_rect["width"]
                and grown["height"] > after_rect["height"],
                f"after-move={after_rect} after-resize={grown} grip={grip} diag={grip_diag}",
            )

            # ---- scenario 11b: arrange is a NO-OP on a detached fence -----------
            # Sharper than an empty-fence no-op: the console born in scenario 12
            # IS a member on the plane, so without the guard it would be tiled.
            quiet(desk_file)
            desk_before_arrange = desk_file.read_bytes()
            born_before = by_id(boxes(page)).get(born)
            click_sel(page, "[data-fence-id='f-alpha'] .fence-arrange")
            page.wait_for_timeout(1200)
            quiet(desk_file)
            check(
                "arrange on a detached fence writes NOTHING",
                desk_file.read_bytes() == desk_before_arrange,
                f"{len(desk_before_arrange)}B -> {len(desk_file.read_bytes())}B",
            )
            check(
                "…and does not move the member it can still see",
                by_id(boxes(page)).get(born) == born_before,
                f"before={born_before} after={by_id(boxes(page)).get(born)}",
            )

            # ---- and the returned consoles are UNTILED --------------------------
            popup.close()
            settle_windows(page, 4)
            page.wait_for_timeout(800)
            returned = by_id(boxes(page))
            check(
                "closing the popup returns the consoles UNTILED, at their original boxes",
                all(returned.get(k) == before_boxes.get(k) for k in before_boxes),
                f"before={before_boxes} returned={{k: returned.get(k) for k in before_boxes}}",
            )

            ctx_b.close()
            check("no page error was raised by the whole pass", errors == [], f"weberrors={errors}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 49
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("A FENCE DETACHES INTO ITS OWN WINDOW, AND COMES HOME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
