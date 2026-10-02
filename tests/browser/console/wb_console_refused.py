"""A console the daemon cannot start shows the reason.

The browser cannot read the body of a refused WebSocket upgrade, so a refused
launch is announced in a `session-end` frame with `reason: "refused"` and the
reason as `message`. This pass checks, in a real browser over a REAL daemon,
that the console prints that reason instead of a bare `[session closed]`.

Scenario 1  a Gemini console in a repo with no owned Gemini root prints
            `[could not start: gemini: no owned configuration root …]`, names
            the remedy `ralphy run --agent gemini`, and does not print
            `[session closed]`
Scenario 2  no session was created for it (`/api/sessions` is empty)
Scenario 3  an agent console on an unregistered repo prints
            `[could not start: unknown repo]`
Scenario 4  zero `pageerror` events over the whole pass

Boots a Localhost daemon on 7396 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Writes .ralphy/screenshots/console-refused.png.
Run: python tests/browser/console/wb_console_refused.py   (exit 0 = all pass)
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

PORT = 7396
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/console/<this file> -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
WIN = os.name == "nt"
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if WIN else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")

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


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb_refused_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb-refused@example.com"],
        ["git", "config", "user.name", "wb-refused"],
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
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def screen(page, i):
    """The i-th console window's whole terminal buffer as text."""
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w._term && w._term.term && w._term.term.buffer.active;"
        " if (!b) return '';"
        " let out = '';"
        " for (let y = 0; y < b.length; y++) {"
        "   const line = b.getLine(y);"
        "   if (line) out += line.translateToString(true) + '\\n';"
        " }"
        " return out; }",
        i,
    )


def wait_screen(page, i, needle, timeout=10):
    deadline = time.time() + timeout
    got = ""
    while time.time() < deadline:
        got = screen(page, i)
        if needle in got:
            return got
        time.sleep(0.2)
    return got


def flat(text):
    """The terminal wraps long lines; compare without the line breaks."""
    return "".join(text.split("\n"))


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb_refused_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows.
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page_errors = []
            page.on("pageerror", lambda exc: page_errors.append(str(exc)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => !!window.WBConsole", timeout=8000)

            # --- scenario 1: the Gemini refusal is shown in the console --------
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', agent: 'gemini' }})")
            text = flat(wait_screen(page, 0, "could not start"))
            check(
                "the Gemini console prints the refusal",
                "[could not start: gemini: no owned configuration root" in text,
                f"screen={text[-300:]!r}",
            )
            check("the refusal names the remedy", "ralphy run --agent gemini" in text, "")
            check("the console does not print [session closed]", "[session closed]" not in text, "")
            page.screenshot(path=os.path.join(SHOT_DIR, "console-refused.png"))

            # --- scenario 2: nothing was spawned ---------------------------------
            sessions = json.loads(page.request.get(BASE + "api/sessions").text())
            check("no session exists for the refused launch", sessions == [], f"got={sessions}")

            # --- scenario 3: an unregistered repo is refused the same way --------
            page.evaluate("() => window.WBConsole.open({ repo: 'nobody/nothing', agent: 'claude' })")
            text = flat(wait_screen(page, 1, "could not start"))
            check(
                "an unknown repo prints its reason",
                "[could not start: unknown repo]" in text,
                f"screen={text[-200:]!r}",
            )

            ctx.close()
            browser.close()

            # --- scenario 4: no uncaught error over the whole pass ---------------
            check("zero pageerror events captured", page_errors == [], f"got={page_errors}")
    finally:
        stop(proc)

    # The count floor: an early exit must not report success on a few checks.
    ok = all(results) and len(results) >= 7
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
