"""#336 browser acceptance: the Consoles stage is a PLANE, not a box.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The `desk.toml` fixture is
written BEFORE the daemon starts, in the PRE-CHANGE shape, so the "an existing
desk reopens byte-identical" criterion is proved against a real legacy file.

Scenario 1   at 1400x900 the fixture desk restores verbatim; `#workspace` is the
             viewport (`overflow:auto`), `#stage` the sized plane (bbox 1300x680
             + 200 margin = 1500x880), the dotted floor lives on the stage and
             pans with it, and `clampAll` is gone from the module surface
Scenario 2   shrunk to 800x600 NOTHING moves — the same four rects, the same
             stage, only more scroll room; a window outside the view is reached
             by scrolling, not by being dragged back into the frame
Scenario 3   back at 1400x900 the rects are byte-identical to scenario 1 — the
             measurement in the issue ("does not return") is now the assertion
Scenario 4   a drag up-left stops at the pinned 0,0 origin; a drag right GROWS
             the stage past its old edge instead of clipping the window

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/336-stage-plane-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_stage_336.py   (exit 0 = all pass)
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

PORT = 7431
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

# The fixture desk, in the shape a PRE-#336 shell wrote: absolute pixels, no
# stage, no migration marker. bbox = 1300 x 680, so the stage must measure
# 1300+200 x 680+200 at any viewport this test uses.
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
    empty = tempfile.mkdtemp(prefix="wb336_empty_")
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
    d = tempfile.mkdtemp(prefix="wb336_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #336 stage fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb336@example.com"],
        ["git", "config", "user.name", "wb336"],
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


def geom(page):
    """Every measurement of the viewport/stage pair, in ONE evaluate."""
    return page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        "  const st = document.getElementById('stage');"
        "  const cs = getComputedStyle(ws); const ss = getComputedStyle(st);"
        "  const tb = getComputedStyle(document.querySelector('.tabbody'));"
        "  return { overflowX: cs.overflowX, overflowY: cs.overflowY,"
        "    clientWidth: ws.clientWidth, clientHeight: ws.clientHeight,"
        "    scrollWidth: ws.scrollWidth, scrollHeight: ws.scrollHeight,"
        "    stageWidth: st.offsetWidth, stageHeight: st.offsetHeight,"
        "    stageParent: st.parentElement.id,"
        "    stageFloor: ss.backgroundImage, tabbodyFloor: tb.backgroundImage }; }"
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


def drag_title(page, index, dx, dy):
    box = page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const h = w.querySelector('.session-titlebar');"
        " const r = h.getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
        index,
    )
    page.mouse.move(box["x"], box["y"])
    page.mouse.down()
    page.mouse.move(box["x"] + dx / 3, box["y"] + dy / 3, steps=5)
    page.mouse.move(box["x"] + dx * 2 / 3, box["y"] + dy * 2 / 3, steps=5)
    page.mouse.move(box["x"] + dx, box["y"] + dy, steps=5)
    page.mouse.up()
    page.wait_for_timeout(400)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb336_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    write_legacy_desk(daemon_dir, slug)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        status, body = http("GET", "api/desk")
        served = json.loads(body) if status == 200 else []
        check(
            "the daemon serves the hand-written pre-#336 desk.toml",
            status == 200 and [r["rect"] for r in served] == [FIX_A, FIX_B],
            f"status={status} got={body[:200]}",
        )

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})

            # ===== scenario 1: the plane at 1400x900 ==========================
            page = desk_page(ctx, {"width": 1400, "height": 900})
            settle(page)
            big = rects(page)
            check(
                "an existing desk.toml reopens with byte-identical rects",
                big == [FIX_A, FIX_B],
                f"want={[FIX_A, FIX_B]} got={big}",
            )

            surface = page.evaluate(
                "() => ({ clamp: typeof window.WBConsole.clampAll,"
                " extent: typeof window.WBConsole.stageExtent })"
            )
            check(
                "`clampAll` is gone from the module surface",
                surface["clamp"] == "undefined",
                f"got={surface['clamp']}",
            )
            check(
                "…and the stage extent is exported as a pure function instead",
                surface["extent"] == "function",
                f"got={surface['extent']}",
            )

            g1 = geom(page)
            check("the stage is the viewport's child", g1["stageParent"] == "workspace", f"got={g1}")
            check(
                "the viewport scrolls on X",
                g1["overflowX"] == "auto",
                f"got={g1['overflowX']}",
            )
            check(
                "…and on Y",
                g1["overflowY"] == "auto",
                f"got={g1['overflowY']}",
            )
            check(
                f"the stage measures the bbox + margin on X ({STAGE_W})",
                g1["stageWidth"] == STAGE_W,
                f"got={g1['stageWidth']}",
            )
            check(
                f"…and on Y ({STAGE_H})",
                g1["stageHeight"] == STAGE_H,
                f"got={g1['stageHeight']}",
            )
            check(
                "the scrollbar measures the stage, not a guess",
                g1["scrollWidth"] == STAGE_W,
                f"got={g1['scrollWidth']}",
            )
            check(
                "…over a viewport that is really laid out",
                g1["clientWidth"] > 0 and g1["clientHeight"] > 0,
                f"got={g1['clientWidth']}x{g1['clientHeight']}",
            )
            gap_big = g1["scrollWidth"] - g1["clientWidth"]
            check("…and really scrollable at 1400x900", gap_big > 0, f"gap={gap_big}")

            check(
                "the dotted floor is on the STAGE",
                "radial-gradient" in g1["stageFloor"],
                f"got={g1['stageFloor'][:60]}",
            )
            check(
                "…and no longer on the tab body it would sit still on",
                g1["tabbodyFloor"] == "none",
                f"got={g1['tabbodyFloor'][:60]}",
            )
            pan = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " const st = document.getElementById('stage');"
                " ws.scrollLeft = 0; const before = st.getBoundingClientRect().left;"
                " ws.scrollLeft = 200; const after = st.getBoundingClientRect().left;"
                " ws.scrollLeft = 0; return after - before; }"
            )
            check(
                "…so the floor pans with the content, pixel for pixel",
                pan == -200,
                f"scrollLeft 200 moved the stage by {pan}",
            )

            # ===== scenario 2: shrunk to 800x600, nothing moves ===============
            page.set_viewport_size({"width": 800, "height": 600})
            page.wait_for_timeout(900)
            small = rects(page)
            check(
                "shrinking the browser moves no window",
                small == big,
                f"want={big} got={small}",
            )
            g2 = geom(page)
            check(
                "…and leaves the stage exactly as it was",
                (g2["stageWidth"], g2["stageHeight"]) == (STAGE_W, STAGE_H),
                f"got={g2['stageWidth']}x{g2['stageHeight']}",
            )
            check(
                "…a laid-out viewport, just a smaller one",
                g2["clientWidth"] > 0 and g2["clientWidth"] < g1["clientWidth"],
                f"got={g2['clientWidth']} (was {g1['clientWidth']})",
            )
            gap_small = g2["scrollWidth"] - g2["clientWidth"]
            check(
                "…so the SCROLL room grows instead of the layout shrinking",
                gap_small > gap_big,
                f"800x600 gap={gap_small} vs 1400x900 gap={gap_big}",
            )

            reach = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " const w = document.querySelectorAll('.session-window')[1];"
                " ws.scrollLeft = 0;"
                " const wsr = ws.getBoundingClientRect();"
                " const off = w.getBoundingClientRect();"
                " ws.scrollLeft = 700;"
                " const on = w.getBoundingClientRect();"
                " return { wsLeft: wsr.left, wsRight: wsr.right, off: off.left, on: on.left,"
                "          scrolled: ws.scrollLeft }; }"
            )
            check(
                "a window saved outside the small viewport is off-view",
                reach["off"] > reach["wsRight"],
                f"window at {reach['off']}, viewport right edge {reach['wsRight']}",
            )
            check(
                "…and is REACHED by scrolling, not by being dragged back in",
                reach["wsLeft"] <= reach["on"] < reach["wsRight"],
                f"after scrollLeft={reach['scrolled']} the window sits at {reach['on']}",
            )
            check(
                "…with its rect still untouched by the whole excursion",
                rects(page) == big,
                f"got={rects(page)}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "336-stage-plane-2026-07-27.png"))
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")

            # ===== scenario 3: back to 1400x900 — the layout RETURNS ==========
            page.set_viewport_size({"width": 1400, "height": 900})
            page.wait_for_timeout(900)
            back = rects(page)
            check(
                "1400x900 -> 800x600 -> 1400x900 leaves every rect identical",
                back == big,
                f"want={big} got={back}",
            )
            g3 = geom(page)
            check(
                "…and the stage unchanged across the whole round trip",
                (g3["stageWidth"], g3["stageHeight"]) == (STAGE_W, STAGE_H),
                f"got={g3['stageWidth']}x{g3['stageHeight']}",
            )

            # ===== scenario 4: the pinned origin, and a stage that grows ======
            drag_title(page, 0, -400, -400)
            after_up_left = rects(page)
            check(
                "a drag 400px up-left stops at the stage origin on X",
                after_up_left[0]["left"] == 0,
                f"got={after_up_left[0]}",
            )
            check(
                "…and on Y",
                after_up_left[0]["top"] == 0,
                f"got={after_up_left[0]}",
            )
            check(
                "…leaving no window with a negative origin",
                all(r["left"] >= 0 and r["top"] >= 0 for r in after_up_left),
                f"got={after_up_left}",
            )
            stage_before = geom(page)["stageWidth"]
            check(
                "…and the stage still measures the same bbox",
                stage_before == STAGE_W,
                f"got={stage_before}",
            )

            # Scroll the far window into view first: its titlebar centre is past
            # the viewport's right edge at scrollLeft 0, so the press would land
            # on the clipped-away half and never reach the handle.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 700; }")
            page.wait_for_timeout(200)
            drag_title(page, 1, 260, 0)
            grown = geom(page)
            moved = rects(page)[1]
            check(
                "dragging a window toward the far edge GROWS the stage",
                grown["stageWidth"] > stage_before,
                f"{stage_before} -> {grown['stageWidth']}",
            )
            check(
                "…carrying the window past the stage's old edge, unclipped",
                moved["left"] + moved["width"] > stage_before,
                f"window right edge {moved['left'] + moved['width']} vs old stage {stage_before}",
            )
            check(
                "…and never outside the stage it just grew",
                moved["left"] + moved["width"] <= grown["stageWidth"],
                f"window right edge {moved['left'] + moved['width']} vs stage {grown['stageWidth']}",
            )
            check(
                "…and the scrollbar follows the new extent",
                grown["scrollWidth"] == grown["stageWidth"],
                f"scrollWidth={grown['scrollWidth']} stage={grown['stageWidth']}",
            )

            page.wait_for_timeout(700)
            persisted = json.loads(http("GET", "api/desk")[1])
            by_id = {r["id"]: r["rect"] for r in persisted}
            check(
                "the pinned origin is what the daemon persisted",
                by_id.get("w-fixture-a") == {"left": 0, "top": 0, "width": 600, "height": 380},
                f"got={by_id.get('w-fixture-a')}",
            )
            check(
                "…and so is the rect that went past the old edge",
                (by_id.get("w-fixture-b") or {}).get("left") == moved["left"],
                f"got={by_id.get('w-fixture-b')} window left={moved['left']}",
            )

            # ===== scenario 5: maximize pins to the VIEWPORT, not the plane ===
            # `width/height: 100%` resolve against `#workspace`, but the window
            # scrolls with the stage — so without the `--max-left`/`--max-top`
            # pin a maximized console would sit at the plane's origin, off-view.
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 300; }")
            page.wait_for_timeout(200)
            page.locator(".session-window").nth(0).locator(".session-max").click()
            page.wait_for_timeout(600)
            maxed = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " const w = document.querySelectorAll('.session-window')[0];"
                " const a = w.getBoundingClientRect(); const b = ws.getBoundingClientRect();"
                " return { dx: Math.round(a.left - b.left), dy: Math.round(a.top - b.top),"
                "   width: Math.round(a.width), vw: Math.round(b.width),"
                "   lock: ws.classList.contains('maxlock'),"
                "   overflowX: getComputedStyle(ws).overflowX,"
                "   inlineLeft: w.style.left, inlineTop: w.style.top,"
                "   inlineWidth: w.style.width }; }"
            )
            check(
                "a maximized console is pinned to the scrolled VIEWPORT, not the plane origin",
                maxed["dx"] == 0 and maxed["dy"] == 0,
                f"offset from the viewport corner = {maxed['dx']},{maxed['dy']}",
            )
            check(
                "…filling it exactly",
                maxed["width"] == maxed["vw"],
                f"window {maxed['width']} vs viewport {maxed['vw']}",
            )
            check(
                "…with the viewport's scroll locked so the pin cannot desync",
                maxed["lock"] and maxed["overflowX"] == "hidden",
                f"maxlock={maxed['lock']} overflowX={maxed['overflowX']}",
            )
            check(
                "…while the inline styles still hold the PRE-maximize rect",
                (maxed["inlineLeft"], maxed["inlineTop"], maxed["inlineWidth"])
                == ("0px", "0px", "600px"),
                f"got={maxed['inlineLeft']},{maxed['inlineTop']},{maxed['inlineWidth']}",
            )
            page.locator(".session-window").nth(0).locator(".session-max").click()
            page.wait_for_timeout(600)
            restored = rects(page)[0]
            check(
                "restoring puts the console back on its own box",
                restored == {"left": 0, "top": 0, "width": 600, "height": 380},
                f"got={restored}",
            )
            check(
                "…and gives the viewport its scrollbars back",
                page.evaluate(
                    "() => { const ws = document.getElementById('workspace');"
                    " return !ws.classList.contains('maxlock')"
                    "   && getComputedStyle(ws).overflowX === 'auto'; }"
                ),
                "maxlock must lift with the last maximized window",
            )
            page.evaluate("() => { document.getElementById('workspace').scrollLeft = 0; }")
            page.wait_for_timeout(400)

            # A negative rect can only arrive from a hand-rolled client; the
            # daemon refuses it rather than persisting an off-plane origin.
            try:
                bad = http(
                    "PUT",
                    "api/desk",
                    [
                        {
                            "id": "w-bad",
                            "repo": slug,
                            "agent": "console",
                            "kind": "console",
                            "rect": {"left": -1, "top": 0, "width": 600, "height": 380},
                            "max": False,
                            "sessionId": None,
                            "ts": 1,
                        }
                    ],
                )[0]
            except urllib.error.HTTPError as e:
                bad = e.code
            check("PUT /api/desk refuses a negative origin", bad == 400, f"got={bad}")
            check(
                "…without touching the desk it already holds",
                [r["id"] for r in json.loads(http("GET", "api/desk")[1])]
                == [r["id"] for r in persisted],
                "the refused upload must not replace the desk",
            )

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 30
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE STAGE IS A PLANE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
