"""#403 browser acceptance: the Worktrees section of the branch picker.

One Playwright pass over a REAL daemon proving the read path end to end
(ADR-0063 §4): a workbench worktree under `.ralphy/worktrees/<name>` shows up
as a row in the Files bar's checkout chip menu, `primary` first, with its
branch and a dirty dot — and a project with no worktrees shows no chip at all
(ADR-0063 amendment 2026-09-16 b; creation is the console's, wb_worktree_405).

Scenario 1  the daemon is listening
Scenario 2  on fixture A (one workbench worktree `wt-a`, dirtied, plus a
            hand-made worktree ELSEWHERE) the chip's menu renders exactly two
            rows: `primary` then `wt-a · wt-a` with a dirty dot; no row is
            named `elsewhere`; the chip reads `primary`
Scenario 3  clicking a worktree row selects that checkout (#406): the menu
            closes, the chip reads `wt-a`, and the project's branch is
            unchanged (a selection is not a switch)
Scenario 4  on fixture B (no worktrees) there is NO chip; the branch picker
            opens and lists branches only

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
        "() => { const c = document.querySelector('li.project.open .project-head .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )
    page.evaluate("() => document.querySelector('li.project.open .project-head .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # `branch.list` arrives by round trip: gate on the real list having landed.
    page.wait_for_function(f"() => {SH}.branchModal.branches.length >= 1", timeout=15000)


def close_picker(page, slug):
    page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
    page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
    page.wait_for_function(f"() => {SH}.openSlug === null", timeout=10000)


# --- the Files bar's checkout chip and its menu (ADR-0063 amendment 2026-09-16 b) ---
CHIP = "li.project.open .files-sec .checkout-chip"
CHIP_VISIBLE = "() => { const c = document.querySelector('li.project.open .files-sec .checkout-chip'); return !!c && c.offsetParent !== null && c.clientWidth > 0; }"
CHIP_MENU_OPEN = "() => !!document.querySelector('.session-checkout-menu')"
CHIP_ITEM = (
    "(n) => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === n) || null"
)
CHIP_ROWS = (
    "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .map(e => ({ name: e.querySelector('.session-checkout-name').textContent.trim(),"
    "    branch: (e.querySelector('.session-checkout-branch') || {}).textContent || '',"
    "    dot: !!e.querySelector('.session-checkout-dirty'),"
    "    state: (() => { const s = e.querySelector('.session-checkout-state'); return s ? [...s.classList].find(c => c !== 'session-checkout-state') || null : null; })(),"
    "    current: e.classList.contains('current'), laid: e.clientWidth > 0 }))"
)


def open_project(page, slug):
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)


def open_chip(page, slug):
    """Open the project and its checkout chip's menu; the chip exists once the
    listing answered with a worktree, so the wait is the listing's."""
    open_project(page, slug)
    page.wait_for_function(CHIP_VISIBLE, timeout=15000)
    if not page.evaluate(CHIP_MENU_OPEN):
        page.evaluate(f"() => document.querySelector('{CHIP}').click()")
        page.wait_for_function(CHIP_MENU_OPEN, timeout=5000)


def close_chip(page):
    if page.evaluate(CHIP_MENU_OPEN):
        page.keyboard.press("Escape")
        page.wait_for_function(f"() => !({CHIP_MENU_OPEN})()", timeout=5000)


def chip_rows(page, slug):
    open_chip(page, slug)
    rows = page.evaluate(CHIP_ROWS)
    close_chip(page)
    return rows


def click_row(page, name, slug=None):
    """Pick a checkout from the chip's menu (the menu closes on the pick)."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CHIP_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CHIP_ITEM})(n).click()", arg=name)
    page.wait_for_function(f"() => !({CHIP_MENU_OPEN})()", timeout=5000)


def click_remove(page, name, slug=None):
    """The trash on a row of the chip's menu; waits for the re-read to land."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CHIP_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CHIP_ITEM})(n).querySelector('.session-checkout-remove').click()", arg=name)
    page.wait_for_function(f"() => !({CHIP_MENU_OPEN})()", timeout=5000)
    page.wait_for_function(f"() => Object.keys({SH}.worktreeRemoving).length === 0", timeout=20000)


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
            open_chip(page, slug_a)
            rows = page.evaluate(CHIP_ROWS)
            check("the chip's menu renders exactly two rows", len(rows) == 2, "got={}".format(rows))
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
            chip = page.evaluate(f"() => document.querySelector('{CHIP} .checkout-chip-name').textContent.trim()")
            check("the chip reads primary", chip == "primary", "got={}".format(chip))

            shot = os.path.join(SHOT_DIR, "403-worktree-picker-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # --- scenario 3: a row click SELECTS the checkout (#406) and never
            # touches the branch — the picker closes on the pick; the project's
            # branch is the primary's, and a selection is not a switch. (Until
            # #406 this pinned the click as inert; the select path has its own
            # suite, wb_worktree_406.py.)
            branch_before = page.evaluate(
                f"(s) => ({SH}.projects.find(p => {SH}.repoRef(p) === s) || {{}}).branch", arg=slug_a
            )
            page.evaluate(f"(n) => ({CHIP_ITEM})(n).click()", arg="wt-a")
            page.wait_for_function(f"() => !({CHIP_MENU_OPEN})()", timeout=5000)
            selected = page.evaluate(f"(s) => {SH}.checkoutOf(s)", arg=slug_a)
            page.wait_for_function(f"() => document.querySelector('{CHIP} .checkout-chip-name').textContent.trim() === 'wt-a'", timeout=5000)
            branch_after = page.evaluate(
                f"(s) => ({SH}.projects.find(p => {SH}.repoRef(p) === s) || {{}}).branch", arg=slug_a
            )
            check(
                "clicking a worktree row selects it (the chip reads wt-a), closes the menu and leaves the branch unchanged",
                selected == "wt-a" and branch_before == "main" and branch_after == branch_before,
                "selected={} before={} after={}".format(selected, branch_before, branch_after),
            )
            # Back to the primary so the rest of the suite reads the tree it seeded.
            page.evaluate(f"(s) => {SH}.setCheckout(s, null)", arg=slug_a)
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug_a, timeout=10000)
            close_picker(page, slug_a)

            # --- scenario 4: the plain fixture renders no checkouts section at
            # all (2026-09-16). `open_picker` already gated on the (empty)
            # listing having landed.
            open_project(page, slug_b)
            page.wait_for_function(f"(s) => s in {SH}.worktreeListings", arg=slug_b, timeout=15000)
            open_picker(page, slug_b)
            plain = page.evaluate(
                f"(s) => ({{ items: document.querySelectorAll('.branch-modal .worktree-item').length,"
                "  create: document.querySelectorAll('.branch-modal .worktree-sec').length,"
                f"  chip: ({CHIP_VISIBLE})(),"
                f"  listing: {SH}.worktreeListings[s],"
                "  branches: Array.from(document.querySelectorAll('.branch-modal .branch-item'))"
                "    .filter(e => e.offsetParent !== null).length })",
                arg=slug_b,
            )
            check(
                "with no worktrees there is no chip and the picker has no worktree rows",
                plain["items"] == 0 and plain["create"] == 0 and plain["chip"] is False,
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
