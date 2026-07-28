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
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "346-a-fence-detaches-into-its-own-window-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}

# The fixture geometry, in stage coordinates. NO TWO FENCES OVERLAP — an
# overlapping pair would make `fenceMembership`'s tie-break, not the detach,
# decide which fence a member belongs to.
FENCE_A = {"left": 40, "top": 40, "width": 600, "height": 460}
FENCE_B = {"left": 700, "top": 40, "width": 320, "height": 300}
FENCE_G = {"left": 1100, "top": 40, "width": 320, "height": 300}
FENCE_D = {"left": 40, "top": 560, "width": 320, "height": 260}
FENCE_E = {"left": 420, "top": 560, "width": 320, "height": 260}

# Two members in alpha. Both start below the head band (top >= 100) and stop
# well short of the SE grip at (626..640, 486..500).
MEM_1 = {"left": 60, "top": 100, "width": 260, "height": 160}
MEM_2 = {"left": 340, "top": 100, "width": 260, "height": 160}

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


def fence_toml(fid, name, rect, ts):
    return "[[fences]]\n" f'id = "{fid}"\n' f'name = "{name}"\n' f"ts = {ts}\n" f"{rect_toml(rect)}\n\n"


def write_fixture_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_text(
        window_toml("w-m1", slug, MEM_1, 100)
        + window_toml("w-m2", slug, MEM_2, 101)
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


def desk_page(ctx, fences=5, windows=2):
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
            before_list = open_fence_list(page)
            close_menus(page)
            quiet(desk_file)
            desk_before = desk_file.read_bytes()
            page_bg = page.evaluate("() => getComputedStyle(document.body).backgroundColor")

            popup = detach(page, "f-alpha")
            popup_windows(popup, 2)

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
            check(
                "…holding this fence's consoles, LIVE",
                live == 2,
                f"windows-with-a-terminal={live}",
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
                moved["left"] > 0 and moved["width"] > 0,
                f"moved={moved}",
            )
            quiet(desk_file)
            desk_after_popup = desk_file.read_bytes()
            check(
                "NOT ONE PUT /api/desk is issued from the popup context",
                all(w is not popup for w in puts),
                f"puts-from-popup={sum(1 for w in puts if w is popup)} total={len(puts)}",
            )
            check(
                "…and desk.toml is byte-identical across the detach and the popup's own layout",
                desk_after_popup == desk_before,
                f"{len(desk_before)}B -> {len(desk_after_popup)}B",
            )

            # ---- scenario 5: the popup offers no way to open a console -------
            launchers = popup.evaluate(
                "() => ({"
                "  reconnect: document.querySelectorAll('.session-reconnect').length,"
                "  newConsole: document.querySelectorAll('[data-act=\"new-console\"]').length,"
                "  agents: document.querySelectorAll('.agent-item').length,"
                "  byText: [...document.querySelectorAll('button')]"
                "    .filter((b) => /new console/i.test(b.textContent)).length })"
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
            settle_windows(page, 2)
            page.wait_for_timeout(600)
            check(
                "closing the popup re-attaches automatically",
                members_of(page, "f-alpha") == 2,
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

            # ---- scenario 3b: the glyph with no popup left is a safe no-op ----
            # `reattachFence` is the ONE function both the glyph and the
            # opener's closed-poll call, and the fold makes the second call
            # inert — this is that idempotence, observed in the browser.
            gone = page.evaluate(
                "() => { const g = document.querySelector(\"[data-fence-id='f-alpha'] .fence-detached\");"
                " g.click(); return true; }"
            )
            page.wait_for_timeout(500)
            check(
                "a glyph click with no popup left leaves the consoles exactly where they are",
                gone and members_of(page, "f-alpha") == 2 and by_id(boxes(page)) == back,
                f"members={members_of(page, 'f-alpha')}",
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

            check("no page error was raised by the whole pass", errors == [], f"weberrors={errors}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor is the REAL count, not a loose lower bound: set under the total,
    # a whole scenario could stop running while the suite still exits 0.
    ok = all(results) and len(results) == 26
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("A FENCE DETACHES INTO ITS OWN WINDOW, AND COMES HOME")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
