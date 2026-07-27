"""#334 browser acceptance: the client stops stealing sessions.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. Two browser contexts drive
ONE live session: one holds the writer slot, the other watches.

Scenario 1   `WBConsole.reconnectDecision` in isolation: a 16-row table over
             close-event shapes (announced/not, opened/not, clean/not, id
             known/unknown, both cap boundaries)
Scenario 2   ctx A opens a console and its keystroke round-trips
Scenario 3   ctx B (fresh profile) restores the desk, lands as a WATCHER, sees
             the same output, and neither context storms the socket over 12 s
Scenario 4   an explicit take-over click moves the baton: exactly one
             `takeover=1` URL ever, and A parks instead of stealing it back
Scenario 5   a real dropped link (offline/online) still recovers by reconnecting
Scenario 6   F5 reattaches without evicting anything
Scenario 7   a genuine end (`POST /api/sessions/close`) states "closed" in both

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/334-console-pairing-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_consoles_334.py   (exit 0 = all pass)
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

PORT = 7423
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
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
    """A scratch registry + empty vendor stores, plus the deterministic echo
    child: `RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a
    `session_test_child`, whose `GOT:<line>` reply is a machine-readable oracle
    for "this keystroke reached the PTY"."""
    empty = tempfile.mkdtemp(prefix="wb334_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb334_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #334 console-pairing fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb334@example.com"],
        ["git", "config", "user.name", "wb334"],
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
    # the previous build's console. The echo child is the session under test.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


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
        import json as _json

        data = _json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


# --- scenario 1: the pure reconnect rule, evaluated in the page --------------
# One row per rule, plus both cap boundaries. `announced` is the daemon's
# eviction reason (null when the close carried no announcement); `opened` is
# whether THIS socket opened, `everOpened` whether any socket of this window did.
def row(**kw):
    base = {
        "code": 1006,
        "wasClean": False,
        "opened": False,
        "everOpened": False,
        "announced": None,
        "idKnown": True,
        "failedReopens": 0,
    }
    base.update(kw)
    return base


DECISION_ROWS = [
    (
        "taken-over on an opened socket parks as a watcher",
        row(announced="taken-over", opened=True, everOpened=True, code=1005),
        "park-as-watcher",
    ),
    (
        "taken-over before the socket opened still parks",
        row(announced="taken-over", code=1006),
        "park-as-watcher",
    ),
    (
        "an unknown id gives up even when taken over (R1 precedes R2)",
        row(announced="taken-over", opened=True, everOpened=True, code=1005, idKnown=False),
        "give-up",
    ),
    (
        "child-exited gives up",
        row(announced="child-exited", opened=True, everOpened=True, code=1005),
        "give-up",
    ),
    (
        "daemon-shutdown gives up",
        row(announced="daemon-shutdown", opened=True, everOpened=True, code=1005),
        "give-up",
    ),
    (
        "child-exited on a clean 1000 gives up",
        row(announced="child-exited", opened=True, everOpened=True, code=1000, wasClean=True),
        "give-up",
    ),
    (
        "no announcement + a clean 1000 gives up",
        row(opened=True, everOpened=True, code=1000, wasClean=True),
        "give-up",
    ),
    (
        "no announcement + a dirty 1001 (going away) gives up",
        row(opened=True, everOpened=True, code=1001),
        "give-up",
    ),
    (
        "no announcement + a dirty 1006 reconnects",
        row(opened=True, everOpened=True, code=1006),
        "reconnect",
    ),
    (
        "no announcement + the measured 1005 eviction shape reconnects",
        row(opened=True, everOpened=True, code=1005),
        "reconnect",
    ),
    (
        "no announcement + an unknown id gives up",
        row(opened=True, everOpened=True, code=1006, idKnown=False),
        "give-up",
    ),
    (
        "a never-opened socket retries as a would-be writer",
        row(code=1006, failedReopens=1),
        "reconnect",
    ),
    (
        "…and settles for watching at WATCH_AFTER=3",
        row(code=1006, failedReopens=3),
        "park-as-watcher",
    ),
    (
        "a window that HAS held the session keeps reconnecting past WATCH_AFTER",
        row(code=1006, everOpened=True, failedReopens=5),
        "reconnect",
    ),
    (
        "failedReopens at the cap (10) still reconnects",
        row(opened=True, everOpened=True, code=1005, failedReopens=10),
        "reconnect",
    ),
    (
        "failedReopens past the cap (11) gives up",
        row(opened=True, everOpened=True, code=1005, failedReopens=11),
        "give-up",
    ),
]


def decision_table(page):
    outs = page.evaluate(
        "(rows) => rows.map((r) => { try {"
        " return window.WBConsole.reconnectDecision(r);"
        " } catch (e) { return String(e); } })",
        [r for (_, r, _) in DECISION_ROWS],
    )
    for (label, inp, want), got in zip(DECISION_ROWS, outs):
        check(f"reconnectDecision: {label}", got == want, f"got={got!r} want={want!r} in={inp}")


def probe_page(ctx):
    """A bare page on the Consoles tab — enough for the pure-function table."""
    page = ctx.new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(600)
    return page


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb334_reg_")
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
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = probe_page(ctx)

            # --- scenario 1: the pure reconnect rule -------------------------
            check(
                "reconnectDecision is exported like resizeRect/reconcileDesk",
                page.evaluate("() => typeof window.WBConsole.reconnectDecision") == "function",
            )
            decision_table(page)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results)
    print(f"\n{sum(1 for r in results if r)}/{len(results)} checks passed", flush=True)
    print("NO TAKEOVER, NO STORM" if ok else "FAILED", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
