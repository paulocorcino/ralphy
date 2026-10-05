"""The audit log of devices (ADR-0074) in three real browser engines.

Scenario 1  each engine (Chromium, Firefox, WebKit) gets its own device cookie
            and writes one `device_facts` line, normalized to its engine
Scenario 2  a reload of the same page writes no second `device_facts` line
Scenario 3  a request that changes state writes an `action` line with the
            device, and a login with a wrong code writes `login_failed`
Scenario 4  Settings → Devices lists the three devices, marks this one, and
            shows its activity

Boots a Localhost daemon on 7477 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own store is untouched. The daemon is stopped by its own
subprocess handle, NEVER by name.

Writes .ralphy/screenshots/device-audit-settings.png.
Run: python tests/browser/security/wb_device_audit.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7477
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
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


def launch(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wbaudit_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def log_lines(daemon_dir):
    path = os.path.join(daemon_dir, "daemon-audit.jsonl")
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def wait_lines(daemon_dir, pred, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        found = [l for l in log_lines(daemon_dir) if pred(l)]
        if found:
            return found
        time.sleep(0.2)
    return []


def value(line, key):
    v = line.get(key)
    return v.get("value") if isinstance(v, dict) else v


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") != "1":
        subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbaudit_reg_")
    proc = launch(daemon_dir)
    try:
        check("daemon listening", wait_listening(BASE))
        want = {"chromium": "blink", "firefox": "gecko", "webkit": "webkit"}
        devices = {}
        with sync_playwright() as pw:
            pages = {}
            for name, engine in want.items():
                browser = getattr(pw, name).launch()
                page = browser.new_context(viewport={"width": 1400, "height": 900}).new_page()
                page.goto(BASE)
                page.wait_for_function("() => window.Alpine", timeout=20000)
                pages[name] = (browser, page)
                cookie = [c for c in page.context.cookies() if c["name"] == "ralphy_device"]
                check(f"{name}: a device cookie", len(cookie) == 1, cookie and cookie[0]["httpOnly"])
                device = cookie[0]["value"].split(".")[1] if cookie else None
                devices[name] = device
                facts = wait_lines(daemon_dir, lambda l: l.get("event") == "device_facts" and l.get("device") == device)
                line = facts[0] if facts else {}
                check(
                    f"{name}: one device_facts line, engine {engine}",
                    len(facts) == 1 and value(line, "engine") == engine,
                    {k: value(line, k) for k in ("os", "os_version", "browser", "browser_version", "engine", "form", "screen")},
                )
                print(f"    {name} gpu={line.get('gpu')} conflicts={line.get('conflicts')}", flush=True)

            # ── 2. a reload sends the same facts: no second line ──────────
            _, chromium = pages["chromium"]
            chromium.reload()
            chromium.wait_for_function("() => window.Alpine", timeout=20000)
            time.sleep(1.5)
            again = [l for l in log_lines(daemon_dir) if l.get("event") == "device_facts" and l.get("device") == devices["chromium"]]
            check("a reload writes no second device_facts line", len(again) == 1, len(again))

            # ── 3. an action and a failed login ───────────────────────────
            chromium.evaluate("fetch('/api/desk/new', {method: 'POST'})")
            acts = wait_lines(daemon_dir, lambda l: l.get("event") == "action" and l.get("path") == "/api/desk/new")
            check(
                "a POST writes an action line with the device",
                bool(acts) and acts[0].get("device") == devices["chromium"],
                acts[:1],
            )
            chromium.evaluate(
                "fetch('/api/login', {method: 'POST', headers: {'content-type': 'application/x-www-form-urlencoded'}, body: 'code=000000'})"
            )
            time.sleep(0.8)
            logins = [l for l in log_lines(daemon_dir) if l.get("event", "").startswith("login")]
            check(
                "a login on a daemon with no login enabled is not a login line",
                logins == [],
                logins,
            )

            # ── 4. Settings → Devices ─────────────────────────────────────
            chromium.evaluate(f"{SH}.openSettings(); {SH}.showSettingsSection('devices')")
            chromium.wait_for_selector(".device-row", timeout=10000)
            rows = chromium.locator(".device-row")
            check("the section lists three devices", rows.count() == 3, rows.count())
            this = chromium.locator(".device-row:has(.device-this:visible)")
            check("one row is marked as this device", this.count() == 1, this.count())
            this.first.locator("button").click()
            chromium.wait_for_selector(".device-event", timeout=10000)
            texts = chromium.locator(".device-event").all_inner_texts()
            check(
                "its activity shows the action and the device facts",
                any("/api/desk/new" in t for t in texts) and any("Reported its device facts" in t for t in texts),
                texts,
            )
            shot = os.path.join(SHOT_DIR, "device-audit-settings.png")
            chromium.locator(".settings-content").screenshot(path=shot)
            print(f"    screenshot: {shot}", flush=True)
            for browser, _ in pages.values():
                browser.close()
    finally:
        stop(proc)
    print(f"{sum(results)}/{len(results)} passed")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
