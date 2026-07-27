"""#339 browser acceptance: the view you left comes back, PER CLIENT.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. The desk fixture is two
`kind = "agent"` records, which `reconcileDesk` turns into PLACEHOLDERS: a
placeholder honours the record's rect through the same `buildChrome` path with
no PTY and no xterm, so the geometry is deterministic and the pass is cheap.

The state split under test is ADR-0051 §8: the desk (windows, and later fences)
stays daemon-owned, while the viewport offset and the open file tabs are per
CLIENT, in this browser profile. ADR-0050 §3's "no browser store" is narrowed to
"no *desk* in browser storage" — scenario 6 is the assertion of that narrowing.

Scenario 1   an empty desk lands on the stage origin
Scenario 2   nothing stored lands on the BOUNDING BOX of the restored windows
Scenario 3   pan + two file tabs survive a reload, offset byte-identical
Scenario 4   a second browser profile gets its own view and disturbs neither
Scenario 5   restoring on a smaller screen shows work, with every rect untouched
Scenario 6   no desk data reaches browser storage — key set, shape, vocabulary
Scenario 7   a tab switch away from Consoles and back keeps the pan (`x-show`)

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/339-view-per-client-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_view_339.py   (exit 0 = all pass)
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

PORT = 7434
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "339-view-per-client-2026-07-27.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
VIEW_KEY = "wb.view.v1"

# Far from the origin ON BOTH AXES on purpose: a landing bug that answers 0,0
# must not be able to pass by accident.
FIX_A = {"left": 1600, "top": 900, "width": 600, "height": 380}
FIX_B = {"left": 2400, "top": 1500, "width": 600, "height": 380}
# The bbox of the two: 1600..3000 x 900..1880, so its centre is (2300, 1390).
BBOX_CENTRE = (2300, 1390)

README_NEEDLE = "The #339 view fixture repo."

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
    empty = tempfile.mkdtemp(prefix="wb339_empty_")
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
    d = tempfile.mkdtemp(prefix="wb339_fixture_")
    p = Path(d)
    (p / "README.md").write_text(f"# fixture\n\n{README_NEEDLE}\n", encoding="utf-8")
    (p / "notes.md").write_text("# notes\n\nThe second tab.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb339@example.com"],
        ["git", "config", "user.name", "wb339"],
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


def desk_records(slug):
    out = []
    for wid, r, ts in (("w-339-a", FIX_A, 100), ("w-339-b", FIX_B, 101)):
        out.append(
            {
                "id": wid,
                "repo": slug,
                "agent": "claude",
                "kind": "agent",
                "rect": dict(r),
                "max": False,
                "sessionId": None,
                "ts": ts,
            }
        )
    return out


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
    page.wait_for_timeout(600)


def view_box(page):
    """The viewport's live scroll state, plus everything a landing is judged on."""
    return page.evaluate(
        "() => { const ws = document.getElementById('workspace');"
        "  return { scrollLeft: ws.scrollLeft, scrollTop: ws.scrollTop,"
        "    clientWidth: ws.clientWidth, clientHeight: ws.clientHeight,"
        "    scrollWidth: ws.scrollWidth, scrollHeight: ws.scrollHeight }; }"
    )


def stage_extent(page):
    return page.evaluate(
        "() => { const st = document.getElementById('stage');"
        "  return { width: st.offsetWidth, height: st.offsetHeight }; }"
    )


def bbox_landing(box, ext, rects_):
    """The landing `viewLanding` owes for these rects — centre the bbox, clamp.

    Derived from the LIVE viewport and extent rather than hard-coded, so this is
    an oracle over the rule and not a transcription of one observed run.
    """
    left = min(r["left"] for r in rects_)
    top = min(r["top"] for r in rects_)
    right = max(r["left"] + r["width"] for r in rects_)
    bottom = max(r["top"] + r["height"] for r in rects_)
    want_l = left + (right - left) / 2 - box["clientWidth"] / 2
    want_t = top + (bottom - top) / 2 - box["clientHeight"] / 2
    return (
        max(0, min(want_l, ext["width"] - box["clientWidth"])),
        max(0, min(want_t, ext["height"] - box["clientHeight"])),
    )


