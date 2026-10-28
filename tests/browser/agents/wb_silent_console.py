"""Browser acceptance: a Claude console whose hooks send nothing shows a
warning dot that says why.

One Playwright pass over a REAL daemon, built on the helpers of
`wb_agent_state.py`. `RALPHY_EXE_OVERRIDE` names a program that does not
exist, so the hook command the daemon checks at start fails; the helper child
stands in for Claude. The daemon then reports the console as silent on
`/api/sessions`, and the shell's poll paints the dot.

Scenario 1  the daemon is listening
Scenario 2  a claude console: `/api/sessions` carries `source: silent` with a
            reason and no `state`
Scenario 3  the console title bar shows `session-state silent` with the reason
            in its tooltip
Scenario 4  the project row shows `dot silent`, its tooltip has the reason
Scenario 5  the Go-to list row shows the dot and the reason
Scenario 6  the picker of the tab menu shows the dot and the reason
Scenario 7  a screenshot; no page errors

Boots a Localhost daemon on 7461 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name.

Writes .ralphy/screenshots/silent-console-dot.png
Run: python tests/browser/agents/wb_silent_console.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
import time

from playwright.sync_api import sync_playwright

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_agent_state as base  # noqa: E402

base.PORT = 7461
base.BASE = f"http://127.0.0.1:{base.PORT}/"
SHOT = "silent-console-dot.png"
MISSING_EXE = os.path.join(tempfile.gettempdir(), "wb_silent_no_such_ralphy")
GOTO_ROW = (
    "() => [...document.querySelectorAll('.window-menu .window-item .session-state')]"
    "  .map(e => ({ cls: e.className, title: e.title }))"
)
PICKER_ROW = (
    "() => [...document.querySelectorAll('.console-tab-menu .console-tab-item .session-state')]"
    "  .map(e => ({ cls: e.className, title: e.title }))"
)
PROJECT_ROW = "() => { const d = document.querySelector('li.project .dot'); return d ? { cls: d.className, title: d.title } : null; }"
check = base.check


def silent_block():
    for row in base.sessions():
        block = row.get("agent_state") or {}
        if block.get("source") == "silent":
            return block
    return None


def main():
    shot_dir = base.SHOT_DIR
    os.makedirs(shot_dir, exist_ok=True)
    base.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbsilent_reg_")
    fixture = base.seed("wbsilent_", "plain")
    slug = base.register_fixture(daemon_dir, str(fixture))
    env = dict(base.empty_env(daemon_dir), RALPHY_EXE_OVERRIDE=MISSING_EXE)
    proc = base.subprocess.Popen(
        [base.EXE, "daemon", "--port", str(base.PORT)],
        env=env,
        stdout=base.subprocess.DEVNULL,
        stderr=base.subprocess.DEVNULL,
    )
    try:
        if not base.wait_listening(base.BASE):
            check(f"daemon listening on {base.PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {base.PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport={"width": 1440, "height": 900}).new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(base.BASE)
            base.wait_shell(page)

            base.open_project(page, slug)
            page.evaluate(f"() => {base.MENUS}.newConsole('claude')")
            page.wait_for_function(f"() => ({base.WINDOWS})() === 1", timeout=15000)
            check("the child printed READY", base.wait_flat_contains(page, 0, base.READY))

            # The start check runs on a background thread: wait for its verdict.
            deadline = time.time() + 20
            block = silent_block()
            while block is None and time.time() < deadline:
                time.sleep(0.3)
                block = silent_block()
            check("/api/sessions carries a silent block", block is not None, f"got={block!r}")
            reason = (block or {}).get("reason", "")
            check("…with a reason and no state", bool(reason) and "state" not in block, f"got={block!r}")

            base.wait_console_dot(page, 0, "silent")
            dot = page.evaluate(base.CONSOLE_DOT, 0)
            check(
                "the console title shows the silent dot with the reason",
                dot["title"] == f"Agent state not reported: {reason}",
                f"got={dot!r}",
            )

            page.wait_for_function(f"() => ({base.PROJECT_DOT})() === 'dot silent'", timeout=10000)
            row = page.evaluate(PROJECT_ROW)
            check("the project row shows the silent dot with the reason", reason in row["title"], f"got={row!r}")

            page.evaluate(f"() => {{ {base.MENUS}.windowMenu = true; {base.MENUS}.windowList = WBConsole.list(); }}")
            page.wait_for_timeout(200)
            goto = page.evaluate(GOTO_ROW)
            check(
                "the Go-to row shows the silent dot with the reason",
                len(goto) == 1 and goto[0]["cls"] == "session-state silent" and reason in goto[0]["title"],
                f"got={goto!r}",
            )
            page.evaluate(f"() => {{ {base.MENUS}.windowMenu = false; }}")

            page.click(".tab-add")
            page.wait_for_selector(".console-tab-menu .console-tab-item", state="visible", timeout=5000)
            picker = page.evaluate(PICKER_ROW)
            check(
                "the picker row shows the silent dot with the reason",
                len(picker) == 1
                and picker[0]["cls"] == "session-state silent"
                and picker[0]["title"] == f"Agent state not reported: {reason}",
                f"got={picker!r}",
            )
            page.screenshot(path=os.path.join(shot_dir, SHOT))
            check("screenshot written", os.path.exists(os.path.join(shot_dir, SHOT)))
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        base.stop(proc)

    if len(base.results) != 10:
        print(f"[FAIL] the suite ran {len(base.results)} checks, expected 10", flush=True)
        sys.exit(1)
    sys.exit(0 if all(base.results) else 1)


if __name__ == "__main__":
    main()
