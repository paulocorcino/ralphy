"""#406 browser acceptance: selecting a worktree from the branch picker.

One Playwright pass over a REAL daemon proving the select path end to end
(ADR-0063 §2/§4): clicking a worktree row in the picker's Worktrees section
makes the project's Files tree, viewer and Find read from that checkout, the
branch chip names it, the choice survives a reload and a second browser (it
is desk state), the tree stays live inside the worktree, a Write under it is
refused, a branch act reaches git (which refuses a branch the primary has
checked out), and the first `unknown checkout` reply (the worktree removed
from disk) drops the selection.

Fixture: a repo on `main` with `README.md` and `only-in-primary.txt`, and one
worktree `wt-a` made by `ralphy worktree add` whose branch `wt-a` commits
`only-in-wt.txt` and removes `only-in-primary.txt` — the two trees differ by
one file each, so a row proves which tree is listed.

Scenario 1  the daemon is listening
Scenario 2  clicking the `wt-a` row closes the picker, `SH.checkouts[slug]`
            is `wt-a`, the tree's root rows include `only-in-wt.txt` and
            exclude `only-in-primary.txt`, the chip reads `wt-a`
Scenario 3  live nudge: writing `<wt-a>/nudge.txt` from Python makes a
            `nudge.txt` row appear with no click
Scenario 4  reload: `page.reload()` + open the project (no picker) → the
            selection and the chip are back, the worktree is listed
Scenario 5  a second browser context sees the same selection and chip
Scenario 6  the `primary` row clears the selection: the primary is listed and
            the chip reads `main`; re-selecting `wt-a` lists the worktree
Scenario 7  with `wt-a` selected `switchBranch("main")` is SENT (#407) and
            git refuses it (`main` is checked out in the primary): the
            `branchError` names `main`, and neither tree's HEAD moved
Scenario 8  a `file.write` carrying `checkout: "wt-a"` is refused with
            `not available`, and neither tree's `README.md` changed
Scenario 8b a tab opened under `wt-a` stays pinned to it after `primary` is
            selected; its pane's own Save is refused (`not available`) and the
            primary never gains the file
Scenario 9  `rmtree(<wt-a>)` then a tree read → `unknown checkout` drops the
            selection, the chip reads `main`, the primary is listed
Scenario 10 no page errors

Boots a Localhost daemon on 7453 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/406-worktree-select-2026-09-15.png.
Run: python crates/ralphy-daemon/tests/wb_worktree_406.py   (exit 0 = all pass)
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7453
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_406.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb406_empty_")
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


def seed(parent_prefix, name):
    """A committed git repo on `main` at a CHOSEN directory name, `.ralphy/`
    gitignored so a worktree under it never dirties the primary tree, with a
    file only the primary holds."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #406 select fixture repo.\n", encoding="utf-8")
    (d / "only-in-primary.txt").write_text("primary\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb406@example.com")
    git(d, "config", "user.name", "wb406")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
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
    (wt / "only-in-wt.txt").write_text("worktree\n", encoding="utf-8")
    git(wt, "config", "user.email", "wb406@example.com")
    git(wt, "config", "user.name", "wb406")
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
    # after any assets/ui edit or the browser loads yesterday's picker.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# Every LAID-OUT tree row's title (the wb_explorer_362.py probe).
ROW_TITLES = (
    "() => [...document.querySelectorAll('.project.open .wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)
TITLES_INCLUDE = (
    "(t) => [...document.querySelectorAll('.project.open .wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .some(r => r.querySelector('.wb-title')?.textContent.trim() === t)"
)
CHIP = "() => ((document.querySelector('li.project.open .project-head .branch-chip-name') || {}).textContent || '').trim()"
CHIP_IS = (
    "(t) => ((document.querySelector('li.project.open .project-head .branch-chip-name') || {}).textContent || '')"
    "  .trim() === t"
)
# A laid-out picker row by its `.worktree-name`.
ROW_BY_NAME = (
    "(n) => [...document.querySelectorAll('.branch-modal .worktree-item')]"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0)"
    "  .find(e => e.querySelector('.worktree-name')?.textContent.trim() === n) || null"
)


def wait_shell(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)
    page.wait_for_function(
        "() => Array.from(document.querySelectorAll('li.project'))"
        "  .filter(e => e.offsetParent !== null).length === 1",
        timeout=15000,
    )


