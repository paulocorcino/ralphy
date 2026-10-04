"""Two pages that load one desk at the same time start each console once.

ADR-0050 amendment 2026-10-04. One Playwright pass over a REAL daemon on a
scratch `RALPHY_DAEMON_DIR`, so the operator's own desk is untouched.

Scenario 1   a desk with 3 free-console records whose sessions are gone; two
             browser profiles load it at the same moment. The daemon holds 3
             sessions, not 6, each names its record, the desk keeps 3 records
             at their saved rects, and each console is read-only on exactly one
             of the two pages.
Scenario 2   a desk with 30 records is full: opening a new console is refused
             with a message, and no record is cut.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes .ralphy/screenshots/desk-double-launch-2026-10-04.png.
Run: python tests/browser/desk/wb_desk_double_launch.py   (exit 0 = all pass)
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

PORT = 7429
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SHOT = "desk-double-launch-2026-10-04.png"
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


def daemon_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wbdouble_empty_")
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
    d = tempfile.mkdtemp(prefix="wbdouble_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbdouble@example.com"],
        ["git", "config", "user.name", "wbdouble"],
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
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
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


def record(rid, repo, kind, agent, left, ts):
    return {
        "id": rid,
        "repo": repo,
        "agent": agent,
        "kind": kind,
        "rect": {"left": left, "top": 40.0, "width": 420.0, "height": 260.0},
        "max": False,
        "sessionId": None,
        "ts": ts,
    }


def put_desk(windows, removed=()):
    # The PUT is a list of desk changes (ADR-0050 amendment 2026-10-04): each
    # window is a `create` (a record that exists takes only its session), and
    # each id in `removed` is a `remove`. A record the body does not name
    # stays as it is.
    changes = [{"op": "create", "type": "window", "record": w} for w in windows]
    changes += [{"op": "remove", "type": "window", "id": rid} for rid in removed]
    return http("PUT", "api/desk", {"seq": 1, "changes": changes})


def close_all_sessions():
    _, body = http("GET", "api/sessions")
    for s in json.loads(body):
        http("POST", f"api/sessions/close?id={s['id']}&repo={urllib.request.quote(s['repo'], safe='')}")


def open_consoles_tab(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbdouble_reg_")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=daemon_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_listening(BASE):
            check("the daemon listens", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # --- scenario 1: two pages relaunch one desk at once -------------
            ids = ["w-shell-a", "w-shell-b", "w-shell-c"]
            lefts = [20.0, 480.0, 940.0]
            status, _ = put_desk(
                [record(i, slug, "console", "console", left, n + 1) for n, (i, left) in enumerate(zip(ids, lefts))]
            )
            check("the desk with 3 shell records is stored", status == 200)

            ctx_a = browser.new_context(viewport={"width": 1400, "height": 900})
            ctx_b = browser.new_context(viewport={"width": 1400, "height": 900})
            page_a = ctx_a.new_page()
            page_b = ctx_b.new_page()
            # Both loads start before either page restores its desk.
            page_a.goto(BASE, wait_until="commit")
            page_b.goto(BASE, wait_until="commit")
            open_consoles_tab(page_a)
            open_consoles_tab(page_b)
            page_a.wait_for_timeout(6000)

            _, body = http("GET", "api/sessions")
            sessions = json.loads(body)
            check("3 records start 3 sessions, not 6", len(sessions) == 3, f"got={len(sessions)}")
            check(
                "each session names its record",
                sorted(s.get("record") for s in sessions) == ids,
                f"got={[s.get('record') for s in sessions]}",
            )
            _, body = http("GET", "api/desk")
            desk = json.loads(body)["windows"]
            check("the desk keeps 3 records", sorted(w["id"] for w in desk) == ids, f"got={[w['id'] for w in desk]}")
            check(
                "every record keeps its saved rect",
                all(w["rect"]["left"] == lefts[ids.index(w["id"])] for w in desk if w["id"] in ids),
                f"got={[(w['id'], w['rect']['left']) for w in desk]}",
            )
            windows_a = page_a.locator(".session-window").count()
            windows_b = page_b.locator(".session-window").count()
            check("each page shows 3 consoles", windows_a == 3 and windows_b == 3, f"a={windows_a} b={windows_b}")
            parked = page_a.locator(".session-parked").count() + page_b.locator(".session-parked").count()
            check("each console is read-only on exactly one page", parked == 3, f"got={parked}")
            page_b.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            ctx_a.close()
            ctx_b.close()

            # --- scenario 2: a full desk refuses a new console ---------------
            close_all_sessions()
            full = [
                record(f"w-full-{n}", slug, "agent", "claude", 20.0 + n, n + 1) for n in range(30)
            ]
            status, _ = put_desk(full, removed=ids)
            check("a desk with 30 agent records is stored", status == 200)
            ctx_c = browser.new_context(viewport={"width": 1400, "height": 900})
            page_c = ctx_c.new_page()
            page_c.goto(BASE)
            open_consoles_tab(page_c)
            page_c.evaluate(f"() => {{ {SH}.openSlug = {json.dumps(slug)}; }}")
            page_c.wait_for_timeout(2500)
            opened = page_c.evaluate(
                f"() => window.WBConsole.open({{ repo: {json.dumps(slug)}, plain: true }})"
            )
            check("opening a 31st console is refused", opened is False, f"got={opened}")
            toast = page_c.get_by_text("You can have at most 30 consoles. Close one first.")
            check("…with a message", toast.count() >= 1)
            page_c.wait_for_timeout(800)
            _, body = http("GET", "api/desk")
            check("…and no record is cut", len(json.loads(body)["windows"]) == 30)
            _, body = http("GET", "api/sessions")
            check("…and nothing is started", len(json.loads(body)) == 0, body[:200])
            ctx_c.close()
            browser.close()
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
