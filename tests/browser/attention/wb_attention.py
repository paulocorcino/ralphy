"""The attention list in a real browser: a Claude console that asks for a
permission is one request on the rail, with the count in the tab title, and
it leaves the list when the permission is answered.

One Playwright pass over a REAL daemon. The helper child stands in for the
vendor; the script stands in for `ralphy hook status` by appending lines to
the session's `<store>/sessions/<id>.agent-status.jsonl`. The daemon's tail
folds them, `GET /api/attention` lists the request, and the page reads the
list on the `sessions.dirty` push.

Scenario 1  a claude console, no request: the badge is hidden, the title is `Ralphy`
Scenario 2  a `PermissionRequest` line for `Bash` -> the badge says `1`, the
            title is `(1) Ralphy`, the panel has a row with `permission: Bash`;
            screenshot HERE
Scenario 3  "Go to the console" closes the panel and the console stays shown
Scenario 4  a `PostToolUse` line for `Bash` -> the badge is hidden, the title
            is `Ralphy`
Scenario 5  no page errors

Boots a Localhost daemon on 7471 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name. Built on the
helpers of `agents/wb_agent_state.py`; `RALPHY_TEST_SKIP_BUILD=1` and
`RALPHY_TEST_TARGET_DIR` work as there.

Prints `PASS <name>` or `FAIL <name>` per check. Writes
.ralphy/screenshots/attention-list-2026-10-28.png (`-linux` off Windows).
Run: python tests/browser/attention/wb_attention.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "agents"))

from playwright.sync_api import sync_playwright  # noqa: E402

import wb_agent_state as agents  # noqa: E402

PORT = 7471
BASE = f"http://127.0.0.1:{PORT}/"
SHOT = "attention-list-2026-10-28" + ("" if os.name == "nt" else "-linux") + ".png"

BADGE = (
    "() => { const b = document.querySelector('.rail-attention .attention-badge');"
    "  return b && b.offsetParent !== null ? b.textContent.trim() : null; }"
)
ROWS = (
    "() => [...document.querySelectorAll('.attention-panel .attention-row')]"
    "  .filter(r => r.offsetParent !== null)"
    "  .map(r => ({ detail: (r.querySelector('.attention-detail') || {}).textContent || '',"
    "    vendor: (r.querySelector('.attention-vendor') || {}).textContent || '' }))"
)
PANEL_OPEN = "() => { const p = document.querySelector('.attention-panel'); return !!p && p.offsetParent !== null; }"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'} {name} {detail}".rstrip(), flush=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [agents.EXE, "daemon", "--port", str(PORT)],
        env=agents.empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def sessions():
    with urllib.request.urlopen(BASE + "api/sessions", timeout=5) as r:
        return json.load(r)


def wait_true(page, js, timeout=15000):
    try:
        page.wait_for_function(js, timeout=timeout)
        return True
    except Exception:
        return False


def main():
    os.makedirs(agents.SHOT_DIR, exist_ok=True)
    agents.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbattention_reg_")
    fixture = agents.seed("wbattention_", "plain")
    slug = agents.register_fixture(daemon_dir, str(fixture))

    proc = launch(daemon_dir)
    try:
        if not agents.wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            agents.wait_shell(page)

            # --- scenario 1: a console, nothing waits ---------------------------
            agents.open_project(page, slug)
            page.evaluate(f"() => {agents.MENUS}.newConsole('claude')")
            page.wait_for_function(f"() => ({agents.WINDOWS})() === 1", timeout=15000)
            check("the child printed READY", agents.wait_flat_contains(page, 0, agents.READY))
            sid = sessions()[0]["id"]
            page.wait_for_timeout(1500)
            check("no badge before a request", page.evaluate(BADGE) is None, f"got={page.evaluate(BADGE)!r}")
            check("the title has no count", page.title() == "Ralphy", f"got={page.title()!r}")

            # --- scenario 2: a permission request -------------------------------
            agents.hook(daemon_dir, sid, "PermissionRequest", "Bash", {"command": "ls"})
            ok = wait_true(page, f"() => ({BADGE})() === '1'")
            check("badge", ok, f"got={page.evaluate(BADGE)!r}")
            ok = wait_true(page, "() => document.title === '(1) Ralphy'", timeout=5000)
            check("title", ok, f"got={page.title()!r}")
            page.locator(".rail-attention > button").click()
            wait_true(page, PANEL_OPEN, timeout=5000)
            rows = page.evaluate(ROWS)
            ok = len(rows) == 1 and rows[0]["detail"].strip() == "permission: Bash" and rows[0]["vendor"].strip() == "claude"
            check("row", ok, f"got={rows!r}")
            page.screenshot(path=os.path.join(agents.SHOT_DIR, SHOT))

            # --- scenario 3: go to the console ----------------------------------
            page.locator(".attention-panel .attention-go").first.click()
            ok = wait_true(page, f"() => !({PANEL_OPEN})()", timeout=5000)
            check("go to the console closes the panel", ok)
            check("the console is still shown", page.evaluate(f"() => ({agents.WINDOWS})()") == 1)

            # --- scenario 4: the permission is answered -------------------------
            agents.hook(daemon_dir, sid, "PostToolUse", "Bash", {"command": "ls"})
            ok = wait_true(page, f"() => ({BADGE})() === null && document.title === 'Ralphy'")
            check("cleared", ok, f"badge={page.evaluate(BADGE)!r} title={page.title()!r}")

            # --- scenario 5 -----------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        agents.stop(proc)

    check_floor = 11
    if len(results) != check_floor:
        print(f"FAIL the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
