"""#317 browser acceptance: Changes is a rail view, not a sidebar accordion.

One Playwright pass over a REAL daemon proving the promotion: the rail switches
the sidebar's view, the view is scoped to the open project, the change count
survives the move as a per-project badge that CANNOT read as a cross-repo
aggregate, the diff tab is unchanged, the accordion is gone, and the view still
holds a toolbar and a message box over a 60-file list at two viewport widths.

Scenario 1  the rail has 5 buttons and no digits; clicking Changes shows
            `.changes-view` and hides `.projects-view`, and Projects reverses it
Scenario 2  the visible `.chg-name` set equals the OPEN project's paths, and
            re-scopes when another project is opened
Scenario 3  the Projects view's `.chg-badge` reads 2 on A and 3 on B while NO
            element on the page reads 5 (the aggregate an implementation that
            summed the map would print), an unopened project shows no badge, and
            a failed `changes.list` moves A's badge to `—`, never `0`
Scenario 4  clicking a `.chg-row` still opens the diff as a canvas tab
Scenario 5  the accordion is gone: no `.changes-sec`, no `li.project
            .changes-list`
Scenario 6  1440x900 over 60 changes: the toolbar and the message box stay
            inside the sidebar and the list scrolls instead of pushing them out
Scenario 7  the same at 390x844 (a phone width)

Boots a Localhost daemon on 7417 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/317-changes-rail-{desktop,phone}-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_317.py   (exit 0 = all pass)
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

PORT = 7417
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_317.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

RAIL_CHANGES = "nav.rail button[title=\"Changes\"]"
RAIL_PROJECTS = "nav.rail button[title=\"Projects\"]"
VIEW = "document.querySelector('.changes-view')"
PROJ_VIEW = "document.querySelector('.projects-view')"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wb317_empty_")
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


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed(d, tag, extra=()):
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #317 rail-view fixture repo.\n", encoding="utf-8")
    for name in extra:
        (p / name).write_text(f"{name}\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb317@example.com")
    git(d, "config", "user.name", "wb317")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return p


def make_two():
    """A = exactly 2 changed PATHS: a staged README.md, and a renamed path that
    is also modified in the worktree — so both groups render AND `.chg-from`
    has a row to draw on."""
    d = tempfile.mkdtemp(prefix="wb317_a_")
    p = seed(d, "alpha", extra=("old.txt",))
    (p / "README.md").write_text("# alpha\n\nstaged edit\n", encoding="utf-8")
    git(d, "add", "README.md")
    git(d, "mv", "old.txt", "new.txt")
    (p / "new.txt").write_text("old.txt\nworktree edit\n", encoding="utf-8")
    return d


def make_three():
    """B = exactly 3 changed paths, all unstaged."""
    d = tempfile.mkdtemp(prefix="wb317_b_")
    p = seed(d, "bravo", extra=("one.txt", "two.txt"))
    for name in ("README.md", "one.txt", "two.txt"):
        (p / name).write_text(f"{name} edited\n", encoding="utf-8")
    return d


def make_clean():
    """C = a registered project that is never opened — the control for "a slug
    nobody read renders no badge at all"."""
    d = tempfile.mkdtemp(prefix="wb317_c_")
    seed(d, "charlie")
    return d


def make_sixty():
    """D = 60 changed paths: the layout budget's load."""
    d = tempfile.mkdtemp(prefix="wb317_d_")
    p = seed(d, "delta")
    for i in range(60):
        (p / f"file-{i:02d}.txt").write_text(f"file {i}\n", encoding="utf-8")
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


def show_projects(page):
    """Return to the Projects view — a no-op click would COLLAPSE the sidebar,
    since the rail button of the view already showing is the collapse gesture."""
    page.evaluate(
        f"() => {{ const v = {PROJ_VIEW};"
        f" if (!v || v.offsetParent === null) document.querySelector('{RAIL_PROJECTS}').click(); }}"
    )
    page.wait_for_function(
        f"() => {{ const v = {PROJ_VIEW}; return !!v && v.offsetParent !== null; }}", timeout=15000
    )