def open_project(page, slug):
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(
        "() => { const c = document.querySelector('li.project.open .project-head .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )


def open_picker(page, slug):
    open_project(page, slug)
    page.evaluate("() => document.querySelector('li.project.open .project-head .branch-chip').click()")
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
    page.wait_for_function(f"() => {SH}.confirmModal.open", timeout=5000)
    page.evaluate(f"() => {SH}.confirmRespond(true)")
    page.wait_for_function(f"() => Object.keys({SH}.worktreeRemoving).length === 0", timeout=20000)




def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb406_reg_")
    fixture = seed("wb406_", "plain")
    wt = add_worktree(fixture)
    slug = register_fixture(daemon_dir, str(fixture))
    readme_primary = (fixture / "README.md").read_text(encoding="utf-8")
    readme_wt = (wt / "README.md").read_text(encoding="utf-8")

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

            # --- scenario 2: clicking the wt-a row selects the checkout -------
            open_picker(page, slug)
            before = page.evaluate(CHIP)
            check("before any selection the chip reads the primary's branch", before == "main", f"got={before!r}")
            click_row(page, "wt-a")
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=10000)
            check("clicking the row closes the picker and records the selection", True)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-wt.txt", timeout=15000)
            titles = page.evaluate(ROW_TITLES)
            check(
                "the Files tree lists the worktree: only-in-wt.txt in, only-in-primary.txt out",
                "only-in-wt.txt" in titles and "only-in-primary.txt" not in titles,
                f"titles={titles}",
            )
            page.wait_for_function(CHIP_IS, arg="wt-a", timeout=10000)
            check("the chip reads `wt-a`", True)
            shot = os.path.join(SHOT_DIR, "406-worktree-select-2026-09-15.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # --- scenario 3: the tree is live inside the worktree -------------
            (wt / "nudge.txt").write_text("x\n", encoding="utf-8")
            page.wait_for_function(TITLES_INCLUDE, arg="nudge.txt", timeout=15000)
            check("a file written into the worktree appears as a row with no click (live nudge)", True)

            # --- scenario 4: the selection survives a reload ------------------
            page.reload()
            wait_shell(page)
            open_project(page, slug)
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=15000)
            check("after a reload the selection is back from the desk", True)
            page.wait_for_function(CHIP_IS, arg="wt-a", timeout=15000)
            check("after a reload the chip reads `wt-a` without opening the picker", True)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-wt.txt", timeout=15000)
            check("after a reload the worktree is listed", True)

            # --- scenario 5: a second browser sees the same selection ---------
            ctx2 = browser.new_context(viewport={"width": 1440, "height": 900})
            page2 = ctx2.new_page()
            page2.on("pageerror", lambda e: thrown.append(str(e)))
            page2.goto(BASE)
            wait_shell(page2)
            open_project(page2, slug)
            page2.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=15000)
            page2.wait_for_function(CHIP_IS, arg="wt-a", timeout=15000)
            check("a second browser context reads the same selection and chip", True)
            ctx2.close()

            # --- scenario 6: the primary row clears the selection -------------
            click_row(page, "primary", slug)
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-primary.txt", timeout=15000)
            page.wait_for_function(CHIP_IS, arg="main", timeout=10000)
            titles = page.evaluate(ROW_TITLES)
            check(
                "the primary row clears the selection: the primary is listed and the chip reads `main`",
                "only-in-primary.txt" in titles and "only-in-wt.txt" not in titles,
                f"titles={titles}",
            )
            click_row(page, "wt-a", slug)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-wt.txt", timeout=15000)
            check("re-selecting wt-a lists the worktree again", True)

            # --- scenario 7: a branch act under a selected worktree reaches git
            # and is refused for a branch checked out in the primary (#407).
            # Git's wording is `already used by worktree` on 2.50 (`already
            # checked out` on older ones): assert the stable words.
            open_picker(page, slug)
            page.evaluate(f"() => {SH}.switchBranch('main')")
            page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=10000)
            err = page.evaluate(f"() => {SH}.branchError")
            check(
                "switchBranch('main') under wt-a is sent and refused by git (already used by the primary)",
                "already" in (err or "") and "main" in (err or ""),
                f"got={err!r}",
            )
            branch_now = git(fixture, "rev-parse", "--abbrev-ref", "HEAD")
            check("the primary's HEAD did not move", branch_now == "main", f"got={branch_now!r}")
            wt_branch = git(wt, "rev-parse", "--abbrev-ref", "HEAD")
            check("the worktree's HEAD did not move", wt_branch == "wt-a", f"got={wt_branch!r}")
            page.evaluate(f"() => {{ {SH}.branchError = ''; {SH}.branchOpen = false; }}")

            # --- scenario 8: a write under the worktree is refused ------------
            reply = page.evaluate(
                "(s) => WBDaemon.write('file.write', { repo: s, path: 'README.md', content: 'clobbered', checkout: 'wt-a' })",
                arg=slug,
            )
            check(
                "file.write with checkout is refused with `not available`",
                reply and reply.get("status") == "error" and "not available" in (reply.get("message") or ""),
                f"got={reply}",
            )
            check(
                "neither tree's README.md changed",
                (fixture / "README.md").read_text(encoding="utf-8") == readme_primary
                and (wt / "README.md").read_text(encoding="utf-8") == readme_wt,
            )

            # --- scenario 8b: the viewer pin — a tab opened under wt-a keeps its
            # checkout after the selection moves back to primary, and its Save
            # (the viewer's own `save` intent, pin included) is refused rather
            # than landing on the primary's README.md.
            page.evaluate(
                f"(s) => {SH}.openTab({{ project: s, path: 'only-in-wt.txt', title: 'only-in-wt.txt', ftype: 'code' }})",
                arg=slug,
            )
            page.wait_for_function(
                f"(s) => {SH}.tabs.some(t => t.path === 'only-in-wt.txt' && t.checkout === 'wt-a')",
                arg=slug,
                timeout=15000,
            )
            page.wait_for_function(
                "() => !!document.querySelector(\".viewer[data-tab-id$='@wt-a:only-in-wt.txt']\")",
                timeout=15000,
            )
            check("a tab opened under wt-a is pinned to it and its pane carries the pinned id", True)
            click_row(page, "primary", slug)
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            still = page.evaluate(
                f"() => ({SH}.tabs.find(t => t.path === 'only-in-wt.txt') || {{}}).checkout"
            )
            check("selecting primary leaves the open tab pinned to wt-a", still == "wt-a", f"got={still!r}")
            # The pane's own Save: `WBViewer.save` reads the pane and emits the
            # `save` intent with the pin, which the write bridge honours. The
            # flash self-clears after 2.6 s, so it is LATCHED, not polled.
            page.evaluate(
                f"() => {{ const sh = {SH}; sh._latched = []; const orig = sh._flashAction.bind(sh);"
                "  sh._flashAction = (m) => { sh._latched.push(String(m)); orig(m); }; }"
            )
            page.evaluate(
                "() => { const id = document.querySelector(\".viewer[data-tab-id$='@wt-a:only-in-wt.txt']\").dataset.tabId;"
                "  const el = document.querySelector('.viewer[data-tab-id=\"' + id + '\"] [data-act=\"save\"]');"
                "  el.click(); }"
            )
            page.wait_for_function(
                f"() => {SH}._latched.some(m => m.includes('not available'))",
                timeout=15000,
            )
            check("the pinned tab's Save is refused (`not available`), never written to the primary", True)
            check(
                "the primary has no only-in-wt.txt after the refused Save",
                not (fixture / "only-in-wt.txt").exists(),
            )
            page.evaluate(f"(s) => {SH}.closeTab('file:' + s + '@wt-a:only-in-wt.txt')", arg=slug)
            click_row(page, "wt-a", slug)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-wt.txt", timeout=15000)

            # --- scenario 9: unknown checkout resets the selection ------------
            # The pointer file goes with the directory: the daemon's next read
            # answers `unknown checkout`, which is the one reply that drops it.
            shutil.rmtree(wt)
            page.evaluate(f"() => {SH}.fetchTreeLevel('').catch(() => null)")
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === undefined", arg=slug, timeout=15000)
            page.wait_for_function(CHIP_IS, arg="main", timeout=10000)
            page.wait_for_function(TITLES_INCLUDE, arg="only-in-primary.txt", timeout=15000)
            check("after the worktree is removed, `unknown checkout` drops the selection and the primary shows", True)
            desk = page.evaluate("() => WBConsole.checkouts()")
            check("the desk mirror no longer carries the ref", slug not in desk, f"got={desk}")

            # --- scenario 10 --------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 24
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