def shows(box, rect):
    """Does `rect` (stage coordinates) intersect the viewport placed at `box`?"""
    return (
        rect["left"] < box["scrollLeft"] + box["clientWidth"]
        and rect["left"] + rect["width"] > box["scrollLeft"]
        and rect["top"] < box["scrollTop"] + box["clientHeight"]
        and rect["top"] + rect["height"] > box["scrollTop"]
    )


def inside(box, point):
    x, y = point
    return (
        box["scrollLeft"] <= x <= box["scrollLeft"] + box["clientWidth"]
        and box["scrollTop"] <= y <= box["scrollTop"] + box["clientHeight"]
    )


def fresh_context(browser, viewport):
    """A brand-new browser profile that records what it inherited at BOOT.

    The landing itself fires a `scroll`, which `saveOffset` persists — so a
    "nothing was stored" assertion read after the page settles always finds the
    landing it just wrote. `add_init_script` runs before any page script, which
    is the only honest moment to sample it.
    """
    ctx = browser.new_context(viewport=viewport)
    ctx.add_init_script(
        f"window.__viewAtBoot = localStorage.getItem({VIEW_KEY!r});"
        f"window.__keysAtBoot = Object.keys(localStorage);"
    )
    return ctx


def activate_consoles(page, want=2):
    """Return to the Consoles tab the way a CLICK does.

    Writing `SH.active` by hand is not the same act: only `activate()` reaches
    `refitAll()`, and `refitAll()` is the path that re-applies the stored offset
    after `x-show`'s `display:none` threw the scroll position away.
    """
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    if want:
        settle(page, want)
    else:
        page.wait_for_timeout(1200)


def desk_page(ctx, viewport, want=2):
    page = ctx.new_page()
    page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    activate_consoles(page, want)
    return page


def open_file_tab(page, slug, path):
    ftype = "markdown" if path.endswith(".md") else "code"
    page.evaluate(
        f"([slug, path, ftype]) => {SH}.openTab("
        "{ project: slug, path: path, title: path, ftype: ftype })",
        [slug, path, ftype],
    )
    page.wait_for_timeout(900)
    return f"file:{slug}:{path}"


def tab_state(page):
    return page.evaluate(f"() => ({{ ids: {SH}.tabs.map((t) => t.id), active: {SH}.active }})")


def stored_raw(page):
    return page.evaluate(f"() => localStorage.getItem({VIEW_KEY!r})")


