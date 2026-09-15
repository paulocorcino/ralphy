"""#403 browser acceptance: the Worktrees section of the branch picker.

One Playwright pass over a REAL daemon proving the read path end to end
(ADR-0063 §4): a workbench worktree under `.ralphy/worktrees/<name>` shows up
as a row in the project's branch picker, `primary` first, with its branch and a
dirty dot — and a project with no worktrees renders the picker exactly as it
did before the section existed.

Scenario 1  the daemon is listening
Scenario 2  on fixture A (one workbench worktree `wt-a`, dirtied, plus a
            hand-made worktree ELSEWHERE) the picker renders exactly two
            `.worktree-item` rows: `primary` then `wt-a · wt-a` with a displayed
            dirty dot; no row is named `elsewhere`
Scenario 3  clicking a row does nothing in this slice: the picker stays open and
            the project's branch is unchanged
Scenario 4  on fixture B (no worktrees) the picker has ZERO `.worktree-sec` /
            `.worktree-item` nodes — the section is `x-if`, so nothing is
            rendered, not merely hidden

Boots a Localhost daemon on 7451 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/403-worktree-picker-2026-09-15.png.
Run: python crates/ralphy-daemon/tests/wb_worktree_403.py   (exit 0 = all pass)
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

PORT = 7451
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_403.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb403_empty_")
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


def seed(parent_prefix, name):
    """A committed git repo on `main` at a CHOSEN directory name, `.ralphy/`
    gitignored so a worktree under it never dirties the primary tree."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #403 picker fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb403@example.com")
    git(d, "config", "user.name", "wb403")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def add_worktrees(root):
    """Fixture A's worktrees: `wt-a` under the fixed location (then dirtied
    with an untracked file) and `elsewhere` in a sibling temp dir — the one the
    filter must drop."""
    (root / ".ralphy" / "worktrees").mkdir(parents=True)
    git(root, "worktree", "add", "--no-track", "-b", "wt-a", ".ralphy/worktrees/wt-a", "HEAD")
    elsewhere = Path(tempfile.mkdtemp(prefix="wb403_elsewhere_")) / "elsewhere"
    git(root, "worktree", "add", "--no-track", "-b", "elsewhere", str(elsewhere), "HEAD")
    (root / ".ralphy" / "worktrees" / "wt-a" / "scratch.txt").write_text("dirty\n", encoding="utf-8")


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's picker.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The rendered Worktrees rows, laid-out ones only. The dot is read through its
# COMPUTED display: `x-show` flips `display`, and a class read would pass on a
# hidden dot.
ROWS_EXPR = (
    "() => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
    "  .filter(e => e.offsetParent !== null)"
    "  .map(e => ({ name: e.querySelector('.worktree-name').textContent.trim(),"
    "    branch: e.querySelector('.worktree-branch').textContent.trim(),"
    "    dot: getComputedStyle(e.querySelector('.worktree-dirty')).display !== 'none',"
    "    laid: e.clientWidth > 0 }))"
)


def open_picker(page, slug):
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(
        "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )
    page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # The listing arrives by round trip: gate on the reply having landed, not
    # on the modal being open (an open modal samples an empty section).
    page.wait_for_function(f"() => {SH}.branchModal.checkouts !== null", timeout=15000)


def close_picker(page, slug):
    page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
    page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
    page.wait_for_function(f"() => {SH}.openSlug === null", timeout=10000)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb403_reg_")
    dir_a = seed("wb403_a_", "with-worktrees")
    add_worktrees(dir_a)
    dir_b = seed("wb403_b_", "plain")
    slug_a = register_fixture(daemon_dir, str(dir_a))
    slug_b = register_fixture(daemon_dir, str(dir_b))

    proc = launch(daemon_dir)
    try:
        # --- scenario 1 ---------------------------------------------------
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
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('li.project'))"
                "  .filter(e => e.offsetParent !== null).length === 2",
                timeout=15000,
            )

            # --- scenario 2: the rows on the fixture with worktrees -----------
            open_picker(page, slug_a)
            page.wait_for_function(
                "() => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
                "  .filter(e => e.offsetParent !== null && e.clientWidth > 0).length === 2",
                timeout=15000,
            )
            rows = page.evaluate(ROWS_EXPR)
            check("the picker renders exactly two worktree rows", len(rows) == 2, "got={}".format(rows))
            check(
                "the first row is the primary tree on the project's branch",
                len(rows) >= 1 and rows[0]["laid"] and rows[0]["name"] == "primary" and rows[0]["branch"] == "main",
                "got={}".format(rows[:1]),
            )
            check(
                "the second row is wt-a · wt-a with a displayed dirty dot",
                len(rows) >= 2
                and rows[1]["laid"]
                and rows[1]["name"] == "wt-a"
                and rows[1]["branch"] == "wt-a"
                and rows[1]["dot"] is True,
                "got={}".format(rows[1:2]),
            )
            check(
                "the primary row shows no dirty dot (its tree is clean)",
                len(rows) >= 1 and rows[0]["dot"] is False,
                "got={}".format(rows[:1]),
            )
            check(
                "the hand-made worktree elsewhere is not a row",
                not any(r["name"] == "elsewhere" for r in rows),
                "got={}".format([r["name"] for r in rows]),
            )
            head = page.evaluate(
                "() => { const h = document.querySelector('.branch-modal .worktree-head');"
                "  return h && h.offsetParent !== null ? h.textContent.trim() : null; }"
            )
            check("the section carries its Worktrees heading", head == "Worktrees", "got={}".format(head))

            # --- scenario 3: a row click is inert in this slice ---------------
            branch_before = page.evaluate(
                f"(s) => ({SH}.projects.find(p => {SH}.repoRef(p) === s) || {{}}).branch", arg=slug_a
            )
            page.evaluate("() => document.querySelectorAll('.branch-modal .worktree-item')[1].click()")
            page.wait_for_timeout(300)
            still_open = page.evaluate(f"() => {SH}.branchOpen")
            branch_after = page.evaluate(
                f"(s) => ({SH}.projects.find(p => {SH}.repoRef(p) === s) || {{}}).branch", arg=slug_a
            )
            check(
                "clicking a worktree row leaves the picker open and the branch unchanged",
                still_open is True and branch_before == "main" and branch_after == branch_before,
                "open={} before={} after={}".format(still_open, branch_before, branch_after),
            )

            shot = os.path.join(SHOT_DIR, "403-worktree-picker-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)
            close_picker(page, slug_a)

            # --- scenario 4: the plain fixture renders no section at all ------
            open_picker(page, slug_b)
            page.wait_for_timeout(300)
            plain = page.evaluate(
                "() => ({ nodes: document.querySelectorAll('.branch-modal .worktree-sec, .branch-modal .worktree-item').length,"
                f"  listing: {SH}.branchModal.checkouts,"
                "  branches: Array.from(document.querySelectorAll('.branch-modal .branch-item'))"
                "    .filter(e => e.offsetParent !== null).length })"
            )
            check(
                "with no worktrees the picker has zero worktree nodes",
                plain["nodes"] == 0,
                "got={}".format(plain),
            )
            check(
                "…even though the listing DID arrive (an empty one), so absence is the fold's, not a timeout's",
                isinstance(plain["listing"], dict) and plain["listing"].get("worktrees") == [],
                "got={}".format(plain["listing"]),
            )
            check(
                "the branch list itself still renders on the plain fixture",
                plain["branches"] >= 1,
                "got={}".format(plain["branches"]),
            )
            close_picker(page, slug_b)

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 12
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
