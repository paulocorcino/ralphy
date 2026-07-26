"""#327 browser acceptance: the desk layout is DAEMON state, not browser state.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched.

Scenario 1   a fresh daemon dir serves `GET /api/desk` → 200 `[]` and has written
             no `desk.toml`
Scenario 2   two consoles (one dragged, one maximized) land in `desk.toml` as two
             `[[windows]]` tables
Scenario 3   a SECOND browser context (fresh profile, empty storage) restores the
             same rects and the same maximized flags
Scenario 4   a desk saved at 1400x900 restores fully on-screen at 800x600 — the
             `clampAll` the issue predicted, which restoreDesk now CALLS (its
             ResizeObserver has already fired by the time windows are restored)
Scenario 5   the shell touches no browser storage at all — `localStorage.length`
             is 0 after the session, and `wb-console.js` names it zero times
Scenario 6   a CORRUPT `desk.toml` yields an empty desk, not a startup failure
Scenario 7   30 uploaded records come back as exactly 24, newest by `ts`, with the
             live windows still present

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/327-desk-daemon-2026-07-26.png.
Run: python crates/ralphy-daemon/tests/wb_desk_327.py   (exit 0 = all pass)
"""

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


def desk_page(ctx, viewport=None):
    """A page on the Consoles tab, with the desk already restored."""
    page = ctx.new_page()
    if viewport:
        page.set_viewport_size(viewport)
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(1800)
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

            # --- scenario 5a: the shell touched no browser storage ------------
            store_len = page.evaluate("() => localStorage.length")
            check(
                "the shell leaves localStorage completely empty",
                store_len == 0,
                f"got={store_len}",
            )
            js_hits = Path(CONSOLE_JS).read_text(encoding="utf-8").count("localStorage")
            check(
                "…because wb-console.js names localStorage zero times",
                js_hits == 0,
                f"got={js_hits}",
            )

            page.screenshot(path=os.path.join(SHOT_DIR, "327-desk-daemon-2026-07-26.png"))
            ctx_a.close()

            # --- scenario 3: a SECOND browser profile restores the desk -------
            ctx_b = browser.new_context(viewport={"width": 1400, "height": 900})
            page_b = desk_page(ctx_b)
            check(
                "a fresh browser profile starts with empty storage",
                page_b.evaluate("() => localStorage.length") == 0,
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
                "…still holding no browser-side desk key",
                page_b.evaluate("() => localStorage.length") == 0,
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

            # --- scenario 4: a smaller viewport restores fully on-screen ------
            # Restore the two-window desk saved at 1400x900 before shrinking.
            http("PUT", "api/desk", [desk_record(live_ids[0], 200, 900, 500)])
            ctx_c = browser.new_context(viewport={"width": 800, "height": 600})
            page_c = desk_page(ctx_c, viewport={"width": 800, "height": 600})
            page_c.wait_for_timeout(800)
            fits = page_c.evaluate(
                "() => { const ws = document.getElementById('workspace');"
                " return [...document.querySelectorAll('.session-window')].map((w) =>"
                "   w.offsetLeft >= 0 && w.offsetTop >= 0 &&"
                "   w.offsetLeft + w.offsetWidth <= ws.clientWidth &&"
                "   w.offsetTop + w.offsetHeight <= ws.clientHeight); }"
            )
            check(
                "a desk saved at 1400x900 restores a window at 800x600",
                len(fits) >= 1,
                f"windows={len(fits)}",
            )
            check(
                "…fully on-screen — `clampAll` unchanged, now CALLED after a restore",
                all(fits),
                f"got={fits}",
            )
            ctx_c.close()
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

    ok = all(results) and len(results) >= 20
    print(f"\n{sum(results)}/{len(results)} checks passed")
    if ok:
        print("DESK IS DAEMON STATE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
