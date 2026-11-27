"""Browser check: the roster serves the OpenCode version of this machine; the menu shows none.

One Playwright pass over a REAL daemon on a SCRATCH `RALPHY_DAEMON_DIR`. The
expected version is what `opencode --version` prints here; `/api/agents` must
serve it on the `opencode` row only, and no row of the "New console" menu shows
a version: the label is the plain agent name.

The daemon is stopped by its own subprocess handle, never by name.
Run: python tests/browser/agents/wb_agents_version.py   (exit 0 = all pass)
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7399
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
MENUS = "Alpine.$data(document.querySelector('[x-data=\"wbConsoleMenus\"]'))"
MENU = ".console-menu"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def machine_opencode_version():
    program = shutil.which("opencode") or os.path.join(os.path.expanduser("~"), ".opencode", "bin", "opencode")
    out = subprocess.run([program, "--version"], capture_output=True, encoding="utf-8", timeout=30, check=True)
    word = out.stdout.strip().splitlines()[0].split()[-1]
    return re.sub(r"^v", "", word)


def wait_listening(timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(BASE, timeout=1)
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


def main():
    expected = machine_opencode_version()
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbver_reg_")
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_listening():
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport={"width": 1400, "height": 900}).new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            served = page.evaluate("async () => (await fetch('/api/agents')).json()")
            with_version = [r["id"] for r in served if "version" in r]
            check("only the opencode row carries a version", with_version == ["opencode"], f"got={with_version}")
            page.evaluate(f"() => {{ {MENUS}.agentMenu = true; }}")
            page.wait_for_timeout(400)
            labels = page.locator(f"{MENU} .dropdown-item span:not(.row-live):not(.row-new)").all_inner_texts()
            served_version = next((r.get("version") for r in served if r["id"] == "opencode"), None)
            check("the roster serves the version of this machine", served_version == expected, f"want={expected!r} got={served_version!r}")
            check("the opencode menu row is the plain name, with no version", "opencode" in labels, f"got={labels}")
            check("the other rows keep their plain label", "claude" in labels and "codex" in labels, f"got={labels}")
            browser.close()
    finally:
        stop(proc)
    ok = all(results) and len(results) == 4
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