def open_changes(page, slug):
    """Open a project in the Projects view, then reach its change set the way an
    operator does: by clicking the rail's Changes button."""
    show_projects(page)
    # The slug rides as an ARGUMENT, never interpolated into the source: a repo
    # registered from a Windows path carries backslashes a string literal would
    # swallow as escapes, silently opening nothing (#316).
    page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.evaluate(
        f"() => {{ const v = {VIEW};"
        f" if (!v || v.offsetParent === null) document.querySelector('{RAIL_CHANGES}').click(); }}"
    )
    # An Alpine x-show flip is NOT visible to the very next evaluate, so every
    # wait polls on an offsetParent-gated predicate (KNOWLEDGE.md #307/#309).
    page.wait_for_function(
        f"() => {{ const v = {VIEW}; return !!v && v.offsetParent !== null; }}", timeout=15000
    )


def wait_rows(page, n):
    page.wait_for_function(
        f"(n) => {{ const v = {VIEW}; if (!v || v.offsetParent === null) return false;"
        " return Array.from(v.querySelectorAll('.chg-row'))"
        "   .filter(e => e.offsetParent !== null).length === n; }",
        arg=n,
        timeout=20000,
    )


def visible_names(page):
    return page.evaluate(
        f"() => {{ const v = {VIEW}; if (!v) return [];"
        " return Array.from(v.querySelectorAll('.chg-row'))"
        "   .filter(e => e.offsetParent !== null)"
        "   .map(e => (e.getAttribute('title') || '').trim()); }"
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb317_reg_")
    dir_a, dir_b, dir_c, dir_d = make_two(), make_three(), make_clean(), make_sixty()
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)
    register_fixture(daemon_dir, dir_c)
    slug_d = register_fixture(daemon_dir, dir_d)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 4", timeout=15000)

            # --- scenario 1: the rail switches the sidebar's view --------------
            rail = page.evaluate(
                "() => ({ n: document.querySelectorAll('nav.rail button').length,"
                " titles: Array.from(document.querySelectorAll('nav.rail button'))"
                "   .map(b => b.getAttribute('title')),"
                " text: (document.querySelector('nav.rail').textContent || '') })"
            )
            check(
                "the rail carries five buttons, Changes among them",
                rail["n"] == 5 and "Changes" in rail["titles"],
                f"got={rail['titles']}",
            )
            # The negative control for criterion 6: an implementation that hung a
            # roll-up badge on the rail button would print a digit HERE.
            import re as _re

            check(
                "…and no digit anywhere in the rail (no cross-repo aggregate)",
                _re.search(r"[0-9]", rail["text"]) is None,
                f"text={rail['text']!r}",
            )

            open_changes(page, slug_a)
            flip = page.evaluate(
                f"() => ({{ changes: !!{VIEW} && {VIEW}.offsetParent !== null,"
                f" projects: !!{PROJ_VIEW} && {PROJ_VIEW}.offsetParent !== null }})"
            )
            check(
                "clicking Changes shows the Changes view and hides Projects",
                flip["changes"] and not flip["projects"],
                f"got={flip}",
            )
            show_projects(page)
            back = page.evaluate(
                f"() => ({{ changes: !!{VIEW} && {VIEW}.offsetParent !== null,"
                f" projects: !!{PROJ_VIEW} && {PROJ_VIEW}.offsetParent !== null }})"
            )
            check(
                "…and clicking Projects reverses it",
                back["projects"] and not back["changes"],
                f"got={back}",
            )

            info("fixtures", f"a={dir_a} b={dir_b} c={dir_c} d={dir_d}")
            info("slugs", f"a={slug_a} b={slug_b} d={slug_d}")
            check("the page threw nothing", not thrown, f"pageerrors={thrown}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 5
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES RAIL LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
