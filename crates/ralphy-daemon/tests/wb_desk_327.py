"""#327 browser acceptance: the desk layout is DAEMON state, not browser state.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched.

Scenario 1   a fresh daemon dir serves `GET /api/desk` → 200 `[]` and has written
             no `desk.toml`
Scenario 2   two consoles (one dragged, one maximized) land in `desk.toml` as two
             `[[windows]]` tables
Scenario 3   a SECOND browser context (fresh profile, empty storage) restores the
             same rects and the same maximized flags
Scenario 4   a desk saved at 1400x900 restores VERBATIM at 800x600 — since #336
             the stage is a plane and the viewport scrolls over it, so an
             off-view window is reached by scrolling, never refitted (the
             `clampAll` this scenario used to assert is deleted)
Scenario 5   no DESK in browser storage — the only permitted key is the per-client
             view `wb.view.v1` (viewport offset + open file tabs), which ADR-0051
             §8 narrows ADR-0050 §3 to allow: that rejection was of a second copy
             of the DESK, authoritative in no mode. The stored record's shape, the
             absence of every desk word, and the absence of every id the daemon is
             actually serving are all asserted; `wb-console.js` still names the
             browser store zero times, which now means "the desk module never
             touches it" (issue #339)
Scenario 6   a CORRUPT `desk.toml` yields an empty desk, not a startup failure
Scenario 7   30 uploaded records come back as exactly 24, newest by `ts`, with the
             live windows still present

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/327-desk-daemon-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_desk_327.py   (exit 0 = all pass)
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

PORT = 7399
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
CONSOLE_JS = os.path.join(REPO_ROOT, "crates", "ralphy-daemon", "assets", "ui", "wb-console.js")
# The ONE browser key ADR-0051 §8 permits (issue #339) — the per-client view, not
# a second copy of the desk. Scenario 5 is the assertion of exactly that line.
VIEW_KEY = "wb.view.v1"
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
    empty = tempfile.mkdtemp(prefix="wb327_empty_")
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
    d = tempfile.mkdtemp(prefix="wb327_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #327 desk fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb327@example.com"],
        ["git", "config", "user.name", "wb327"],
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
    """A raw request that does NOT go through a browser — proves the daemon
    answers on its own, with no page in the loop."""
    data = None
    headers = {}
    if body is not None:
        import json as _json

        data = _json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


def open_console(page, slug):
    before = page.locator(".session-window").count()
    page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000
    )
    win = page.locator(".session-window").nth(before)
    win.locator(".xterm").wait_for(timeout=15000)
    page.wait_for_timeout(400)
    return win


def rects(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) =>"
        " ({ left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth, height: w.offsetHeight }))"
    )


def maxima(page):
    return page.evaluate(
        "() => [...document.querySelectorAll('.session-window')].map((w) =>"
        " w.classList.contains('maximized'))"
    )


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
    page.mouse.move(box["x"] + dx / 2, box["y"] + dy / 2, steps=3)
    page.mouse.move(box["x"] + dx, box["y"] + dy, steps=3)
    page.mouse.up()
    page.wait_for_timeout(400)


def desk_page(ctx, viewport=None, at_origin=False):
    """A page on the Consoles tab, with the desk already restored.

    `at_origin` puts the viewport back on the plane's corner after the landing
    (issue #339): this suite's subject is the RECTS, and since #336 a maximized
    window's `offsetLeft` is the viewport pin (`--max-left`), so a page that
    lands on the bbox of a far-off desk reports the pin, not the stored rect.
    Zeroing here restores this suite's own origin — it does not weaken anything.
    """
    page = ctx.new_page()
    if viewport:
        page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(1800)
    if at_origin:
        page.evaluate(
            "() => { const ws = document.getElementById('workspace');"
            "  ws.scrollLeft = 0; ws.scrollTop = 0; }"
        )
        page.wait_for_timeout(600)
    return page


def desk_record(rid, ts, left=40, top=40):
    return {
        "id": rid,
        "repo": "owner/repo",
        "agent": "console",
        "kind": "console",
        "rect": {"left": left, "top": top, "width": 400, "height": 300},
        "max": False,
        "sessionId": None,
        "ts": ts,
    }


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb327_reg_")
    desk_toml = os.path.join(daemon_dir, "desk.toml")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        # --- scenario 1: a fresh daemon dir has no desk at all ---------------
        status, body = http("GET", "api/desk")
        check("a fresh daemon serves GET /api/desk as 200", status == 200, f"got={status}")
        check("…with an empty desk", body.strip() == "[]", f"got={body!r}")
        check(
            "…having written no desk.toml",
            not os.path.exists(desk_toml),
            f"exists={os.path.exists(desk_toml)}",
        )

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # --- scenario 2: two consoles land in desk.toml -------------------
            ctx_a = browser.new_context(viewport={"width": 1400, "height": 900})
            page = desk_page(ctx_a)
            open_console(page, slug)
            open_console(page, slug)
            drag_title(page, 0, -80, 40)
            page.locator(".session-window").nth(1).locator(".session-max").click()
            page.wait_for_timeout(700)

            rects_a = rects(page)
            max_a = maxima(page)
            check("two consoles are open", len(rects_a) == 2, f"got={len(rects_a)}")
            check("…the second one maximized", max_a == [False, True], f"got={max_a}")
            check("the daemon wrote desk.toml", os.path.exists(desk_toml))
            toml_text = Path(desk_toml).read_text(encoding="utf-8")
            check(
                "…holding one [[windows]] table per console",
                toml_text.count("[[windows]]") == 2,
                f"got={toml_text.count('[[windows]]')}",
            )
            served = http("GET", "api/desk")[1]
            check(
                "…and serves both records over HTTP, outside any browser",
                served.count('"id"') == 2 and '"max":true' in served,
                f"got={served[:200]}",
            )

            # --- scenario 5a: no DESK in browser storage (#339) ----------------
            # The desk on the daemon side, read in the SAME breath, so "none of
            # these ids is in the browser" cannot pass by the desk being empty.
            desk_now = json.loads(served)
            check(
                "the daemon's desk is non-empty, so the absence below is not vacuous",
                len(desk_now) == 2,
                f"records={len(desk_now)}",
            )
            keys = page.evaluate("() => Object.keys(localStorage)")
            check(
                "browser storage holds nothing but the permitted per-client view key",
                set(keys) <= {VIEW_KEY},
                f"got={keys}",
            )
            raw = page.evaluate(f"() => localStorage.getItem({VIEW_KEY!r})") or "{}"
            check(
                "…whose record carries only the view: v, off, tabs, active",
                set(json.loads(raw).keys()) <= {"v", "off", "tabs", "active"},
                f"got={sorted(json.loads(raw).keys())}",
            )
            leaked = [w for w in ("windows", "fences", "rect", "sessionId") if w in raw]
            check(
                "…and none of the desk's vocabulary reaches the browser",
                leaked == [],
                f"leaked={leaked} raw={raw!r}",
            )
            leaked_ids = [r["id"] for r in desk_now if r["id"] in raw]
            check(
                "…nor any id the daemon's desk is serving right now",
                leaked_ids == [],
                f"leaked={leaked_ids} ids={[r['id'] for r in desk_now]}",
            )
            js_hits = Path(CONSOLE_JS).read_text(encoding="utf-8").count("localStorage")
            check(
                "…and the desk module itself never touches the browser store",
                js_hits == 0,
                f"got={js_hits}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "327-desk-daemon-2026-07-26.png"))
            ctx_a.close()

            # --- scenario 3: a SECOND browser profile restores the desk -------
            ctx_b = browser.new_context(viewport={"width": 1400, "height": 900})
            page_b = desk_page(ctx_b, at_origin=True)
            check(
                "a fresh browser profile starts with no desk key of its own (#339)",
                set(page_b.evaluate("() => Object.keys(localStorage)")) <= {VIEW_KEY},
                f"got={page_b.evaluate('() => Object.keys(localStorage)')}",
            )
            check(
                "a second browser restores both windows",
                page_b.locator(".session-window").count() == 2,
                f"got={page_b.locator('.session-window').count()}",
            )
            rects_b = rects(page_b)
            check(
                "…at byte-identical rectangles",
                rects_b == rects_a,
                f"want={rects_a} got={rects_b}",
            )
            check(
                "…with the same maximized state",
                maxima(page_b) == [False, True],
                f"got={maxima(page_b)}",
            )
            check(
                "…still holding no browser-side desk key (#339)",
                set(page_b.evaluate("() => Object.keys(localStorage)")) <= {VIEW_KEY},
                f"got={page_b.evaluate('() => Object.keys(localStorage)')}",
            )

            # --- scenario 7: the cap, with the live windows pinned ------------
            live_ids = page_b.evaluate(
                "() => [...document.querySelectorAll('.session-window')].map((w) => w._deskId)"
            )
            check("the live windows carry desk ids", all(live_ids), f"got={live_ids}")
            # 30 records, the two live ones LAST (highest ts) so the cap keeps them.
            uploaded = [desk_record(f"w{n}", n) for n in range(1, 29)] + [
                desk_record(live_ids[0], 100),
                desk_record(live_ids[1], 101),
            ]
            status, body = http("PUT", "api/desk", uploaded)
            check("PUT /api/desk answers 200", status == 200, f"got={status}")
            import json as _json

            pruned = _json.loads(body)
            check("…pruning 30 records to exactly 24", len(pruned) == 24, f"got={len(pruned)}")
            ids = [r["id"] for r in pruned]
            check(
                "…keeping the newest by ts and dropping the oldest",
                "w1" not in ids and "w6" not in ids and "w28" in ids,
                f"got={ids}",
            )
            check(
                "…with both live windows still on the desk",
                live_ids[0] in ids and live_ids[1] in ids,
                f"live={live_ids} kept={ids}",
            )
            back = _json.loads(http("GET", "api/desk")[1])
            check(
                "…and the persisted desk agrees",
                [r["id"] for r in back] == ids,
                f"got={[r['id'] for r in back]}",
            )
            ctx_b.close()

            # --- scenario 4: a smaller viewport restores VERBATIM -------------
            # Restore the two-window desk saved at 1400x900 before shrinking.
            http("PUT", "api/desk", [desk_record(live_ids[0], 200, 900, 500)])
            ctx_c = browser.new_context(viewport={"width": 800, "height": 600})
            page_c = desk_page(ctx_c, viewport={"width": 800, "height": 600})
            page_c.wait_for_timeout(800)
            small = page_c.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " return { rects: [...document.querySelectorAll('.session-window')].map((w) =>"
                "     ({ left: w.offsetLeft, top: w.offsetTop, width: w.offsetWidth,"
                "        height: w.offsetHeight })),"
                "   clientWidth: ws.clientWidth, clientHeight: ws.clientHeight,"
                "   scrollWidth: ws.scrollWidth, scrollHeight: ws.scrollHeight }; }"
            )
            check(
                "a desk saved at 1400x900 restores a window at 800x600",
                len(small["rects"]) >= 1,
                f"windows={len(small['rects'])}",
            )
            # The FULL rect, not just the X axis: a build that refitted only the
            # Y axis (or only the width) would slip past a partial assertion. The
            # other windows on screen are ADOPTED live sessions with cascaded
            # rects — this criterion is about the one the desk RECORD placed.
            saved_rect = {"left": 900, "top": 500, "width": 400, "height": 300}
            check(
                "…at the rect it was saved with, every component — nothing refits it (#336)",
                saved_rect in small["rects"],
                f"want={saved_rect} among={small['rects']}",
            )
            check(
                "…because the viewport SCROLLS over the stage instead",
                small["scrollWidth"] > small["clientWidth"] > 0
                and small["scrollHeight"] > small["clientHeight"] > 0,
                f"scroll={small['scrollWidth']}x{small['scrollHeight']}"
                f" client={small['clientWidth']}x{small['clientHeight']}",
            )
            ctx_c.close()

            # --- scenario 8: a REFUSED desk read must not wipe the desk -------
            # The seam the self-review flagged. `PUT /api/desk` replaces the desk
            # WHOLESALE, and under the `Session` policy the pre-login `GET
            # /api/desk` answers 401. Treating that as "an empty desk" and then
            # flushing on the first drag destroys the operator's real layout, so
            # a page that could not READ the desk must never WRITE it.
            saved = [desk_record(f"s{n}", n, 40 + n, 40 + n) for n in range(1, 6)]
            http("PUT", "api/desk", saved)
            ctx_d = browser.new_context(viewport={"width": 1400, "height": 900})
            page_d = ctx_d.new_page()
            page_d.route(
                "**/api/desk",
                lambda route: (
                    route.fulfill(status=401, body="unauthorized")
                    if route.request.method == "GET"
                    else route.continue_()
                ),
            )
            page_d.goto(BASE)
            page_d.wait_for_selector("[x-data]", timeout=8000)
            page_d.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            page_d.wait_for_timeout(600)
            open_console(page_d, slug)
            drag_title(page_d, 0, -60, 30)
            page_d.wait_for_timeout(1500)
            after = [r["id"] for r in _json.loads(http("GET", "api/desk")[1])]
            check(
                "a page whose desk read was REFUSED does not overwrite the saved desk",
                after == [r["id"] for r in saved],
                f"want={[r['id'] for r in saved]} got={after}",
            )
            # And the guard lifts: once a read succeeds, writes resume.
            page_d.unroute("**/api/desk")
            page_d.evaluate("() => window.WBConsole.afterLogin()")
            page_d.wait_for_timeout(800)
            drag_title(page_d, 0, 40, -20)
            page_d.wait_for_timeout(1200)
            resumed = [r["id"] for r in _json.loads(http("GET", "api/desk")[1])]
            check(
                "…and resumes writing once the desk is readable again",
                all(r["id"] in resumed for r in saved) and len(resumed) > len(saved),
                f"got={resumed}",
            )
            ctx_d.close()
            browser.close()

        # --- scenario 6: a corrupt desk.toml is an empty desk, not a crash ----
        stop(proc)
        Path(desk_toml).write_text("not a toml { ][", encoding="utf-8")
        proc = launch(daemon_dir)
        check("the daemon starts with a corrupt desk.toml", wait_listening(BASE))
        check("…still answering /api/sessions", http("GET", "api/sessions")[0] == 200)
        status, body = http("GET", "api/desk")
        check("…and serving an EMPTY desk", status == 200 and body.strip() == "[]", f"{status} {body!r}")
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 34
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("DESK IS DAEMON STATE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