def pan_to(page, left, top):
    """Pan by writing the offsets, then wait for the debounced store to catch up."""
    page.evaluate(
        "([l, t]) => { const ws = document.getElementById('workspace');"
        "  ws.scrollLeft = l; ws.scrollTop = t; }",
        [left, top],
    )
    page.wait_for_function(
        f"([l, t]) => {{ const raw = localStorage.getItem({VIEW_KEY!r}); if (!raw) return false;"
        "  const off = (JSON.parse(raw) || {}).off;"
        "  return !!off && off.left === l && off.top === t; }",
        arg=[left, top],
        timeout=8000,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb339_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # ===== scenario 1: an empty desk lands on the origin ================
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900}, want=0)
            check(
                "the desk really is empty, so the origin landing is about an empty plane",
                json.loads(http("GET", "api/desk")[1]) == {"windows": [], "fences": []}
                and page.evaluate("() => document.querySelectorAll('.session-window').length") == 0,
            )
            box = view_box(page)
            check(
                "an empty desk lands on the stage origin",
                box["scrollLeft"] == 0 and box["scrollTop"] == 0,
                f"got={box['scrollLeft']},{box['scrollTop']}",
            )
            ctx.close()

            # ===== scenario 2: nothing stored lands on the bounding box =========
            status, _ = http("PUT", "api/desk", {"windows": desk_records(slug), "fences": []})
            check("the two far-off fixture windows reach the daemon's desk", status == 200, f"status={status}")

            ctx = fresh_context(browser, {"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900})
            check(
                "the fixture desk restores verbatim — the landing moves the VIEW, not the rects",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )
            # Without this the scenario proves nothing: a stored offset from an
            # earlier profile would make the bbox landing below unfalsifiable.
            at_boot = page.evaluate("() => window.__viewAtBoot")
            check(
                "…and this profile stored NOTHING before the page ran",
                at_boot is None,
                f"at_boot={at_boot!r}",
            )
            box = view_box(page)
            check(
                "with nothing stored the view leaves the corner of the plane",
                box["scrollLeft"] > 0 and box["scrollTop"] > 0,
                f"got={box['scrollLeft']},{box['scrollTop']} client={box['clientWidth']}x{box['clientHeight']}",
            )
            check(
                "…landing on the BOUNDING BOX of the restored windows, whose centre is in frame",
                inside(box, BBOX_CENTRE),
                f"centre={BBOX_CENTRE} view={box['scrollLeft']},{box['scrollTop']}"
                f" +{box['clientWidth']}x{box['clientHeight']}",
            )
            check(
                "…with BOTH windows actually showing, not merely their midpoint",
                shows(box, FIX_A) and shows(box, FIX_B),
                f"a={shows(box, FIX_A)} b={shows(box, FIX_B)}",
            )
            # The EXACT pair the rule owes, derived from the live viewport and
            # extent. Without this, a landing centred on ONE rect satisfies every
            # assertion above — FIX_B is visible from FIX_A's centring and vice
            # versa, so a `bboxOf` returning its first rect would pass the lot.
            want = bbox_landing(box, stage_extent(page), [FIX_A, FIX_B])
            check(
                "…on the UNION's centre exactly, not on either window's own",
                (box["scrollLeft"], box["scrollTop"]) == want,
                f"want={want} got={box['scrollLeft']},{box['scrollTop']}",
            )
            ctx.close()

            # ===== scenario 3: pan + two file tabs survive a reload ============
            # PAN_TO differs from the bbox landing (1774,983) on both axes AND
            # still shows a window — the two things that make the assertion below
            # discriminate between "restored" and "re-landed".
            PAN_TO = (1500, 850)
            ctx = fresh_context(browser, {"width": 1400, "height": 900})
            page = desk_page(ctx, {"width": 1400, "height": 900})
            landing_here = bbox_landing(view_box(page), stage_extent(page), [FIX_A, FIX_B])
            pan_to(page, *PAN_TO)
            before = view_box(page)
            check(
                "the pan is a REAL offset, and one the bbox landing would NOT produce",
                (before["scrollLeft"], before["scrollTop"]) == PAN_TO and PAN_TO != landing_here,
                f"got={before['scrollLeft']},{before['scrollTop']} landing={landing_here}",
            )
            readme_id = open_file_tab(page, slug, "README.md")
            notes_id = open_file_tab(page, slug, "notes.md")
            state = tab_state(page)
            check(
                "both file tabs are open with the second active before the reload",
                state["ids"] == ["consoles", readme_id, notes_id] and state["active"] == notes_id,
                f"got={state}",
            )
            page.wait_for_timeout(600)

            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(
                f"(want) => {SH}.tabs.length === want", arg=3, timeout=15000
            )
            page.wait_for_timeout(900)
            state = tab_state(page)
            check(
                "the reload brings back the same three tabs, in order",
                state["ids"] == ["consoles", readme_id, notes_id],
                f"got={state['ids']}",
            )
            check(
                "…with the same one active",
                state["active"] == notes_id,
                f"got={state['active']!r}",
            )
            # A restored tab that never fetched its bytes is an empty shell: the
            # content check is what proves the tab is USABLE, not merely listed.
            page.evaluate(f"([id]) => {SH}.activate(id)", [readme_id])
            page.wait_for_function(
                "(needle) => document.getElementById('viewers').innerText.includes(needle)",
                arg=README_NEEDLE,
                timeout=15000,
            )
            check(
                "…and the restored tab shows the file's real bytes",
                README_NEEDLE in page.evaluate("() => document.getElementById('viewers').innerText"),
            )

            activate_consoles(page)
            after = view_box(page)
            check(
                "returning to Consoles lands back on the EXACT offset left before the reload",
                (after["scrollLeft"], after["scrollTop"]) == PAN_TO,
                f"want={PAN_TO} got={after['scrollLeft']},{after['scrollTop']}",
            )
            check(
                "…with the desk's rects untouched by any of it",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )
            raw_a = stored_raw(page)
            # The DIRECT oracle for the store, not just for the screen: a flush
            # that races the tab switch persists the `x-show` reset (`off:{0,0}`)
            # while the screen still shows the pan, and every assertion above
            # stays green while the NEXT reload silently re-lands on the bbox.
            check(
                "…and the STORE holds that offset too, not the `x-show` reset",
                json.loads(raw_a).get("off") == {"left": PAN_TO[0], "top": PAN_TO[1]},
                f"got={json.loads(raw_a).get('off')} want={{'left': {PAN_TO[0]}, 'top': {PAN_TO[1]}}}",
            )

            # ===== scenario 4: a second profile gets its OWN view ==============
            ctx_b = fresh_context(browser, {"width": 1400, "height": 900})
            page_b = desk_page(ctx_b, {"width": 1400, "height": 900})
            check(
                "the second profile inherited nothing from the first",
                page_b.evaluate("() => window.__viewAtBoot") is None,
                f"at_boot={page_b.evaluate('() => window.__viewAtBoot')!r}",
            )
            box_b = view_box(page_b)
            check(
                "…so it takes the bounding-box landing, NOT profile A's pan",
                (box_b["scrollLeft"], box_b["scrollTop"]) != PAN_TO and box_b["scrollLeft"] > 0,
                f"a={PAN_TO} b={box_b['scrollLeft']},{box_b['scrollTop']}",
            )
            check(
                "…and profile B opened no file tabs of its own",
                tab_state(page_b)["ids"] == ["consoles"],
                f"got={tab_state(page_b)['ids']}",
            )
            page_b.evaluate("() => { const ws = document.getElementById('workspace');"
                            " ws.scrollLeft = 40; ws.scrollTop = 30; }")
            page_b.wait_for_timeout(700)
            ctx_b.close()

            check(
                "profile A's stored view is byte-identical after B panned and closed",
                stored_raw(page) == raw_a,
                f"before={raw_a!r} after={stored_raw(page)!r}",
            )

            # ===== scenario 5: a smaller screen still shows work ===============
            page.set_viewport_size({"width": 800, "height": 600})
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            activate_consoles(page)
            small = view_box(page)
            check(
                "every rect is untouched by the smaller screen — nothing refits (#336)",
                rects(page) == [FIX_A, FIX_B],
                f"got={rects(page)}",
            )
            check(
                "…and the LARGE-screen offset is what came back — clamped, not re-landed",
                (small["scrollLeft"], small["scrollTop"]) == PAN_TO,
                f"want={PAN_TO} got={small['scrollLeft']},{small['scrollTop']}"
                f" client={small['clientWidth']}x{small['clientHeight']}",
            )
            check(
                "…the restored offset is inside what this viewport can actually reach",
                small["scrollLeft"] <= small["scrollWidth"] - small["clientWidth"]
                and small["scrollTop"] <= small["scrollHeight"] - small["clientHeight"],
                f"off={small['scrollLeft']},{small['scrollTop']}"
                f" max={small['scrollWidth'] - small['clientWidth']},"
                f"{small['scrollHeight'] - small['clientHeight']}",
            )
            check(
                "…and it lands on a view that SHOWS work, not on empty plane",
                shows(small, FIX_A) or shows(small, FIX_B),
                f"view={small['scrollLeft']},{small['scrollTop']}"
                f" +{small['clientWidth']}x{small['clientHeight']}",
            )

            # ----- scenario 5b: a stored offset that shows NOTHING degrades ----
            # The falsifying half of the smaller-screen criterion, in a browser
            # rather than only in the node table: 0,0 is a well-formed stored
            # offset under which no window is on screen. Drop the intersection
            # test and the page boots pinned at the corner of an empty plane.
            page.evaluate(
                f"() => {{ const r = JSON.parse(localStorage.getItem({VIEW_KEY!r}));"
                f"  r.off = {{ left: 0, top: 0 }};"
                f"  localStorage.setItem({VIEW_KEY!r}, JSON.stringify(r)); }}"
            )
            page.reload()
            page.wait_for_selector("[x-data]", timeout=8000)
            activate_consoles(page)
            degraded = view_box(page)
            want = bbox_landing(degraded, stage_extent(page), [FIX_A, FIX_B])
            check(
                "a stored offset showing no window falls back to the bbox landing",
                (degraded["scrollLeft"], degraded["scrollTop"]) == want,
                f"want={want} got={degraded['scrollLeft']},{degraded['scrollTop']}",
            )
            check(
                "…which is to say it still shows work, from a stored 0,0",
                shows(degraded, FIX_A) or shows(degraded, FIX_B),
                f"view={degraded['scrollLeft']},{degraded['scrollTop']}"
                f" +{degraded['clientWidth']}x{degraded['clientHeight']}",
            )

            # ===== scenario 6: no DESK in browser storage ======================
            desk_now = json.loads(http("GET", "api/desk")[1])["windows"]
            check(
                "the daemon's desk is NON-empty, so the absence below is not vacuous",
                len(desk_now) >= 2,
                f"records={len(desk_now)}",
            )
            keys = page.evaluate("() => Object.keys(localStorage)")
            check(
                "browser storage holds exactly the one permitted view key",
                set(keys) == {VIEW_KEY},
                f"got={keys}",
            )
            raw = stored_raw(page)
            stored = json.loads(raw)
            check(
                "…whose shape carries only the view: v, off, tabs, active",
                set(stored.keys()) <= {"v", "off", "tabs", "active"},
                f"got={sorted(stored.keys())}",
            )
            leaked = [w for w in ("windows", "fences", "rect", "sessionId") if w in raw]
            check(
                "…and none of the desk's vocabulary appears in the raw string",
                leaked == [],
                f"leaked={leaked} raw={raw!r}",
            )
            leaked_ids = [r["id"] for r in desk_now if r["id"] in raw]
            check(
                "…nor any id the daemon's desk is actually serving",
                leaked_ids == [],
                f"leaked={leaked_ids} ids={[r['id'] for r in desk_now]}",
            )

            # ===== scenario 7: a tab switch must not lose the pan ==============
            page.set_viewport_size({"width": 1400, "height": 900})
            activate_consoles(page)
            SWITCH_TO = (1620, 920)
            pan_to(page, *SWITCH_TO)
            page.evaluate(f"([id]) => {SH}.activate(id)", [readme_id])
            page.wait_for_timeout(900)
            hidden = page.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                "  return { shown: ws.offsetParent !== null, cw: ws.clientWidth }; }"
            )
            check(
                "switching to a file tab really hides the viewport (`x-show`)",
                not hidden["shown"] or hidden["cw"] == 0,
                f"got={hidden}",
            )
            activate_consoles(page)
            back = view_box(page)
            check(
                "…and switching back lands on the pan again, not on the origin",
                (back["scrollLeft"], back["scrollTop"]) == SWITCH_TO,
                f"want={SWITCH_TO} got={back['scrollLeft']},{back['scrollTop']}",
            )
            # The plane the return lands ON must still be the desk's. `refitAll`
            # runs from Alpine's `$nextTick`, which fires BEFORE `x-show` applies
            # the flip: measured, it refit a still-hidden tab, read every window
            # as 0×0 and collapsed the stage to the bare margin (3200×2080 ->
            # 200×200) with nothing to recompute it after. The offsets above
            # survive that (Chrome restores them itself), so only the extent
            # catches it.
            ext_back = stage_extent(page)
            check(
                "…on a stage that still holds the whole desk, not the bare margin",
                ext_back["width"] >= FIX_B["left"] + FIX_B["width"]
                and ext_back["height"] >= FIX_B["top"] + FIX_B["height"],
                f"got={ext_back} desk_edge=({FIX_B['left'] + FIX_B['width']},"
                f"{FIX_B['top'] + FIX_B['height']})",
            )

            page.screenshot(path=SHOT)
            check("the evidence screenshot is written", os.path.exists(SHOT), SHOT)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The floor matches the real count: set loosely, a scenario that stopped
    # running would leave the suite green.
    ok = all(results) and len(results) >= 37
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("THE VIEW IS PER CLIENT")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
