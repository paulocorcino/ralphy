"""Console launches in the audit log, and refused console sockets in the
daemon log, browser acceptance.

A console the page opens writes a `console_launch` line in
`daemon-audit.jsonl` with the device of the browser (ADR-0074, amendment
2026-10-05). A console socket the daemon refuses writes one
`console socket refused` line in the daemon log.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`.
PORT 7465 and the two-console desk are `wb_console_detach.py`'s.

A1  each restored console wrote one console_launch line, agent console
A2  the lines carry the device of the browser and a holder
A3  the Devices panel shows the launch by the project name, not the registry key
R1  a reattach to an unknown session is refused with 404, and logged

The daemon is stopped by its own subprocess handle, NEVER by name.

Run: python tests/browser/console/wb_console_audit.py   (exit 0 = all pass)
"""

import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_console_detach as D  # noqa: E402  the daemon, desk and log helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

T = D.T
BASE = D.BASE
FLOOR = 6  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def launches(daemon_dir):
    path = Path(daemon_dir, "daemon-audit.jsonl")
    if not path.exists():
        return []
    lines = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]
    return [line for line in lines if line.get("event") == "console_launch"]


def refused_lines(log_path):
    text = Path(log_path).read_text(encoding="utf-8", errors="replace")
    return [line for line in text.splitlines() if "console socket refused" in line]


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbaudit_daemon_")
    log_path = os.path.join(daemon_dir, "daemon-stderr.log")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    D.write_desk(daemon_dir, slug)
    proc = D.launch(daemon_dir, log_path)
    errors = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(D.VIEW))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page, want=2)
            page.wait_for_function(
                "() => [...document.querySelectorAll('#stage .session-window')]"
                ".every((w) => w._term?.term?.cols > 0)",
                timeout=30000,
            )
            device = next(
                (c["value"].split(".")[1] for c in ctx.cookies() if c["name"] == "ralphy_device"),
                None,
            )

            # A1-A2 ------------------------------------------------------------
            got = launches(daemon_dir)
            check("A1 two console_launch lines", len(got) == 2, json.dumps(got))
            check("A1 the agent is console", all(l.get("agent") == "console" for l in got))
            check(
                "A2 the lines carry the browser's device and a holder",
                device is not None and all(l.get("device") == device and l.get("holder") for l in got),
                f"{device} {json.dumps(got)}",
            )

            # A3 ---------------------------------------------------------------
            text = page.evaluate(
                "async (d) => (await (await fetch('/api/audit/events?device=' + d)).json())"
                ".events.map((e) => window.WBDevices.eventLine(e))",
                device,
            )
            fixture_name = Path(fixture).name
            check(
                "A3 the Devices panel names the project, not the registry key",
                f"Opened a console in {fixture_name}" in text,
                f"{fixture_name} {text}",
            )

            # R1 ---------------------------------------------------------------
            page.evaluate(
                "() => new Promise((done) => {"
                " const ws = new WebSocket(`ws://${location.host}/ws/session?id=999999`);"
                " ws.onerror = ws.onclose = () => done(); })"
            )
            deadline = time.time() + 5
            while not refused_lines(log_path) and time.time() < deadline:
                time.sleep(0.2)
            got = refused_lines(log_path)
            check(
                "R1 the refused reattach is logged with status 404",
                any("status=404" in l and "launch=false" in l for l in got),
                str(got),
            )
            check("no page errors", not errors, str(errors[:3]))
            browser.close()
    finally:
        T.stop(proc)
    check(f"floor: at least {FLOOR} checks ran", len(results) >= FLOOR, str(len(results)))


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"\n{passed}/{len(results)} passed")
    sys.exit(0 if results and all(results) else 1)
