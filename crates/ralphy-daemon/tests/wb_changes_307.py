"""#307 browser acceptance: the Changes section's count in the sidebar.

One Playwright pass over a REAL daemon proving the count is the repo's own
working-tree change set, visible without a click, scoped to the open project.

Scenario 1  opening a project shows its Changes count (literal `3`), no click
Scenario 2  a clean repo reads `0` and carries the quiet `zero` class
Scenario 3  the section sits BELOW the file tree inside the same open project
Scenario 4  no new rail icon and no tab switcher was introduced
Scenario 5  switching projects re-scopes the count (`3` -> `1`)
Scenario 6  a 4th file written from OUTSIDE the browser + `.side-refresh`
            reloads the count (`3` -> `4`)

Boots a Localhost daemon on 7407 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/307-changes-count-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_307.py   (exit 0 = all pass)
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

PORT = 7407
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_307.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb307_empty_")
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


def make_fixture_repo(tag, dirt):
    """A committed git repo, then `dirt` applied on top. `.gitignore` hides
    `.ralphy/`, so the fixture's change count is exactly what `dirt` made."""
    d = tempfile.mkdtemp(prefix=f"wb307_{tag}_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #307 changes fixture repo.\n", encoding="utf-8")
    (p / "tracked.txt").write_text("committed\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb307@example.com"],
        ["git", "config", "user.name", "wb307"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    dirt(p, d)
    return d


def three_changes(p, d):
    (p / "README.md").write_text("# edited\n", encoding="utf-8")  # modified
    (p / "added.txt").write_text("staged\n", encoding="utf-8")  # added
    subprocess.run(["git", "add", "added.txt"], cwd=d, check=True, capture_output=True)
    (p / "untracked.txt").write_text("loose\n", encoding="utf-8")  # untracked


def one_change(p, d):
    (p / "README.md").write_text("# edited\n", encoding="utf-8")


def no_change(p, d):
    pass


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
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].querySelector('.count').textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


def open_project(page, slug, expected):
    page.evaluate(f"() => {SH}.toggle('{slug}')")
    wait_badge(page, expected)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb307_reg_")
    dir_a = make_fixture_repo("three", three_changes)
    dir_b = make_fixture_repo("one", one_change)
    dir_c = make_fixture_repo("clean", no_change)
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)
    slug_c = register_fixture(daemon_dir, dir_c)

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
            page.wait_for_function(f"() => {SH}.projects.length === 3", timeout=15000)

            # --- scenario 1: the count is there without a click ---------------
            check("no Changes section before a project is open", badge_text(page) is None)
            open_project(page, slug_a, "3")
            check("the open project's Changes count reads 3", badge_text(page) == "3")
            visible = page.evaluate(
                "() => { const el = Array.from(document.querySelectorAll('.changes-sec'))"
                ".find(e => e.offsetParent !== null);"
                " return !!el && el.querySelector('.count').offsetParent !== null; }"
            )
            check("the badge is visible with no click on the section", visible)
            clicked = page.evaluate(
                "() => { const el = Array.from(document.querySelectorAll('.changes-sec'))"
                ".find(e => e.offsetParent !== null); return el.querySelectorAll('button, [x-on\\\\:click], .chev').length; }"
            )
            check("the collapsed section offers no toggle control", clicked == 0, f"controls={clicked}")

            # --- scenario 3: it sits below the file tree ----------------------
            geom = page.evaluate(
                "() => { const li = Array.from(document.querySelectorAll('li.project'))"
                ".find(e => e.querySelector('.changes-sec') && e.querySelector('.changes-sec').offsetParent !== null);"
                " const host = li.querySelector('.wb-host'); const sec = li.querySelector('.changes-sec');"
                " return { host: host.getBoundingClientRect().top, sec: sec.getBoundingClientRect().top,"
                "          sameLi: li.contains(host) && li.contains(sec) }; }"
            )
            check(
                "the Changes section sits below the tree in the same open project",
                geom["sameLi"] and geom["sec"] > geom["host"],
                f"host={geom['host']} sec={geom['sec']}",
            )

            # --- scenario 4: no new rail icon, no tab switcher ----------------
            rail = page.evaluate("() => document.querySelectorAll('nav.rail button').length")
            check("the rail still holds exactly 4 buttons", rail == 4, f"got={rail}")
            tabs = page.evaluate("() => document.querySelectorAll('aside.side .tab').length")
            check("the sidebar introduces no tab switcher", tabs == 0, f"got={tabs}")

            page.screenshot(path=os.path.join(SHOT_DIR, "307-changes-count-2026-07-25.png"))

            # --- scenario 5: the count is scoped to the open project ----------
            open_project(page, slug_b, "1")
            check("switching projects re-scopes the count to 1", badge_text(page) == "1")

            # --- scenario 2: a clean tree reads zero, quietly ------------------
            open_project(page, slug_c, "0")
            check("a clean repo's count reads 0", badge_text(page) == "0")
            quiet = page.evaluate(
                "() => { const el = Array.from(document.querySelectorAll('.changes-sec'))"
                ".find(e => e.offsetParent !== null); return el.querySelector('.count').classList.contains('zero'); }"
            )
            check("the zero badge carries the quiet `zero` class", quiet)
            page.evaluate(f"() => {SH}.toggle('{slug_c}')")
            page.wait_for_function(f"() => {VISIBLE_SECS}.length === 0", timeout=8000)
            check("closing the clean project hides its section", badge_text(page) is None)

            # --- scenario 6: manual refresh reloads the count -----------------
            open_project(page, slug_a, "3")
            # Let the open's own load land BEFORE dirtying the repo, else that
            # in-flight read — not a watcher — could be what updates the badge.
            page.wait_for_timeout(1000)
            # written from OUTSIDE the browser: nothing in the page knows yet.
            Path(dir_a, "fourth.txt").write_text("late arrival\n", encoding="utf-8")
            page.wait_for_timeout(1500)
            got = badge_text(page)
            check("the count is a snapshot — still 3 before a refresh", got == "3", f"got={got!r}")
            page.click(".side-refresh")
            wait_badge(page, "4")
            check("the sidebar refresh reloads the count to 4", badge_text(page) == "4")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 13
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES COUNT LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
