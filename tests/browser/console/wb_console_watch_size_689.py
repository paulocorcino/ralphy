"""#689 browser check: a watcher paints at the session size.

One REAL daemon on a scratch `RALPHY_DAEMON_DIR` (the operator's own desk and
login policy are untouched), with `RALPHY_DAEMON_AGENT_OVERRIDE` set to
`session_test_child`. The desk holds one console, floating at 680x500.

Page A (context A, 1400x900) drives the console. Page B is a second browser
context (another holder, 1100x800), so it restores the same console as a
WATCHER.

1  B's terminal has A's rows and columns.
2  B asks for a fit with a larger font: its terminal keeps A's size and its
   body scrolls instead (class `following`).
3  A maximizes its console: the session size grows, and B follows it.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_watch_size_689.py   (exit 0 = all pass)
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

PORT = 7531
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")

HELPERS = """() => {
  window.__W = () => document.querySelector('#stage .session-window');
}"""
SIZE = """() => { const t = __W()?._term; return !t ? null : {
  rows: t.term.rows, cols: t.term.cols, watching: t.watching,
  following: __W().querySelector('.session-body').classList.contains('following') }; }"""

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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild or the browser loads an old bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True)


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb689w_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb689@example.com"],
        ["git", "config", "user.name", "wb689"],
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


def write_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_bytes(
        (
            "[[windows]]\n"
            'id = "w-1"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            'consoleName = "w-1"\n'
            "ts = 100\n"
            "rect = { left = 40, top = 40, width = 680, height = 500 }\n"
        ).encode("utf-8")
    )


def launch(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb689w_empty_")
    env = dict(
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
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


def open_page(browser, viewport):
    page = browser.new_context(viewport=viewport).new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate("() => Alpine.store('tabs').activate('consoles')")
    page.wait_for_function("() => !!document.querySelector('#stage .session-window')", timeout=20000)
    page.evaluate(HELPERS)
    page.wait_for_function("() => __W()?._term?.term?.cols > 0", timeout=20000)
    return page


def wait_for(page, js, arg=None, timeout_s=15):
    try:
        page.wait_for_function(js, arg=arg, timeout=int(timeout_s * 1000))
        return True
    except Exception:
        return False


def same_size(page, size):
    """Waits until the page's terminal has `size`; returns its size then."""
    wait_for(page, "(s) => { const t = __W()?._term?.term; return !!t && t.rows === s.rows && t.cols === s.cols; }", size)
    return page.evaluate(SIZE)


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb689w_daemon_")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    write_desk(daemon_dir, slug)
    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page_a = open_page(browser, {"width": 1400, "height": 900})
            page_a.wait_for_timeout(1500)
            a = page_a.evaluate(SIZE)
            check("A drives the console", a and not a["watching"], f"got={a}")

            # 1. B, another holder in a smaller window, lands as a watcher at A's size.
            page_b = open_page(browser, {"width": 1100, "height": 800})
            # The watch socket opens after a refused claim and its retries.
            wait_for(
                page_b,
                "() => __W()?._term?.watching === true"
                " && !!__W().querySelector('.session-body.following')",
                timeout_s=40,
            )
            b = same_size(page_b, {"rows": a["rows"], "cols": a["cols"]})
            check("B watches the console", b and b["watching"], f"got={b}")
            check(
                "1 B's terminal has A's rows and columns",
                b and (b["rows"], b["cols"]) == (a["rows"], a["cols"]),
                f"A={a} B={b}",
            )

            # 2. B's own fit request with a larger font changes nothing.
            font = page_b.evaluate(
                "() => { const t = __W()._term; const f = t.term.options.fontSize;"
                " t.term.options.fontSize = f + 6; t.fit.fit(); return f; }"
            )
            page_b.wait_for_timeout(1000)
            b = page_b.evaluate(SIZE)
            check(
                "2 a fit request in B keeps A's size",
                b and (b["rows"], b["cols"]) == (a["rows"], a["cols"]),
                f"A={a} B={b}",
            )
            check("…and B's body scrolls instead", b and b["following"], f"got={b}")
            scroll = page_b.evaluate(
                "() => { const b = __W().querySelector('.session-body'); return [b.scrollWidth, b.clientWidth]; }"
            )
            check("…its content is wider than its box", scroll[0] > scroll[1], f"scrollWidth, clientWidth={scroll}")

            # 3. A maximizes: the session size grows and B follows it.
            page_a.evaluate("() => __W().querySelector('.session-max').click()")
            grown = wait_for(page_a, "(c) => __W()._term.term.cols > c", a["cols"])
            a2 = page_a.evaluate(SIZE)
            check("3 A's maximized console is wider", grown, f"before={a} after={a2}")
            b2 = same_size(page_b, {"rows": a2["rows"], "cols": a2["cols"]})
            check(
                "…and B follows the new session size",
                b2 and (b2["rows"], b2["cols"]) == (a2["rows"], a2["cols"]),
                f"A={a2} B={b2}",
            )
            page_b.evaluate("(f) => { __W()._term.term.options.fontSize = f; }", font)
            browser.close()
    finally:
        stop(proc)
    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
