"""#362 browser acceptance: the workbench explorer's tab, dialog and menu fixes.

One Playwright pass over a REAL daemon proving the five explorer defects are
gone: a right-click no longer costs the operator their tab, the create dialog
asks for a name instead of suggesting one, a created entry is revealed even under
a COLLAPSED parent, the menu offers both path forms as flat rows, and Duplicate
names the copy itself.

Scenario a  right-clicking an open file tab leaves the tab strip untouched — the
            regression this issue exists for (`auxclick` fires for button 2 too)
Scenario b  middle-clicking the SAME tab still closes it, and so does the ×
Scenario c  creating a folder under a COLLAPSED directory reveals AND selects it
            (`tree.dirty` for a collapsed dir is deliberately dropped, so only
            `revealRel`'s own ancestor expansion can put it on screen)
Scenario d  the context menu carries `Copy full path` and `Copy relative path` as
            two FLAT buttons — over a file node and over a folder node
Plus        the dialog asks rather than suggests (empty placeholder, instructional
            message); `revealRel` exists as one named function and `_reconcileOnce`
            routes its re-activation through it; `/api/repos` serves an absolute
            native `root`; creating a FILE still opens its tab; Duplicate produces
            `a copy.txt` with no prompt, then `a copy 2.txt` beside it.

Boots a Localhost daemon on 7441 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every row assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/362-explorer-2026-07-30.png.
Run: python crates/ralphy-daemon/tests/wb_explorer_362.py   (exit 0 = all pass)
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

PORT = 7441
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_explorer_362.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "362-explorer-2026-07-30.png")
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
    empty = tempfile.mkdtemp(prefix="wb362_empty_")
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


def seed():
    """A committed git repo with `a.txt` at the top level and a `deep/` directory
    that starts COLLAPSED — scenario c's whole point."""
    d = Path(tempfile.mkdtemp(prefix="wb362_repo_")) / "explorer-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    (d / "deep").mkdir()
    (d / "deep" / "inner.txt").write_text("inner\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb362@example.com")
    git(d, "config", "user.name", "wb362")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's explorer.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# Every LAID-OUT tree row's title. A row that is present but not laid out is
