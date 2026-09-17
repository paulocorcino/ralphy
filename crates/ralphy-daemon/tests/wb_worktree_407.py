"""#407 browser acceptance: Changes, the diff and the branch chip act on the
selected checkout.

One Playwright pass over a REAL daemon proving the cwd hand-off end to end
(ADR-0063 §2): with a worktree selected in the picker, the Changes panel lists
THAT tree's status, stage/commit land on its index and branch, the diff reads
its HEAD blob, and a branch act moves its HEAD — the primary tree is untouched
throughout, asserted from Python with `git` in both directories.

Fixture: a repo on `main` with `README.md` and `only-in-primary.txt`, a `side`
branch at the same commit, and one worktree `wt-a` made by `ralphy worktree
add` whose branch `wt-a` commits `only-in-wt.txt` (= `worktree\\n`) and removes
`only-in-primary.txt`.

Scenario 1  the daemon is listening
Scenario 2  `wt-a` selected; `dirty.txt` written and `README.md` edited in
            the WORKTREE → the Changes rows list both; the primary's
            `git status --porcelain` is empty
Scenario 3  `stagePaths(['README.md'])` → the worktree's index has it, the
            primary's index is empty
Scenario 4  `commitStaged` with `wt commit` → the worktree's `log -1` is
            `wt commit`, the primary's is `fixture`, branch `wt-a` carries it
Scenario 5  `only-in-wt.txt` edited to `changed\\n`; clicking its row opens
            `diff:<slug>@wt-a:only-in-wt.txt` whose Monaco original is the
            worktree's HEAD blob `worktree\\n` (absent at the primary's HEAD)
Scenario 6  `switchBranch('side')` → the worktree's HEAD is `side`, the
            primary's stays `main`, the chip reads `side`
Scenario 7  `switchBranch('main')` is sent and git refuses it (checked out
            in the primary); both HEADs unchanged
Scenario 8  `createBranch()` with `from-wt` → the worktree's HEAD is
            `from-wt`, the primary's stays `main`, the chip reads
            `from-wt`
Scenario 9  selecting `primary` shows the primary's Changes (no `dirty.txt`)
            and the chip reads `main`
Scenario 10 no page errors

Boots a Localhost daemon on 7454 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/407-worktree-changes-2026-09-15.png.
Run: python crates/ralphy-daemon/tests/wb_worktree_407.py   (exit 0 = all pass)
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

PORT = 7454
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_407.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb407_empty_")
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
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, encoding="utf-8").stdout.strip()


def head_branch(cwd):
    return git(cwd, "rev-parse", "--abbrev-ref", "HEAD")


def wait_git(cwd, want, timeout=10):
    """Poll a git read until it answers `want`; the value is what is asserted."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        if head_branch(cwd) == want:
            return True
        time.sleep(0.2)
    return head_branch(cwd) == want


def seed(parent_prefix, name):
    """A committed git repo on `main` at a CHOSEN directory name, `.ralphy/`
    gitignored so a worktree under it never dirties the primary tree, with a
    file only the primary holds and a `side` branch at the same commit."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #407 changes fixture repo.\n", encoding="utf-8")
    (d / "only-in-primary.txt").write_text("primary\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb407@example.com")
    git(d, "config", "user.name", "wb407")
    git(d, "config", "core.autocrlf", "false")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    git(d, "branch", "side")
    return d


def add_worktree(fixture):
    """`ralphy worktree add wt-a` (the CLI, not git directly), then make the two
    trees differ by one file each on the worktree's own branch."""
    subprocess.run(
        [EXE, "worktree", "add", "wt-a", "--repo", str(fixture)],
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    wt = fixture / ".ralphy" / "worktrees" / "wt-a"
    (wt / "only-in-primary.txt").unlink()
    # Bytes, not text: `write_text` translates `\n` to CRLF on Windows and the
    # Monaco oracle below is byte-exact.
    (wt / "only-in-wt.txt").write_bytes(b"worktree\n")
    git(wt, "config", "user.email", "wb407@example.com")
    git(wt, "config", "user.name", "wb407")
    git(wt, "add", "-A")
    git(wt, "commit", "-m", "wt")
    return wt


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's panel.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


CHIP_IS = (
    "(t) => ((document.querySelector('li.project.open .files-sec .branch-chip-name') || {}).textContent || '')"
    "  .trim() === t"
)
# A laid-out picker row by its `.worktree-name`.
ROW_BY_NAME = (
    "(n) => [...document.querySelectorAll('.branch-modal .worktree-item')]"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0)"
    "  .find(e => e.querySelector('.worktree-name')?.textContent.trim() === n) || null"
)
# Every LAID-OUT Changes row (wb_diff_311.py's probe): rows exist in the DOM
# before Alpine's `x-show` flip, so a raw count resolves early.
VISIBLE_ROWS = "Array.from(document.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null)"
ROW_NAMES = f"() => {VISIBLE_ROWS}.map(r => r.querySelector('.chg-name').textContent.trim())"
HAS_UNSTAGED = "(a) => ({SH}.changesUnstaged[a[0]] || []).some(e => e.path === a[1])".replace("{SH}", SH)
HAS_STAGED = "(a) => ({SH}.changesStaged[a[0]] || []).some(e => e.path === a[1])".replace("{SH}", SH)


