"""A page whose first desk read fails never moves a saved console.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so
the operator's own desk is untouched.

Scenario   a desk with one shell record at a saved rect, its session live.
           A new page loads while `GET /api/desk` fails (a host that just woke,
           a tunnel that drops one request). The desk then answers again, and
           the operator opens a second console on that page. The saved record
           keeps its rect, and the page shows the console at that rect.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes .ralphy/screenshots/desk-failed-load-2026-10-04.png.
Run: python tests/browser/desk/wb_desk_failed_load.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile

from playwright.sync_api import sync_playwright

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_desk_double_launch as base  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

SHOT = "desk-failed-load-2026-10-04.png"
SAVED = {"left": 900.0, "top": 500.0, "width": 420.0, "height": 260.0}


def desk_windows():
    _, body = base.http("GET", "api/desk")
    return json.loads(body)["windows"]


def main():
    os.makedirs(base.SHOT_DIR, exist_ok=True)
    base.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbfailed_reg_")
    slug = base.register_fixture(daemon_dir, base.make_fixture_repo())
    proc = subprocess.Popen(
        [base.EXE, "daemon", "--port", str(base.PORT)],
        env=base.daemon_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not base.wait_listening(base.BASE):
            base.check("the daemon listens", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            rec = base.record("w-keep", slug, "console", "console", SAVED["left"], 1)
            rec["rect"] = dict(SAVED)
            status, _ = base.put_desk([rec])
            base.check("the desk with one shell record is stored", status == 200)

            # A first page starts the shell under its record, then goes away.
            ctx_a = browser.new_context(viewport={"width": 1400, "height": 900})
            page_a = ctx_a.new_page()
            page_a.goto(base.BASE)
            base.open_consoles_tab(page_a)
            page_a.wait_for_timeout(4000)
            ctx_a.close()
            _, body = base.http("GET", "api/sessions")
            sessions = json.loads(body)
            base.check(
                "the shell runs under its record",
                [s.get("record") for s in sessions] == ["w-keep"],
                f"got={[s.get('record') for s in sessions]}",
            )

            # A second page loads while the desk read fails once.
            ctx_b = browser.new_context(viewport={"width": 1024, "height": 768})
            page_b = ctx_b.new_page()
            failed = {"n": 0}

            def fail_first_desk_read(route):
                if route.request.method == "GET" and failed["n"] == 0:
                    failed["n"] += 1
                    route.abort()
                else:
                    route.continue_()

            page_b.route("**/api/desk", fail_first_desk_read)
            page_b.goto(base.BASE)
            base.open_consoles_tab(page_b)
            page_b.wait_for_timeout(3000)
            base.check("the first desk read failed", failed["n"] == 1)
            windows = page_b.locator(".session-window").count()
            base.check("the page shows one console, not two", windows == 1, f"got={windows}")

            # The desk answers again: the page reads it again by itself.
            opened = page_b.evaluate(f"() => window.WBConsole.open({{ repo: {json.dumps(slug)}, plain: true }})")
            base.check("the operator opens a second console", opened is True, f"got={opened}")
            page_b.wait_for_timeout(2500)

            kept = next((w for w in desk_windows() if w["id"] == "w-keep"), None)
            base.check("the saved record is still on the desk", kept is not None)
            base.check(
                "the saved record keeps its rect",
                kept is not None and kept["rect"] == SAVED,
                f"got={kept and kept['rect']}",
            )
            shown = page_b.evaluate(
                """() => [...document.querySelectorAll('.session-window')]
                    .filter((w) => w._deskId === 'w-keep')
                    .map((w) => [parseFloat(w.style.left), parseFloat(w.style.top)])"""
            )
            base.check(
                "the page shows the console at its saved rect",
                shown == [[SAVED["left"], SAVED["top"]]],
                f"got={shown}",
            )
            page_b.screenshot(path=os.path.join(base.SHOT_DIR, SHOT))
            ctx_b.close()
            browser.close()
    finally:
        base.stop(proc)

    passed = sum(base.results)
    print(f"\n{passed}/{len(base.results)} checks passed")
    sys.exit(0 if base.results and all(base.results) else 1)


if __name__ == "__main__":
    main()