# excluded, so "revealme is on screen" cannot pass on a zero-height row.
ROW_TITLES = (
    "() => [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)

# One laid-out row by title, with whether it carries Wunderbaum's active class.
ROW_BY_TITLE = """
(title) => {
  const row = [...document.querySelectorAll('.wb-host .wb-row')]
    .find(r => r.querySelector('.wb-title')?.textContent.trim() === title);
  if (!row) return null;
  return {
    laid: row.offsetParent !== null && row.clientWidth > 0,
    width: row.clientWidth,
    active: row.classList.contains('wb-active'),
  };
}
"""

# The visible tab titles, in strip order. The Consoles tab rides at index 0.
TAB_TITLES = (
    "() => [...document.querySelectorAll('.tabstrip .tab')]"
    "  .filter(t => t.offsetParent !== null && t.clientWidth > 0)"
    "  .map(t => t.querySelector('.tab-title')?.textContent.trim())"
)

# The context menu's rows, flat: every direct button's label, plus how many
# nested menus it grew (the issue forbids a submenu).
MENU_ROWS = """
() => {
  const m = document.getElementById('ctxmenu');
  return {
    open: m.style.display === 'block',
    labels: [...m.querySelectorAll(':scope > button.ctx-item')]
      .map(b => b.querySelector('span')?.textContent.trim()),
    nested: m.querySelectorAll('button .ctx-menu, button ul, .ctx-submenu').length,
  };
}
"""


def open_menu_on(page, rel):
    """Open the context menu over the tree node at `rel`, the way the tree's own
    `contextmenu` listener does (activate the node, then `showMenu`). Driving the
    Alpine method keeps the menu's CONTENTS the subject — a mouse right-click over
    a virtual-scrolled row would be testing hit geometry instead."""
    return page.evaluate(
        "(rel) => { const c = " + SH + ";"
        "  const n = c._tree.findFirst(x => c.relPath(x) === rel);"
        "  if (!n) return false; n.setActive(); c.showMenu(120, 120, n); return true; }",
        arg=rel,
    )


def answer_prompt(page, name):
    """Fill the open prompt dialog and submit it, returning what the dialog asked
    BEFORE it was answered (the placeholder/message this issue changed)."""
    # Gate on the input being LAID OUT, not merely on the flag: Alpine's x-show
    # display flip lands after the property write, so sampling on `open === true`
    # reads a box that measures zero everywhere (CONTEXT.md, the $nextTick trap).
    page.wait_for_function(f"() => {SH}.promptModal.open === true", timeout=10000)
    page.wait_for_function(
        "() => { const i = document.getElementById('prompt-input');"
        "  return !!i && i.offsetParent !== null && i.clientWidth > 0; }",
        timeout=10000,
    )
    asked = page.evaluate(
        "() => { const i = document.getElementById('prompt-input');"
        "  const m = document.querySelector('.prompt-modal .prompt-where');"
        "  return { placeholder: i ? i.getAttribute('placeholder') : null,"
        "    laid: !!i && i.offsetParent !== null && i.clientWidth > 0,"
        "    message: m ? m.textContent.trim() : '' }; }"
    )
    page.evaluate(f"(v) => {{ {SH}.promptModal.value = v; {SH}.promptSubmit(); }}", arg=name)
    page.wait_for_function(f"() => {SH}.promptModal.open === false", timeout=10000)
    return asked


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb362_reg_")
    fixture = seed()
    slug = register_fixture(daemon_dir, fixture)

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
            page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)

            # --- /api/repos carries an absolute native root -------------------
            # Fetched IN-PAGE, from the same origin the UI reads, so this is the
            # response the explorer actually joins onto.
            repos = page.evaluate("() => fetch('/api/repos').then(r => r.json())")
            served_root = (repos[0] or {}).get("root")
            check(
                "/api/repos serves an absolute native root for the project",
                bool(served_root)
                and os.path.isabs(served_root)
                and not served_root.startswith("\\\\?\\")
                and os.path.realpath(served_root) == os.path.realpath(fixture),
                "root={} fixture={}".format(served_root, fixture),
            )
            check(
                "…and `path` is untouched beside it (the peer contract)",
                (repos[0] or {}).get("path") not in (None, ""),
                "path={}".format((repos[0] or {}).get("path")),
            )

            # The slug rides as an ARGUMENT, never interpolated: a repo
            # registered from a Windows path carries backslashes a string
            # literal would swallow as escapes (#316).
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )

            # --- the reveal primitive is ONE named function --------------------
            check(
                "revealRel exists as a named function on the live component",
                page.evaluate(f"() => typeof {SH}.revealRel") == "function",
                "typeof={}".format(page.evaluate(f"() => typeof {SH}.revealRel")),
            )
            # Pin the CALL, not the noun: a comment naming `revealRel` would
            # satisfy a bare substring pin over the source (CONTEXT.md).
            reconcile_src = page.evaluate(f"() => {SH}._reconcileOnce.toString()")
            check(
                "_reconcileOnce re-activates THROUGH revealRel, not its own findFirst",
                "this.revealRel(target, { restore: true })" in reconcile_src
                and "setActive()" not in reconcile_src,
                "src_tail={!r}".format(reconcile_src[-220:]),
            )

            # --- scenario a: right-clicking a tab must not close it ------------
            page.evaluate(
                "(rel) => { const c = " + SH + ";"
                "  const n = c._tree.findFirst(x => c.relPath(x) === rel); c.openFile(n); }",
                arg="a.txt",
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.tabstrip .tab')].some("
                "t => t.offsetParent !== null && t.clientWidth > 0 && "
                "t.querySelector('.tab-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )
            before = page.evaluate(TAB_TITLES)
            tab = page.locator(".tabstrip .tab", has_text="a.txt").first
            box = tab.bounding_box()
            page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2, button="right")
            page.wait_for_timeout(300)
            after_right = page.evaluate(TAB_TITLES)
            check(
                "a right-click leaves the tab strip exactly as it was",
                after_right == before and "a.txt" in after_right,
                "before={} after={}".format(before, after_right),
            )

            # --- scenario b: middle-click still closes ------------------------
            box = tab.bounding_box()
            page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2, button="middle")
            page.wait_for_function(
                "() => ![...document.querySelectorAll('.tabstrip .tab')].some("
                "t => t.querySelector('.tab-title')?.textContent.trim() === 'a.txt')",
                timeout=10000,
            )
            after_middle = page.evaluate(TAB_TITLES)
            check(
                "a middle-click still closes the tab",
                "a.txt" not in after_middle and len(after_middle) == len(before) - 1,
                "after={}".format(after_middle),
            )

            # …and so does the × button, which this issue did not touch.
            page.evaluate(
                "(rel) => { const c = " + SH + ";"
                "  const n = c._tree.findFirst(x => c.relPath(x) === rel); c.openFile(n); }",
                arg="a.txt",
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.tabstrip .tab')].some("
                "t => t.querySelector('.tab-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )
            page.evaluate(
                "() => [...document.querySelectorAll('.tabstrip .tab')]"
                "  .find(t => t.querySelector('.tab-title')?.textContent.trim() === 'a.txt')"
                "  .querySelector('.tab-close').click()"
            )
            page.wait_for_timeout(300)
            check(
                "the × button is unchanged and still closes",
                "a.txt" not in page.evaluate(TAB_TITLES),
                "tabs={}".format(page.evaluate(TAB_TITLES)),
            )

            # --- scenario d: the two path rows, flat, for a FILE ---------------
            check("the menu opens over a file node", open_menu_on(page, "a.txt"))
            file_menu = page.evaluate(MENU_ROWS)
            check(
                "a file's menu carries both path forms as flat rows, no submenu",
                file_menu["open"]
                and file_menu["labels"].count("Copy full path") == 1
                and file_menu["labels"].count("Copy relative path") == 1
                and file_menu["nested"] == 0,
                "labels={}".format(file_menu["labels"]),
            )
            check(
                "…and Duplicate, which only a file gets",
                file_menu["labels"].count("Duplicate") == 1,
                "labels={}".format(file_menu["labels"]),
            )
            check("the menu opens over a folder node", open_menu_on(page, "deep"))
            folder_menu = page.evaluate(MENU_ROWS)
            check(
                "a folder's menu carries both path forms too, still flat",
                folder_menu["open"]
                and folder_menu["labels"].count("Copy full path") == 1
                and folder_menu["labels"].count("Copy relative path") == 1
                and folder_menu["nested"] == 0,
                "labels={}".format(folder_menu["labels"]),
            )
            check(
                "…but NOT Duplicate, which is a file-only byte-op",
                folder_menu["labels"].count("Duplicate") == 0,
                "labels={}".format(folder_menu["labels"]),
            )
            # What the ROWS actually put on the clipboard, captured by spying on
            # `navigator.clipboard.writeText` and CLICKING the menu buttons —
            # re-deriving the join in the test would prove only the test's own
            # arithmetic.
            page.evaluate(
                "() => { window.__copied = [];"
                "  Object.defineProperty(navigator, 'clipboard', { configurable: true,"
                "    value: { writeText: (t) => { window.__copied.push(t); return Promise.resolve(); } } }); }"
            )
            click_row = (
                "(label) => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                "  .find(b => b.querySelector('span')?.textContent.trim() === label).click()"
            )
            check("the menu re-opens over a file for the copy rows", open_menu_on(page, "a.txt"))
            page.evaluate(click_row, arg="Copy full path")
            check("the menu re-opens again", open_menu_on(page, "a.txt"))
            page.evaluate(click_row, arg="Copy relative path")
            copied = page.evaluate("() => window.__copied")
            check(
                "Copy full path writes the absolute native path, Copy relative the rel one",
                len(copied) == 2
                and os.path.isabs(copied[0])
                and os.path.realpath(copied[0]) == os.path.realpath(os.path.join(fixture, "a.txt"))
                and copied[1] == "a.txt",
                "copied={}".format(copied),
            )
            page.evaluate(f"() => {SH}.hideMenu()")

            # --- scenario c: create a folder under a COLLAPSED parent ----------
            collapsed = page.evaluate(
                "() => { const c = " + SH + ";"
                "  const n = c._tree.findFirst(x => c.relPath(x) === 'deep');"
                "  return !!n && !n.expanded; }"
            )
            check("the fixture's `deep/` really is collapsed before the create", collapsed)
            page.evaluate(
                "(rel) => { const c = " + SH + ";"
                "  const n = c._tree.findFirst(x => c.relPath(x) === rel); c.emitCreate(n, 'folder'); }",
                arg="deep",
            )
            asked = answer_prompt(page, "revealme")
            check(
                "the create dialog asks for a name and suggests none",
                asked["laid"]
                and asked["placeholder"] == ""
                and "what should it be called?" in asked["message"].lower(),
                "asked={}".format(asked),
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'revealme')",
                timeout=15000,
            )
            # Same paint lag as the duplicate below: wait for the class, then
            # measure, so a slow frame is never read as "not selected".
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.classList.contains('wb-active') && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'revealme')",
                timeout=10000,
            )
            revealed = page.evaluate(ROW_BY_TITLE, "revealme")
            check(
                "the new folder is revealed under its collapsed parent AND selected",
                revealed
                and revealed["laid"]
                and revealed["width"] > 0
                and revealed["active"],
                "row={}".format(revealed),
            )
            check(
                "it landed inside `deep/`, not at the root",
                os.path.isdir(os.path.join(fixture, "deep", "revealme")),
            )

            # --- creating a FILE still opens its tab --------------------------
            page.evaluate(f"() => {SH}.emitCreate(null, 'file')")
            answer_prompt(page, "made.txt")
            page.wait_for_function(
                "() => [...document.querySelectorAll('.tabstrip .tab')].some("
                "t => t.offsetParent !== null && t.clientWidth > 0 && "
                "t.querySelector('.tab-title')?.textContent.trim() === 'made.txt')",
                timeout=15000,
            )
            made_active = page.evaluate(
                "() => { const t = [...document.querySelectorAll('.tabstrip .tab')]"
                "  .find(e => e.querySelector('.tab-title')?.textContent.trim() === 'made.txt');"
                "  return !!t && t.classList.contains('active'); }"
            )
            check("creating a file still opens it in an active tab", made_active)

            # --- Duplicate: no prompt, a derived non-colliding name -----------
            check("the menu opens over a.txt for the duplicate", open_menu_on(page, "a.txt"))
            page.evaluate(
                "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                "  .find(b => b.querySelector('span')?.textContent.trim() === 'Duplicate').click()"
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a copy.txt')",
                timeout=15000,
            )
            dup = page.evaluate(ROW_BY_TITLE, "a copy.txt")
            check(
                "Duplicate names the copy itself, with NO prompt dialog",
                dup
                and dup["laid"]
                and page.evaluate(f"() => {SH}.promptModal.open") is False,
                "row={} promptOpen={}".format(dup, page.evaluate(f"() => {SH}.promptModal.open")),
            )
            # Selection is asserted on the TREE, then on the painted row: the
            # `wb-active` class lands a frame or more after `setActive()`, so
            # reading the DOM straight after the row appears is a false red.
            # Bounded WAIT, not an instant read: the watcher's own `tree.dirty`
            # for the same directory lands after the write, and the selection
            # converges once that pass settles. The end state is the criterion.
            page.wait_for_function(
                f"() => {SH}.relPath({SH}._tree.getActiveNode()) === 'a copy.txt'",
                timeout=10000,
            )
            check(
                "…and the copy is the selected node once the watcher pass settles",
                page.evaluate(f"() => {SH}.relPath({SH}._tree.getActiveNode())") == "a copy.txt",
                "active={}".format(page.evaluate(f"() => {SH}.relPath({SH}._tree.getActiveNode())")),
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.classList.contains('wb-active') && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a copy.txt')",
                timeout=10000,
            )
            check("…and the painted row shows that selection", True)
            check(
                "…and the copy carries the source's bytes, source intact",
                Path(fixture, "a copy.txt").read_text(encoding="utf-8") == "alpha\n"
                and Path(fixture, "a.txt").read_text(encoding="utf-8") == "alpha\n",
            )
            # A second duplicate must not collide with the first: the free-name
            # search is what the daemon refuses to do (`file.copy` → `exists`).
            check("the menu re-opens over a.txt", open_menu_on(page, "a.txt"))
            page.evaluate(
                "() => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
                "  .find(b => b.querySelector('span')?.textContent.trim() === 'Duplicate').click()"
            )
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a copy 2.txt')",
                timeout=15000,
            )
            titles = page.evaluate(ROW_TITLES)
            check(
                "a second duplicate steps the name instead of colliding",
                titles.count("a copy.txt") == 1 and titles.count("a copy 2.txt") == 1,
                "titles={}".format(titles),
            )

            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 30
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