def wait_shell(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)
    page.wait_for_function(
        "() => Array.from(document.querySelectorAll('li.project'))"
        "  .filter(e => e.offsetParent !== null).length === 1",
        timeout=15000,
    )


def show_view(page, view):
    """Clicking the rail button of the view already showing COLLAPSES the
    sidebar, so every switch is guarded on that view's own visibility (#317)."""
    page.evaluate(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " if (!el || el.offsetParent === null)"
        "   document.querySelector(`nav.rail button[title=\"${v === 'changes' ? 'Changes' : 'Projects'}\"]`)"
        "     .click(); }",
        arg=view,
    )
    page.wait_for_function(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " return !!el && el.offsetParent !== null; }",
        arg=view,
        timeout=15000,
    )


def open_project(page, slug):
    show_view(page, "projects")
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(
        "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )


def open_picker(page, slug):
    open_project(page, slug)
    page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # `branch.list` arrives by round trip: gate on the real list having landed.
    page.wait_for_function(f"() => {SH}.branchModal.branches.length >= 1", timeout=15000)


# --- the Files bar's checkout chip and its menu (ADR-0063 amendment 2026-09-16 b) ---
CK_CHIP = "li.project.open .files-sec .checkout-chip"
CK_VISIBLE = "() => { const c = document.querySelector('li.project.open .files-sec .checkout-chip'); return !!c && c.offsetParent !== null && c.clientWidth > 0; }"
CK_MENU_OPEN = "() => !!document.querySelector('.session-checkout-menu')"
CK_ITEM = (
    "(n) => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === n) || null"
)
CK_ROWS = (
    "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .map(e => ({ name: e.querySelector('.session-checkout-name').textContent.trim(),"
    "    branch: (e.querySelector('.session-checkout-branch') || {}).textContent || '',"
    "    dot: !!e.querySelector('.session-checkout-dirty'),"
    "    state: (() => { const s = e.querySelector('.session-checkout-state'); return s ? [...s.classList].find(c => c !== 'session-checkout-state') || null : null; })(),"
    "    current: e.classList.contains('current') }))"
)


def open_chip(page, slug):
    """Open the project and its checkout chip's menu; the chip exists once the
    listing answered with a worktree, so the wait is the listing's."""
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(CK_VISIBLE, timeout=15000)
    if not page.evaluate(CK_MENU_OPEN):
        page.evaluate(f"() => document.querySelector('{CK_CHIP}').click()")
        page.wait_for_function(CK_MENU_OPEN, timeout=5000)


def close_chip(page):
    if page.evaluate(CK_MENU_OPEN):
        page.keyboard.press("Escape")
        page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)


def chip_rows(page, slug):
    open_chip(page, slug)
    rows = page.evaluate(CK_ROWS)
    close_chip(page)
    return rows


def click_row(page, name, slug=None):
    """Pick a checkout from the chip's menu (the menu closes on the pick)."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CK_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CK_ITEM})(n).click()", arg=name)
    page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)


def click_remove(page, name, slug=None):
    """The trash on a row of the chip's menu; waits for the re-read to land."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CK_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CK_ITEM})(n).querySelector('.session-checkout-remove').click()", arg=name)
    page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)
    page.wait_for_function(f"() => Object.keys({SH}.worktreeRemoving).length === 0", timeout=20000)




def click_change_row(page, path):
    show_view(page, "changes")
    page.wait_for_function(
        f"(want) => !!{VISIBLE_ROWS}.find(r => r.querySelector('.chg-name').textContent.trim() === want)",
        arg=path,
        timeout=15000,
    )
    page.evaluate(
        f"(want) => {VISIBLE_ROWS}.find(r => r.querySelector('.chg-name').textContent.trim() === want).click()",
        arg=path,
    )


