"""#340 browser acceptance: a fence exists end to end.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The `desk.toml` fixture is
written BEFORE the daemon starts, in the PRE-#340 shape (a `[[windows]]`-only
file with no `fences` key at all), so "an existing desk loads unchanged" is
proved against a real legacy file rather than against one this build wrote.

Scenario 1   `GET /api/desk` is an OBJECT with exactly `windows` and `fences`;
             the legacy window survives verbatim and the fence list is empty
Scenario 2   the toolbar `Fence` button draws two fences at the current view,
             disjoint and usable, and each is renamed IN PLACE
Scenario 3   the floor tier: a fence sits under every window, takes no pointer
             events, and swallows neither a drag, a resize, a focus click nor
             the floor's own pan
Scenario 4   the fences reached the daemon's store as their own `[[fences]]`
             records, with the names and rects the screen shows
Scenario 5   a FRESH browser profile re-measures both fences dict-equal — this
             is where the evidence screenshot is taken
Scenario 5b  the `×` removes a fence from the screen AND from the store
Scenario 5c  a fence seeded far out SIZES the plane with no window beside it
Scenario 5d  a page whose desk GET was REFUSED neither overwrites the saved
             fences nor discards them once the read succeeds again
Scenario 6   the daemon enforces the fence cap (13 in → `f2..f13`) and refuses an
             off-plane origin without touching the store
Scenario 7   a corrupt `desk.toml` degrades to an EMPTY stage, not a failure

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/340-fence-end-to-end-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_fence_340.py   (exit 0 = all pass)
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

PORT = 7435
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "340-fence-end-to-end-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

VIEW = {"width": 1400, "height": 900}
# The fixture window, in the shape a PRE-#340 shell wrote. `kind = "agent"`
# restores as a PLACEHOLDER: full chrome, deterministic geometry, no PTY and no
# vendor CLI spawned.
FIX_A = {"left": 40, "top": 620, "width": 600, "height": 380}

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
    empty = tempfile.mkdtemp(prefix="wb340_empty_")
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
    d = tempfile.mkdtemp(prefix="wb340_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #340 fence fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb340@example.com"],
        ["git", "config", "user.name", "wb340"],
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
    """The pre-#340 `desk.toml`: windows only, no `fences` key anywhere."""
    rect = (
        "rect = { left = %(left)s, top = %(top)s,"
        " width = %(width)s, height = %(height)s }" % FIX_A
    )
    Path(daemon_dir, "desk.toml").write_text(
        "[[windows]]\n"
        'id = "w-fixture-a"\n'
        f'repo = "{slug}"\n'
        'agent = "claude"\n'
        'kind = "agent"\n'
        "max = false\n"
        "ts = 100\n"
        f"{rect}\n",
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


def fence_dom(page):
    """Every fence on the stage, as plain data — one evaluate, no DOM handles."""
    return page.evaluate(
        "() => [...document.querySelectorAll('.fence')].map((f) => ({"
        "  id: f.dataset.fenceId,"
        "  name: f.querySelector('.fence-name').value,"
        "  left: f.offsetLeft, top: f.offsetTop,"
        "  width: f.offsetWidth, height: f.offsetHeight }))"
    )


def settle_windows(page, want):
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


def desk_page(ctx, viewport=None):
    page = ctx.new_page()
    page.set_viewport_size(viewport or VIEW)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    # `activate` and not a raw `active =` write: only the former reaches
    # `refitAll()`, the path that re-applies a stored offset after `display:none`
    # threw the scroll position away (KNOWLEDGE, #339).
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_timeout(1800)
    return page


def drag(page, start, dx, dy):
    page.mouse.move(start["x"], start["y"])
    page.mouse.down()
    page.mouse.move(start["x"] + dx / 3, start["y"] + dy / 3, steps=5)
    page.mouse.move(start["x"] + dx * 2 / 3, start["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(start["x"] + dx, start["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(400)


def bare_fence_point(page, index=1):
    """The centre of the VISIBLE part of a fence, in client coordinates.

    A fence is wider than the viewport shows (the sidebar pushes `#workspace`
    right), so the fence's own centre can sit off-screen — where
    `elementFromPoint` answers null and a mouse drag lands nowhere. Intersect
    with the viewport first; `w`/`h` let the caller prove the point is real.
    """
    return page.evaluate(
        "(i) => { const ws = document.getElementById('workspace');"
        "  const wr = ws.getBoundingClientRect();"
        "  const r = document.querySelectorAll('.fence')[i].getBoundingClientRect();"
        "  const left = Math.max(wr.left, r.left), right = Math.min(wr.right, r.right);"
        "  const top = Math.max(wr.top, r.top), bottom = Math.min(wr.bottom, r.bottom);"
        "  return { x: (left + right) / 2, y: (top + bottom) / 2,"
        "    w: right - left, h: bottom - top }; }",
        index,
    )


def centre_of(page, selector, index=0):
    return page.evaluate(
        "([sel, i]) => { const el = document.querySelectorAll(sel)[i];"
        " const r = el.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        [selector, index],
    )


def fence_json(fid, name, ts, left=40.0):
    return {
        "id": fid,
        "name": name,
        "rect": {"left": left, "top": 40.0, "width": 720.0, "height": 460.0},
        "ts": ts,
    }


def draw_fence(page):
    """Draw a fence through the MERGED toolbar control (ADR-0051 §7 amendment).

    `Fence` is one button now: it opens a menu whose first row is the create
    verb and whose remaining rows are the map. Clicking the row closes the menu,
    so every call re-opens it — there is no stale-menu path to reuse.
    """
    page.locator("button[title='draw a fence, or jump to one']").click()
    page.wait_for_function(
        "() => { const m = document.querySelector('.fence-menu');"
        " return m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=8000,
    )
    page.locator("button[title='draw a named fence on the plane']").click()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb340_reg_")
    desk_file = Path(daemon_dir, "desk.toml")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_legacy_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        # ===== scenario 1: the wire shape over a pre-#340 file ================
        status, body = http("GET", "api/desk")
        served = json.loads(body) if status == 200 else {}
        check(
            "GET /api/desk answers an object with exactly windows and fences",
            status == 200 and sorted(served.keys()) == ["fences", "windows"],
            f"status={status} got={body[:200]}",
        )
        check(
            "a hand-written pre-#340 desk.toml loads its window verbatim",
            [w["rect"] for w in served.get("windows", [])] == [FIX_A],
            f"got={served.get('windows')}",
        )
        check(
            "…and reads as having no fences, not as a corrupt desk",
            served.get("fences") == [],
            f"got={served.get('fences')}",
        )

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport=dict(VIEW))

            # ===== scenario 2: draw two fences and name them =================
            page = desk_page(ctx)
            settle_windows(page, 1)
            check(
                "the pre-#340 desk restores its window and draws no fence",
                page.evaluate("() => document.querySelectorAll('.session-window').length") == 1
                and len(fence_dom(page)) == 0,
            )

            draw_fence(page)
            page.wait_for_timeout(300)
            draw_fence(page)
            page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 2", timeout=10000
            )
            page.wait_for_timeout(300)
            drawn = fence_dom(page)
            check("the toolbar Fence button draws a fence per click", len(drawn) == 2, f"got={drawn}")

            a, b = drawn
            check(
                "the two fences are DISJOINT — no overlap ships before the invariant does",
                a["left"] + a["width"] <= b["left"] or b["left"] + b["width"] <= a["left"],
                f"a={a} b={b}",
            )
            check(
                "each fence is a usable region, not a sliver",
                a["width"] >= 240 and a["height"] >= 150 and b["width"] >= 240,
                f"a={a['width']}x{a['height']} b={b['width']}x{b['height']}",
            )
            anchor = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " return { left: ws.scrollLeft, top: ws.scrollTop }; }"
            )
            check(
                "a fence is born where the operator is LOOKING, not at the stage origin",
                a["left"] == anchor["left"] + 40 and a["top"] == anchor["top"] + 40,
                f"offset={anchor} fence={a['left']},{a['top']}",
            )

            # Renamed IN PLACE: type into the live input, Enter commits.
            for i, name in enumerate(("backend", "planning")):
                field = page.locator(".fence .fence-name").nth(i)
                field.fill(name)
                field.press("Enter")
                page.wait_for_timeout(250)
            page.wait_for_timeout(600)
            named = fence_dom(page)
            check(
                "both fences carry the names typed into them",
                [f["name"] for f in named] == ["backend", "planning"],
                f"got={[f['name'] for f in named]}",
            )

            # ===== scenario 3: the floor tier ================================
            tier = page.evaluate(
                "() => { const f = document.querySelector('.fence');"
                " const w = document.querySelector('.session-window');"
                " return { fenceZ: getComputedStyle(f).zIndex,"
                "   fenceEvents: getComputedStyle(f).pointerEvents,"
                "   nameEvents: getComputedStyle(f.querySelector('.fence-name')).pointerEvents,"
                "   winZ: parseInt(w.style.zIndex, 10) || 0 }; }"
            )
            check(
                "a fence draws BELOW every console window",
                tier["fenceZ"] == "1" and tier["winZ"] >= 60,
                f"fence z={tier['fenceZ']} window z={tier['winZ']}",
            )
            check(
                "the fence floor takes no pointer events…",
                tier["fenceEvents"] == "none",
                f"got={tier['fenceEvents']}",
            )
            check(
                "…while its name field stays clickable, so it can be renamed in place",
                tier["nameEvents"] == "auto",
                f"got={tier['nameEvents']}",
            )

            # Fence 1 on purpose: the restored window sits over fence 0, and a
            # point where a WINDOW covers the fence must hit the window — that is
            # the tier working, not failing. This probes BARE fence floor.
            spot = bare_fence_point(page, 1)
            check(
                "the fence has bare floor on screen to probe",
                spot["w"] > 20 and spot["h"] > 20,
                f"visible={spot['w']}x{spot['h']}",
            )
            hit = page.evaluate(
                "(p) => { const el = document.elementFromPoint(p.x, p.y);"
                " return { id: el && el.id, cls: el && el.className,"
                "   overWindow: !!(el && el.closest && el.closest('.session-window')) }; }",
                spot,
            )
            check(
                "a press on bare fence floor reaches the STAGE, not the fence",
                hit["id"] == "stage" and not hit["overWindow"],
                f"elementFromPoint gave id={hit['id']} class={hit['cls']}",
            )

            # A window dragged BY ITS TITLEBAR across the fence must move exactly
            # as far as the pointer did.
            page.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " const f = document.querySelector('.fence');"
                " w.style.left = f.offsetLeft + 60 + 'px'; w.style.top = f.offsetTop + 60 + 'px'; }"
            )
            page.wait_for_timeout(200)
            before = page.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " return { left: w.offsetLeft, top: w.offsetTop,"
                "   width: w.offsetWidth, height: w.offsetHeight }; }"
            )
            drag(page, centre_of(page, ".session-window .session-titlebar"), 60, 40)
            moved = page.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " return { left: w.offsetLeft, top: w.offsetTop }; }"
            )
            check(
                "a titlebar drag OVER a fence moves the window by exactly the pointer delta",
                moved["left"] == before["left"] + 60 and moved["top"] == before["top"] + 40,
                f"before={before} after={moved}",
            )

            drag(page, centre_of(page, ".session-window .h-se"), 50, 30)
            sized = page.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " return { width: w.offsetWidth, height: w.offsetHeight }; }"
            )
            check(
                "…and a resize handle over a fence still grows the window",
                sized["width"] == before["width"] + 50 and sized["height"] == before["height"] + 30,
                f"before={before['width']}x{before['height']} after={sized['width']}x{sized['height']}",
            )

            page.evaluate(
                "() => document.querySelector('.session-window').classList.remove('focused')"
            )
            page.locator(".session-window .session-titlebar").first.click()
            page.wait_for_timeout(300)
            check(
                "…and a focus click over a fence still focuses the window",
                page.evaluate(
                    "() => document.querySelector('.session-window').classList.contains('focused')"
                ),
            )

            # The floor's OWN gesture: panning must survive inside a fence. Again
            # fence 1 — a drag started on the window above fence 0 is a window
            # drag, which is the tier working.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")
            page.wait_for_timeout(200)
            drag(page, bare_fence_point(page, 1), -120, 0)
            panned = page.evaluate("() => document.getElementById('workspace').scrollLeft")
            check(
                "dragging the floor INSIDE a fence still pans the plane",
                panned == 120,
                f"scrollLeft={panned}",
            )
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")
            page.wait_for_timeout(400)

            # The pan surface must survive right of the name field too: the head
            # is the only part of a fence that takes pointer events, so a band
            # stretched edge to edge would swallow the pan into a text selection.
            # Fence 1 sits in the grid's SECOND column, so at scrollLeft 0 the
            # point 40 px right of its head is PAST the viewport's right edge and
            # `elementFromPoint` answers null — the check read as a failure while
            # asserting nothing. Pan first, and take the client coordinates
            # afterwards: `getBoundingClientRect` already carries the offset.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 300; }")
            page.wait_for_timeout(200)
            head = page.evaluate(
                "() => { const f = document.querySelectorAll('.fence')[1];"
                " const h = f.querySelector('.fence-head');"
                " const ws = document.getElementById('workspace');"
                " return { headWidth: h.offsetWidth, fenceWidth: f.offsetWidth,"
                "   right: h.getBoundingClientRect().right,"
                "   viewRight: ws.getBoundingClientRect().right,"
                "   top: h.getBoundingClientRect().top + h.offsetHeight / 2 }; }"
            )
            check(
                "the probe point is really inside the viewport — a null hit asserts nothing",
                head["right"] + 40 < head["viewRight"],
                f"probe={head['right'] + 40} viewport right={head['viewRight']}",
            )
            check(
                "the fence head is shrink-wrapped, not the fence's full width",
                head["headWidth"] < head["fenceWidth"] * 0.75,
                f"head={head['headWidth']} fence={head['fenceWidth']}",
            )
            hit_beside = page.evaluate(
                "(p) => { const el = document.elementFromPoint(p.x, p.y);"
                "  return el && el.id; }",
                {"x": head["right"] + 40, "y": head["top"]},
            )
            check(
                "…so a press level with the name, just right of it, still reaches the stage",
                hit_beside == "stage",
                f"elementFromPoint gave id={hit_beside}",
            )

            # ===== scenario 4: they reached the daemon's own store ===========
            # Poll the store rather than sleeping past the 250 ms debounce: a
            # flat wait is a coin flip under load.
            deadline = time.time() + 10
            while time.time() < deadline:
                if desk_file.exists() and desk_file.read_text(encoding="utf-8").count("[[fences]]") == 2:
                    break
                time.sleep(0.3)
            text = desk_file.read_text(encoding="utf-8")
            check(
                "the desk store holds exactly two [[fences]] records",
                text.count("[[fences]]") == 2,
                f"got={text.count('[[fences]]')}",
            )
            check(
                "…carrying the names the operator typed",
                'name = "backend"' in text and 'name = "planning"' in text,
                f"file={text[-400:]}",
            )
            stored = json.loads(http("GET", "api/desk")[1])["fences"]
            on_screen = fence_dom(page)
            check(
                "…and the stored rects are the ones on screen",
                [
                    {k: int(f["rect"][k]) for k in ("left", "top", "width", "height")}
                    for f in stored
                ]
                == [{k: f[k] for k in ("left", "top", "width", "height")} for f in on_screen],
                f"stored={stored} screen={on_screen}",
            )
            check(
                "…and the legacy window is still in the same file, untouched",
                text.count("[[windows]]") == 1 and "w-fixture-a" in text,
                f"windows={text.count('[[windows]]')}",
            )

            # ===== scenario 5: a FRESH browser profile =======================
            # A new context is a new profile: no cookies, no per-client view, no
            # storage of any kind carried over from the page above.
            fresh_ctx = browser.new_context(viewport=dict(VIEW))
            fresh = desk_page(fresh_ctx)
            fresh.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 2", timeout=15000
            )
            fresh.wait_for_timeout(800)
            reloaded = fence_dom(fresh)
            check(
                "a fresh browser profile finds BOTH fences",
                len(reloaded) == 2,
                f"got={reloaded}",
            )
            check(
                "…with the same names",
                [f["name"] for f in reloaded] == ["backend", "planning"],
                f"got={[f['name'] for f in reloaded]}",
            )
            check(
                "…and rects dict-equal to the pre-reload measurement",
                [{k: f[k] for k in ("left", "top", "width", "height")} for f in reloaded]
                == [{k: f[k] for k in ("left", "top", "width", "height")} for f in on_screen],
                f"before={on_screen} after={reloaded}",
            )
            check(
                "…and the fence still sits under the restored window",
                fresh.evaluate(
                    "() => getComputedStyle(document.querySelector('.fence')).zIndex === '1'"
                    " && document.querySelectorAll('.session-window').length === 1"
                ),
            )
            fresh.screenshot(path=SHOT)
            check("the evidence screenshot is on disk", os.path.exists(SHOT), SHOT)
            fresh_ctx.close()

            # ===== scenario 5b: the × removes a fence, store included ========
            # Park the window well clear first: a console is `z >= 60` and so
            # covers a fence's controls wherever they meet, which is the tier
            # working — but it makes the × unclickable by a REAL click, and this
            # check is about the button, not the z-order (proved in scenario 3).
            page.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                "  w.style.left = '3000px'; w.style.top = '3000px'; }"
            )
            page.wait_for_timeout(200)
            draw_fence(page)
            page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 3", timeout=10000
            )
            page.wait_for_timeout(400)
            page.locator(".fence .fence-drop").nth(2).click()
            page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 2", timeout=10000
            )
            deadline = time.time() + 10
            while time.time() < deadline:
                if desk_file.read_text(encoding="utf-8").count("[[fences]]") == 2:
                    break
                time.sleep(0.3)
            check(
                "the × removes a fence from the screen AND from the daemon's store",
                desk_file.read_text(encoding="utf-8").count("[[fences]]") == 2,
                f"got={desk_file.read_text(encoding='utf-8').count('[[fences]]')}",
            )
            ctx.close()
            time.sleep(1.0)

            # ===== scenario 5c: a fence SIZES the plane =======================
            # `applyExtent` folds `.fence` in alongside `.session-window`
            # (ADR-0051 §2). Seeded far out with NO windows at all, the extent
            # can only come from the fence: revert that one selector and the
            # stage stays viewport-sized while the fence sits unreachable past
            # its edge. STAGE_MARGIN is 200.
            far = fence_json("f-far", "far away", 9)
            far["rect"] = {"left": 4000.0, "top": 40.0, "width": 720.0, "height": 460.0}
            http("PUT", "api/desk", {"windows": [], "fences": [far]})
            far_ctx = browser.new_context(viewport=dict(VIEW))
            far_page = desk_page(far_ctx)
            far_page.wait_for_function(
                "() => document.querySelectorAll('.fence').length === 1", timeout=15000
            )
            far_page.wait_for_timeout(600)
            plane = far_page.evaluate(
                "() => { const st = document.getElementById('stage');"
                "  const f = document.querySelector('.fence');"
                "  return { stage: st.offsetWidth, right: f.offsetLeft + f.offsetWidth,"
                "    windows: document.querySelectorAll('.session-window').length }; }"
            )
            check(
                "a restored fence with no window beside it still sizes the plane",
                plane["windows"] == 0
                and plane["right"] == 4720
                and plane["stage"] >= plane["right"] + 200,
                f"stage={plane['stage']} fence right={plane['right']} windows={plane['windows']}",
            )
            far_ctx.close()
            time.sleep(1.0)

            # ===== scenario 5d: a REFUSED desk read must not wipe the fences ==
            # The window twin of this is `wb_desk_327.py` scenario 8. `PUT
            # /api/desk` replaces the desk WHOLESALE and the pre-login `GET`
            # answers 401, so a page that could not READ the fences must never
            # WRITE over them — and must not discard them once the read succeeds
            # either, which is the half a wholesale-replace ingest gets wrong.
            seeded = [fence_json("f-keep-a", "kept a", 5), fence_json("f-keep-b", "kept b", 6)]
            http("PUT", "api/desk", {"windows": [], "fences": seeded})
            blind_ctx = browser.new_context(viewport=dict(VIEW))
            blind = blind_ctx.new_page()
            blind.route(
                "**/api/desk",
                lambda route: (
                    route.fulfill(status=401, body="unauthorized")
                    if route.request.method == "GET"
                    else route.continue_()
                ),
            )
            blind.goto(BASE)
            blind.wait_for_selector("[x-data]", timeout=8000)
            blind.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            blind.wait_for_timeout(1200)
            draw_fence(blind)
            blind.wait_for_timeout(1500)
            after_blind = [f["id"] for f in json.loads(http("GET", "api/desk")[1])["fences"]]
            check(
                "a page whose desk read was REFUSED does not overwrite the saved fences",
                after_blind == [f["id"] for f in seeded],
                f"want={[f['id'] for f in seeded]} got={after_blind}",
            )
            # And the guard lifts: once a read succeeds, the page's own fence
            # joins the saved ones instead of REPLACING them.
            blind.unroute("**/api/desk")
            blind.evaluate("() => window.WBConsole.afterLogin()")
            blind.wait_for_timeout(1000)
            draw_fence(blind)
            blind.wait_for_timeout(1800)
            resumed = [f["id"] for f in json.loads(http("GET", "api/desk")[1])["fences"]]
            check(
                "…and once it is readable again the saved fences SURVIVE the merge",
                all(f["id"] in resumed for f in seeded),
                f"want to keep={[f['id'] for f in seeded]} got={resumed}",
            )
            check(
                "…with this page's own fences added, not swapped in",
                len(resumed) > len(seeded),
                f"got={resumed}",
            )
            blind_ctx.close()
            time.sleep(1.0)

            # ===== scenario 6: the daemon's own guards ======================
            # Every page closed FIRST: a live shell debounces its own PUT and
            # would race the uploads below.

            many = {
                "windows": [],
                "fences": [fence_json(f"f{n}", "region", n) for n in range(1, 14)],
            }
            status, body = http("PUT", "api/desk", many)
            ids = [f["id"] for f in json.loads(body)["fences"]] if status == 200 else []
            check(
                "PUT /api/desk prunes the fences to the 12 newest by ts",
                status == 200 and ids == [f"f{n}" for n in range(2, 14)],
                f"status={status} ids={ids}",
            )
            kept = json.loads(http("GET", "api/desk")[1])["fences"]
            check(
                "…and the persisted desk agrees",
                [f["id"] for f in kept] == [f"f{n}" for n in range(2, 14)],
                f"got={[f['id'] for f in kept]}",
            )

            before_bad = desk_file.read_text(encoding="utf-8")
            try:
                bad_status, bad_body = http(
                    "PUT",
                    "api/desk",
                    {"windows": [], "fences": [fence_json("f-neg", "off-plane", 1, left=-1.0)]},
                )
            except urllib.error.HTTPError as e:
                bad_status, bad_body = e.code, e.read().decode()
            check(
                "PUT /api/desk refuses a fence with an off-plane origin",
                bad_status == 400,
                f"got={bad_status}",
            )
            check(
                "…naming the offending fence",
                "fence f-neg has an out-of-frame rect" in bad_body,
                f"body={bad_body[:200]}",
            )
            check(
                "…without touching the desk it already holds",
                desk_file.read_text(encoding="utf-8") == before_bad,
                "the refused upload must leave desk.toml byte-identical",
            )

            # The pre-#340 wire shape is refused wholesale, not half-applied.
            try:
                stale_status = http("PUT", "api/desk", [])[0]
            except urllib.error.HTTPError as e:
                stale_status = e.code
            check(
                "the pre-#340 bare-array body is refused, not read as an empty desk",
                stale_status == 422,
                f"got={stale_status}",
            )
            check(
                "…so a stale client cannot wipe the layout",
                desk_file.read_text(encoding="utf-8") == before_bad,
                "desk.toml must be byte-identical",
            )

            # ===== scenario 7: a corrupt desk degrades to an empty stage =====
            stop(proc)
            time.sleep(1.0)
            desk_file.write_text("not a toml { ][", encoding="utf-8")
            proc = launch(daemon_dir)
            if not wait_listening(BASE):
                check("the daemon restarts over a corrupt desk.toml", False)
                sys.exit(1)
            check("the daemon restarts over a corrupt desk.toml", True)

            status, body = http("GET", "api/desk")
            check(
                "a corrupt desk serves an EMPTY desk, not an error",
                status == 200 and body == '{"windows":[],"fences":[]}',
                f"status={status} body={body[:120]}",
            )

            corrupt_ctx = browser.new_context(viewport=dict(VIEW))
            corrupt = desk_page(corrupt_ctx)
            corrupt.wait_for_timeout(1500)
            empty = corrupt.evaluate(
                "() => ({ fences: document.querySelectorAll('.fence').length,"
                "  windows: document.querySelectorAll('.session-window').length,"
                "  alive: !!document.querySelector('[x-data]') })"
            )
            check(
                "…and the stage comes up EMPTY rather than failing",
                empty["fences"] == 0 and empty["windows"] == 0,
                f"got={empty}",
            )
            check("…with the shell itself alive", empty["alive"], f"got={empty}")
            corrupt_ctx.close()

            browser.close()
    finally:
        stop(proc)

    # The floor matches the real count: set loosely, a scenario that stopped
    # running would leave the suite green.
    # The floor is the REAL count, not a loose lower bound: scenario 3 is 10
    # checks and scenario 5 is 5, so a floor set well under the total lets a
    # whole scenario stop running while the suite still exits 0.
    ok = all(results) and len(results) >= 46
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("A FENCE IS DESK STATE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
