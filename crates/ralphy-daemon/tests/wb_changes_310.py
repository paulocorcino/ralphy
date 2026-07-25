"""#310 browser acceptance: the Changes count invalidates when a run finishes.

One Playwright pass over a REAL daemon proving the count refreshes itself when a
daemon-spawned run child exits — and ONLY then (no poll, no repo-wide watch).

Scenario 1  opening project A shows its Changes count (literal `1`)
Scenario 2  a file written from OUTSIDE the browser leaves the badge at `1` for
            a 3s window — no polling timer, no repo-wide filesystem watch
Scenario 3  a daemon-spawned run in A exits and the badge refreshes `1` -> `2`
            with NO click on `.side-refresh`
Scenario 4  a run finishing in project B (not open) leaves A's badge at `2`
Scenario 5  manual refresh still works (`2` -> `3`)
Scenario 6  the count stays scoped — exactly one visible `.changes-sec`

The trigger is a REAL `ralphy run` in a remote-less fixture repo: it fails fast
(`no git remotes found`, ~1s) and that exit is the nudge. `RALPHY_EXE_OVERRIDE`
cannot be used here — `changes.list` resolves the same exe and would spawn the
fake child, destroying the very count under test.

Boots a Localhost daemon on 7410 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/310-changes-nudge-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_310.py   (exit 0 = all pass)
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

PORT = 7410
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_310.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
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
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb310_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def make_fixture_repo(tag):
    """A committed git repo with NO remote (so a dispatched `ralphy run` exits in
    about a second), then one modification on top — a starting count of 1.
    `.gitignore` hides `.ralphy/`, so a run's own scratch never moves the count."""
    d = tempfile.mkdtemp(prefix=f"wb310_{tag}_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #310 nudge fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb310@example.com"],
        ["git", "config", "user.name", "wb310"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    (p / "README.md").write_text("# edited\n", encoding="utf-8")  # the 1st change
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('.changes-sec')).filter(e => e.offsetParent !== null)"
)


def badge_text(page):
    """The count of the OPEN project's Changes section. Every other project's
    section is `x-show`-hidden; returning `MULTI` when more than one is visible
    keeps a scoping regression from reading as a passing count."""
    return page.evaluate(
        f"() => {{ const els = {VISIBLE_SECS};"
        " if (els.length > 1) return 'MULTI';"
        " return els.length ? els[0].querySelector('.count').textContent.trim() : null; }"
    )


def wait_badge(page, expected, timeout=15000):
    # Never a bare `evaluate` after a state change: an Alpine `x-show` flip is not
    # visible to the very next read (KNOWLEDGE.md / #307).
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].querySelector('.count').textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


def fire_run(page, slug):
    """Dispatch the same `workbench:action` the run button emits — a real Spawn
    over `/ws/command`, whose child exit is the nudge under test."""
    page.evaluate(
        "(slug) => document.dispatchEvent(new CustomEvent('workbench:action',"
        " { detail: { action: 'run-start', project: slug, agent: 'claude', branchMode: 'new' } }))",
        slug,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb310_reg_")
    dir_a = make_fixture_repo("a")
    dir_b = make_fixture_repo("b")
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)

            # --- scenario 1: the open project's count ------------------------
            page.evaluate(f"() => {SH}.toggle('{slug_a}')")
            wait_badge(page, "1")
            check("the open project's Changes count reads 1", badge_text(page) == "1")

            # --- scenario 2: no poll, no repo-wide watch ---------------------
            # Settle first: `toggle()` fires its own async `changes.list`, and an
            # in-flight read picking up the write would counterfeit this proof.
            page.wait_for_timeout(1000)
            Path(dir_a, "nudge.txt").write_text("written outside the browser\n", encoding="utf-8")
            page.wait_for_timeout(3000)
            got = badge_text(page)
            check(
                "a write with no run leaves the count at 1 for 3s (no poll, no watch)",
                got == "1",
                f"got={got!r}",
            )

            # --- scenario 3: a finished run refreshes the count --------------
            fire_run(page, slug_a)
            wait_badge(page, "2", timeout=60000)
            check(
                "a finished run refreshes the count to 2 with no manual refresh",
                badge_text(page) == "2",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "310-changes-nudge-2026-07-25.png"))

            # --- scenario 4: a nudge for a repo that is not open --------------
            Path(dir_b, "b-only.txt").write_text("only in B\n", encoding="utf-8")
            fire_run(page, slug_b)
            page.wait_for_timeout(5000)
            got = badge_text(page)
            check(
                "a run finishing in a project that is not open leaves the count at 2",
                got == "2",
                f"got={got!r}",
            )

            # --- scenario 5: manual refresh still works ----------------------
            Path(dir_a, "late.txt").write_text("late arrival\n", encoding="utf-8")
            page.click(".side-refresh")
            wait_badge(page, "3")
            check("the sidebar refresh still reloads the count to 3", badge_text(page) == "3")

            # --- scenario 6: the count stays scoped --------------------------
            secs = page.evaluate(f"() => {VISIBLE_SECS}.length")
            check("exactly one Changes section is visible (scoping intact)", secs == 1, f"got={secs}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 7
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES NUDGE LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