def wait_diff_mounted(page, tab_id, timeout=25000):
    page.wait_for_function(
        "(id) => { const el = document.querySelector(`.diff-viewer[data-tab-id=\"${id}\"]`);"
        " return !!el && !!el.querySelector('.monaco-diff-editor .view-lines'); }",
        arg=tab_id,
        timeout=timeout,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb407_reg_")
    fixture = seed("wb407_", "plain")
    wt = add_worktree(fixture)
    slug = register_fixture(daemon_dir, str(fixture))

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
            wait_shell(page)

            # --- scenario 2: the Changes rows are the worktree's ---------------
            click_row(page, "wt-a", slug)
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=10000)
            (wt / "dirty.txt").write_text("x\n", encoding="utf-8")
            (wt / "README.md").write_text("# edited in the worktree\n", encoding="utf-8")
            page.evaluate(f"(s) => {SH}.loadChanges(s)", arg=slug)
            page.wait_for_function(HAS_UNSTAGED, arg=[slug, "dirty.txt"], timeout=15000)
            show_view(page, "changes")
            page.wait_for_function(
                f"() => {VISIBLE_ROWS}.some(r => r.querySelector('.chg-name').textContent.trim() === 'dirty.txt')",
                timeout=15000,
            )
            names = page.evaluate(ROW_NAMES)
            check(
                "with wt-a selected the Changes rows list the worktree's dirty.txt and README.md",
                "dirty.txt" in names and "README.md" in names,
                f"rows={names}",
            )
            porcelain = git(fixture, "status", "--porcelain")
            check("the primary's working tree is clean", porcelain == "", f"got={porcelain!r}")

            # --- scenario 3: stage lands on the worktree's index --------------
            page.evaluate(f"(s) => {SH}.stagePaths(s, ['README.md'])", arg=slug)
            page.wait_for_function(HAS_STAGED, arg=[slug, "README.md"], timeout=15000)
            check(
                "stagePaths(README.md) stages in the worktree's index",
                git(wt, "diff", "--cached", "--name-only") == "README.md",
                f"got={git(wt, 'diff', '--cached', '--name-only')!r}",
            )
            check(
                "the primary's index is untouched",
                git(fixture, "diff", "--cached", "--name-only") == "",
            )

            # --- scenario 4: commit lands on the worktree's branch ------------
            page.evaluate(
                f"(s) => {{ {SH}.commitMsgSlug = s; {SH}.commitMsg = 'wt commit'; return {SH}.commitStaged(s); }}",
                arg=slug,
            )
            page.wait_for_function(
                f"(s) => ({SH}.changesStaged[s] || []).length === 0 && {SH}.commitMsg === ''",
                arg=slug,
                timeout=15000,
            )
            check("commitStaged commits on the worktree", git(wt, "log", "-1", "--format=%s") == "wt commit")
            check(
                "the primary's HEAD commit is still `fixture`",
                git(fixture, "log", "-1", "--format=%s") == "fixture",
                f"got={git(fixture, 'log', '-1', '--format=%s')!r}",
            )
            check(
                "branch wt-a carries the commit",
                git(fixture, "log", "wt-a", "-1", "--format=%s") == "wt commit",
            )

            # --- scenario 5: the diff reads the worktree's HEAD blob ----------
            # The primary's HEAD has no `only-in-wt.txt`: a diff read on the
            # primary would answer `absent` (original ""), so the original side
            # discriminates the tree.
            (wt / "only-in-wt.txt").write_bytes(b"changed\n")
            page.evaluate(f"(s) => {SH}.loadChanges(s)", arg=slug)
            page.wait_for_function(HAS_UNSTAGED, arg=[slug, "only-in-wt.txt"], timeout=15000)
            diff_id = f"diff:{slug}@wt-a:only-in-wt.txt"
            click_change_row(page, "only-in-wt.txt")
            page.wait_for_function(f"(id) => {SH}.tabs.some(t => t.id === id)", arg=diff_id, timeout=15000)
            check("the diff tab id carries the checkout pin", True, diff_id)
            wait_diff_mounted(page, diff_id)
            sides = page.evaluate(
                """() => { const eds = monaco.editor.getDiffEditors();
                  const ed = eds[eds.length - 1]; const m = ed.getModel();
                  return { original: m.original.getValue(), modified: m.modified.getValue() }; }"""
            )
            check(
                "the diff's original side is the worktree's HEAD blob",
                sides["original"] == "worktree\n",
                f"got={sides['original']!r}",
            )
            check(
                "the diff's modified side is the worktree's file",
                sides["modified"] == "changed\n",
                f"got={sides['modified']!r}",
            )
            page.evaluate(f"(id) => {SH}.closeTab(id)", arg=diff_id)
            # `side` lacks only-in-wt.txt: a modified copy would block the
            # checkout, so restore it from git before the branch acts.
            git(wt, "checkout", "--", "only-in-wt.txt")

            # --- scenario 6: switchBranch moves the worktree's HEAD only ------
            open_picker(page, slug)
            cur = page.evaluate(f"() => {SH}.branchModal.current")
            check("the picker's current is the worktree's branch", cur == "wt-a", f"got={cur!r}")
            primary_row = page.evaluate(f"() => {SH}.branchModal.primaryBranch")
            check("the picker still knows the primary's own branch", primary_row == "main", f"got={primary_row!r}")
            page.evaluate(f"() => {SH}.switchBranch('side')")
            check("switchBranch('side') moves the worktree's HEAD to side", wait_git(wt, "side"), f"got={head_branch(wt)!r}")
            check("the primary's HEAD stays on main", head_branch(fixture) == "main", f"got={head_branch(fixture)!r}")
            page.wait_for_function(CHIP_IS, arg="side", timeout=15000)
            check("the chip reads `side`", True)
            # The sync row and the commit button follow the moved HEAD: the
            # `sync.status` re-read runs in the worktree too.
            page.wait_for_function(
                f"(s) => ({SH}.syncByProject[s] || {{}}).branch === 'side'", arg=slug, timeout=15000
            )
            label = page.evaluate(f"() => {SH}.commitTarget().label")
            check("after the switch the commit button reads `Commit to side`", label == "Commit to side", f"got={label!r}")
            show_view(page, "changes")
            page.wait_for_function(HAS_UNSTAGED, arg=[slug, "dirty.txt"], timeout=15000)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.changes-view .sync-row, .changes-view .sync-branch, .changes-view *')]"
                "  .some(e => e.children.length === 0 && e.textContent.trim() === 'side' && e.offsetParent !== null)",
                timeout=15000,
            )
            # The Changes view (the slice's subject): the sync row on `side`,
            # the worktree's rows, `Commit to side`. The chip is a Projects-view
            # element, asserted above by value.
            shot = os.path.join(SHOT_DIR, "407-worktree-changes-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # --- scenario 7: a branch the primary holds is refused by git -----
            open_picker(page, slug)
            page.evaluate(f"() => {{ {SH}.branchError = ''; {SH}.switchBranch('main'); }}")
            page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=15000)
            err = page.evaluate(f"() => {SH}.branchError")
            check(
                "switchBranch('main') is sent and refused by git (already used by the primary)",
                "already" in (err or "") and "main" in (err or ""),
                f"got={err!r}",
            )
            check("the worktree's HEAD did not move", head_branch(wt) == "side", f"got={head_branch(wt)!r}")
            check("the primary's HEAD did not move", head_branch(fixture) == "main", f"got={head_branch(fixture)!r}")

            # --- scenario 8: createBranch on the worktree ---------------------
            open_picker(page, slug)
            page.evaluate(f"() => {{ {SH}.branchError = ''; {SH}.branchModal.filter = 'from-wt'; {SH}.createBranch(); }}")
            check("createBranch() checks the worktree out onto from-wt", wait_git(wt, "from-wt"), f"got={head_branch(wt)!r}")
            check("the primary's HEAD stays on main after the create", head_branch(fixture) == "main")
            page.wait_for_function(CHIP_IS, arg="from-wt", timeout=15000)
            check("the chip reads `from-wt`", True)

            # --- scenario 9: primary shows the primary's Changes --------------
            click_row(page, "primary", slug)
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            page.wait_for_function(
                f"(s) => !({SH}.changesUnstaged[s] || []).some(e => e.path === 'dirty.txt')",
                arg=slug,
                timeout=15000,
            )
            page.wait_for_function(CHIP_IS, arg="main", timeout=10000)
            check("selecting primary lists the primary's Changes (no dirty.txt) and the chip reads `main`", True)

            # --- scenario 10 --------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 25
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
