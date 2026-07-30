"""#364 browser acceptance: moving a file or a folder to another directory.

One Playwright pass over a REAL daemon proving the explorer grew a MOVE, not
just a rename: the destination is picked from a browser of real directories, the
node lands there on disk, refusals are named rather than silently swallowed, and
an already-open tab follows the file to its new path instead of writing to the
one it left.

Scenario a  the context menu carries a `Move to…` row over a FILE and over a
            FOLDER (a folder move is the half a file-only byte-op cannot do)
Scenario b  moving `a.txt` into `dst/` lands it there, removes it from the root,
            and leaves the tree SELECTED on the destination
Scenario c  moving the folder `src/` into `dst/` carries its contents
Scenario d  a move onto an existing name is refused — source intact, the
            destination's bytes untouched
Scenario e  a destination inside `.ralphy` is refused through the picker, and a
            `../escaped.txt` destination is refused through the shell's own
            `performMove` (the picker cannot express it, the daemon still must)
Scenario f  an open tab for the moved file is RE-PATHED: its `path`/`id` follow,
            and a save through that same tab writes to the new location with no
            file recreated at the old one
Plus        two source pins: the shared rename listener no longer composes a
            parent, and `performMove` reveals through `revealRel(` rather than a
            re-implemented expand/activate sequence.

Boots a Localhost daemon on 7442 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every selection assertion is a BOUNDED WAIT, never a point read: the daemon's own
`tree.dirty` for the same directory arrives after the write and the selection
converges once that pass settles (#362 handoff).

Writes docs/screenshots/364-move-2026-07-30.png.
Run: python crates/ralphy-daemon/tests/wb_move_364.py   (exit 0 = all pass)
"""

import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7442
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_move_364.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
APP_JS = os.path.join(REPO_ROOT, "crates", "ralphy-daemon", "assets", "ui", "app.js")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "364-move-2026-07-30.png")
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
    empty = tempfile.mkdtemp(prefix="wb364_empty_")
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
    """A committed git repo laid out for every move this suite drives: a file to
    move, a folder to move, a destination that already holds a colliding name,
    and a `.ralphy/` the picker can reach (unlike `.git`, which `HARD_EXCLUDE`
    drops from every listing)."""
    d = Path(tempfile.mkdtemp(prefix="wb364_repo_")) / "move-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    (d / "dup.txt").write_text("source-copy\n", encoding="utf-8")
    (d / "dst").mkdir()
    (d / "dst" / "dup.txt").write_text("keep-me", encoding="utf-8")
    (d / "src").mkdir()
    (d / "src" / "inner.txt").write_text("inner\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb364@example.com")
    git(d, "config", "user.name", "wb364")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    # After the commit, so the ignored dir never enters the index.
    (d / ".ralphy").mkdir()
    (d / ".ralphy" / "keep.txt").write_text("keep", encoding="utf-8")
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


# Every LAID-OUT tree row's REL PATH. Row titles alone cannot tell a root
# `a.txt` from a `dst/a.txt` after the reveal expanded the destination, which is
# the exact distinction a move has to make. A row that is present but not laid
# out is excluded, so "it is on screen" cannot pass on a zero-height row.
REL_PATHS = """
() => {
  const c = Alpine.$data(document.querySelector('[x-data]'));
  return [...document.querySelectorAll('.wb-host .wb-row')]
    .filter(r => r.offsetParent !== null && r.clientWidth > 0)
    .map(r => mar10.Wunderbaum.getNode(r))
    .filter(Boolean)
    .map(n => c.relPath(n));
}
"""

# The context menu's rows, flat.
MENU_LABELS = (
    "() => { const m = document.getElementById('ctxmenu');"
    "  return { open: m.style.display === 'block',"
    "    labels: [...m.querySelectorAll(':scope > button.ctx-item')]"
    "      .filter(b => b.offsetParent !== null && b.clientWidth > 0)"
    "      .map(b => b.querySelector('span')?.textContent.trim()) }; }"
)

# The picker's visible rows and its Move-here button state.
PICK_STATE = """
() => {
  const c = Alpine.$data(document.querySelector('[x-data]'));
  const modal = document.querySelector('.move-picker');
  const rows = [...document.querySelectorAll('.move-picker .move-row')]
    .filter(r => r.offsetParent !== null && r.clientWidth > 0)
    .map(r => r.querySelector('span')?.textContent.trim());
  const btn = [...document.querySelectorAll('.move-picker .modal-foot .btn')]
    .find(b => b.textContent.trim() === 'Move here');
  return {
    open: c.movePick.open,
    laid: !!modal && modal.offsetParent !== null && modal.clientWidth > 0,
    dir: c.movePick.dir,
    from: c.movePick.from,
    rows,
    error: c.movePick.error,
    disabled: !!btn && btn.disabled,
  };
}
"""

# The shell's file tabs (the Consoles tab has no path).
TABS = f"() => {SH}.tabs.filter(t => t.path).map(t => ({{ id: t.id, path: t.path, kind: t.kind }}))"


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


def click_menu_row(page, label):
    page.evaluate(
        "(label) => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
        "  .find(b => b.querySelector('span')?.textContent.trim() === label).click()",
        arg=label,
    )


def wait_picker_open(page):
    """Gate on the modal being LAID OUT, not merely on the flag: Alpine's x-show
    display flip lands after the property write, so sampling on `open === true`
    reads a box that measures zero everywhere (the $nextTick trap)."""
    page.wait_for_function(f"() => {SH}.movePick.open === true", timeout=10000)
    page.wait_for_function(
        "() => { const m = document.querySelector('.move-picker');"
        "  return !!m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=10000,
    )
    # …and for the first listing to LAND: the rows are a `tree.list` round trip,
    # so a state read on the open flag alone samples an empty picker.
    page.wait_for_function(f"() => {SH}.movePick.busy === false", timeout=10000)


def pick_into(page, name):
    """Click the picker's folder row for `name` and wait for the browsed dir to
    become it — the listing is a round trip, so the rows lag the click."""
    page.wait_for_function(
        "(name) => [...document.querySelectorAll('.move-picker .move-row')]"
        "  .some(r => r.offsetParent !== null && r.clientWidth > 0 && "
        "    r.querySelector('span')?.textContent.trim() === name)",
        arg=name,
        timeout=10000,
    )
    page.evaluate(
        "(name) => [...document.querySelectorAll('.move-picker .move-row')]"
        "  .find(r => r.querySelector('span')?.textContent.trim() === name).click()",
        arg=name,
    )
    page.wait_for_function(
        "(name) => { const c = " + SH + ";"
        "  return !c.movePick.busy && (c.movePick.dir === name || c.movePick.dir.endsWith('/' + name)); }",
        arg=name,
        timeout=10000,
    )


def click_move_here(page):
    page.evaluate(
        "() => [...document.querySelectorAll('.move-picker .modal-foot .btn')]"
        "  .find(b => b.textContent.trim() === 'Move here').click()"
    )


def open_file(page, rel):
    page.evaluate(
        "(rel) => { const c = " + SH + ";"
        "  const n = c._tree.findFirst(x => c.relPath(x) === rel); c.openFile(n); }",
        arg=rel,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb364_reg_")
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

            # The slug rides as an ARGUMENT, never interpolated: a repo registered
            # from a Windows path carries backslashes a literal would swallow.
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )

            # The refusal channel is `_flashAction`, whose own state clears itself
            # after 2.6 s — a latch that never expires is what makes a refusal
            # assertable without racing the timeout.
            page.evaluate(
                "() => { const c = " + SH + "; c.__flash = '';"
                "  const orig = c._flashAction.bind(c);"
                "  c._flashAction = (m) => { c.__flash = m; return orig(m); }; }"
            )

            # --- source pins ---------------------------------------------------
            # The EXPRESSION, not the noun: a comment mentioning either would
            # satisfy a bare-word pin (the #302 trap).
            src = Path(APP_JS).read_text(encoding="utf-8")
            rename_arm = src.split('case "rename": {', 1)[1].split("break;", 1)[0]
            check(
                "the shared rename listener no longer composes a parent onto from/to",
                "const to = parent ?" not in rename_arm and "parentOf(" not in src,
                "arm={!r}".format(rename_arm.strip()[:120]),
            )
            perform = src.split("async performMove(from, to) {", 1)[1].split("\n    },", 1)[0]
            check(
                "performMove reveals through the named revealRel primitive",
                "revealRel(" in perform,
                "found={}".format("revealRel(" in perform),
            )

            # --- scenario a: the menu row, for a file AND for a folder ---------
            check("the menu opens over a file node", open_menu_on(page, "a.txt"))
            file_menu = page.evaluate(MENU_LABELS)
            check(
                "a file's menu carries a `Move to…` row",
                file_menu["open"] and file_menu["labels"].count("Move to…") == 1,
                "labels={}".format(file_menu["labels"]),
            )
            check("the menu opens over a folder node", open_menu_on(page, "src"))
            folder_menu = page.evaluate(MENU_LABELS)
            check(
                "…and so does a folder's — a move is not a file-only byte-op",
                folder_menu["open"] and folder_menu["labels"].count("Move to…") == 1,
                "labels={}".format(folder_menu["labels"]),
            )
            page.evaluate(f"() => {SH}.hideMenu()")

            # --- scenario f (opening half): a tab open on the file being moved --
            # Opened BEFORE the move so scenario b's move is the thing that
            # re-paths it; the assertions land after b.
            open_file(page, "a.txt")
            page.wait_for_function(
                "() => [...document.querySelectorAll('.tabstrip .tab')].some("
                "t => t.offsetParent !== null && t.clientWidth > 0 && "
                "t.querySelector('.tab-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )
            before_tabs = page.evaluate(TABS)
            check(
                "a tab is open on `a.txt` before the move",
                any(t["path"] == "a.txt" for t in before_tabs),
                "tabs={}".format(before_tabs),
            )

            # --- scenario b: move a.txt into dst/ ------------------------------
            check("the menu re-opens over a.txt for the move", open_menu_on(page, "a.txt"))
            click_menu_row(page, "Move to…")
            wait_picker_open(page)
            opened = page.evaluate(PICK_STATE)
            check(
                "the picker opens laid out, at the source's own directory, listing folders",
                opened["laid"] and opened["dir"] == "" and opened["from"] == "a.txt"
                and "dst" in opened["rows"] and "src" in opened["rows"],
                "state={}".format(opened),
            )
            check(
                "…with `Move here` dead while the browsed dir IS the source's parent",
                opened["disabled"],
                "disabled={}".format(opened["disabled"]),
            )
            # The screenshot is taken HERE: the picker open over the explorer is
            # exactly the state the ledger cites.
            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            pick_into(page, "dst")
            in_dst = page.evaluate(PICK_STATE)
            check(
                "browsing into `dst` moves the breadcrumb and re-enables `Move here`",
                in_dst["dir"] == "dst" and not in_dst["disabled"],
                "state={}".format(in_dst),
            )
            click_move_here(page)
            page.wait_for_function(
                f"() => {SH}.relPath({SH}._tree.getActiveNode()) === 'dst/a.txt'",
                timeout=15000,
            )
            check(
                "moving `a.txt` into `dst` lands it there on disk",
                Path(fixture, "dst", "a.txt").exists()
                and Path(fixture, "dst", "a.txt").read_text(encoding="utf-8") == "alpha\n",
            )
            check(
                "…and removes it from the root",
                not Path(fixture, "a.txt").exists(),
            )
            # A bounded wait already proved it above; the read is the end state.
            check(
                "…and the tree selects the node at its DESTINATION",
                page.evaluate(f"() => {SH}.relPath({SH}._tree.getActiveNode())") == "dst/a.txt",
                "active={}".format(page.evaluate(f"() => {SH}.relPath({SH}._tree.getActiveNode())")),
            )

            # --- scenario f: the open tab followed the file --------------------
            after_tabs = page.evaluate(TABS)
            moved_tab = next((t for t in after_tabs if t["path"] == "dst/a.txt"), None)
            check(
                "the open tab is re-pathed to the destination, id and all",
                moved_tab is not None and moved_tab["id"] == f"file:{slug}:dst/a.txt",
                "tabs={}".format(after_tabs),
            )
            check(
                "…and no tab is left pointing at the old path",
                not any(t["path"] == "a.txt" for t in after_tabs),
                "tabs={}".format(after_tabs),
            )
            check(
                "…and the viewer pane's own record moved with it",
                page.evaluate(
                    "(id) => !!document.querySelector(`.viewer[data-tab-id='${id}']`)",
                    arg=f"file:{slug}:dst/a.txt",
                ),
            )
            # A SAVE through that same tab: the viewer's `save` emits `rec.path`,
            # so a stale record would write to the root and recreate `a.txt`.
            page.evaluate(f"(id) => {SH}.activate(id)", arg=f"file:{slug}:dst/a.txt")
            page.wait_for_selector(
                f".viewer[data-tab-id='file:{slug}:dst/a.txt'] .monaco-editor", timeout=20000
            )
            editor = page.locator(f".viewer[data-tab-id='file:{slug}:dst/a.txt'] .monaco-editor").first
            editor.click()
            page.keyboard.press("Control+A")
            page.keyboard.type("moved-and-saved")
            page.evaluate(
                "(id) => document.querySelector(`.viewer[data-tab-id='${id}'] [data-act=\"save\"]`).click()",
                arg=f"file:{slug}:dst/a.txt",
            )
            deadline = time.time() + 15
            while time.time() < deadline:
                if Path(fixture, "dst", "a.txt").read_text(encoding="utf-8").strip() == "moved-and-saved":
                    break
                page.wait_for_timeout(200)
            check(
                "a save through the moved tab writes to the NEW location",
                Path(fixture, "dst", "a.txt").read_text(encoding="utf-8").strip() == "moved-and-saved",
                "bytes={!r}".format(Path(fixture, "dst", "a.txt").read_text(encoding="utf-8")),
            )
            check(
                "…and recreates nothing at the old one",
                not Path(fixture, "a.txt").exists(),
            )

            # --- scenario c: move the FOLDER src/ into dst/ ---------------------
            check("the menu opens over the folder `src`", open_menu_on(page, "src"))
            click_menu_row(page, "Move to…")
            wait_picker_open(page)
            folder_pick = page.evaluate(PICK_STATE)
            check(
                "the picker hides the moved folder itself (a dir cannot move into its own subtree)",
                "src" not in folder_pick["rows"] and "dst" in folder_pick["rows"],
                "rows={}".format(folder_pick["rows"]),
            )
            pick_into(page, "dst")
            click_move_here(page)
            page.wait_for_function(
                f"() => {SH}.relPath({SH}._tree.getActiveNode()) === 'dst/src'",
                timeout=15000,
            )
            check(
                "moving the folder carries its contents to the destination",
                Path(fixture, "dst", "src", "inner.txt").exists()
                and Path(fixture, "dst", "src", "inner.txt").read_text(encoding="utf-8") == "inner\n",
            )
            check(
                "…and leaves nothing behind at the root",
                not Path(fixture, "src").exists(),
            )

            # --- scenario d: a move onto an existing name is refused ------------
            check("the menu opens over `dup.txt`", open_menu_on(page, "dup.txt"))
            click_menu_row(page, "Move to…")
            wait_picker_open(page)
            pick_into(page, "dst")
            click_move_here(page)
            # The refusal is a FLASH, not a tree write, so wait for the message.
            page.wait_for_function(
                f"() => !!{SH}.__flash && /refused|exists/i.test({SH}.__flash)",
                timeout=15000,
            )
            check(
                "a move onto an existing name is refused with a reason",
                bool(re.search(r"refused|exists", page.evaluate(f"() => {SH}.__flash") or "", re.I)),
                "flash={!r}".format(page.evaluate(f"() => {SH}.__flash")),
            )
            check(
                "…the source is still where it was",
                Path(fixture, "dup.txt").exists()
                and Path(fixture, "dup.txt").read_text(encoding="utf-8") == "source-copy\n",
            )
            check(
                "…and the destination's bytes are untouched, never overwritten",
                Path(fixture, "dst", "dup.txt").read_text(encoding="utf-8") == "keep-me",
                "bytes={!r}".format(Path(fixture, "dst", "dup.txt").read_text(encoding="utf-8")),
            )

            # --- scenario e: protected dir, and an escape ----------------------
            # `.ralphy` is reachable through the picker (unlike `.git`, which
            # `tree::HARD_EXCLUDE` drops from every listing) — so it is the one
            # protected destination an operator can actually aim at.
            check("the menu opens over `dst/a.txt` for the protected-dir move", open_menu_on(page, "dst/a.txt"))
            click_menu_row(page, "Move to…")
            wait_picker_open(page)
            page.evaluate(f"() => {SH}.movePickUp()")
            page.wait_for_function(f"() => {SH}.movePick.dir === '' && !{SH}.movePick.busy", timeout=10000)
            ralphy_listed = page.evaluate(PICK_STATE)
            check(
                "`.ralphy` is offered by the picker (the refusal must come from the daemon)",
                ".ralphy" in ralphy_listed["rows"],
                "rows={}".format(ralphy_listed["rows"]),
            )
            pick_into(page, ".ralphy")
            page.evaluate(f"() => {SH}.__flash = ''")
            click_move_here(page)
            page.wait_for_function(
                f"() => !!{SH}.__flash && /refused|escapes|confined/i.test({SH}.__flash)",
                timeout=15000,
            )
            check(
                "a move INTO `.ralphy` is refused",
                not Path(fixture, ".ralphy", "a.txt").exists()
                and Path(fixture, "dst", "a.txt").exists(),
                "flash={!r}".format(page.evaluate(f"() => {SH}.__flash")),
            )
            # The escape leg goes through the shell's own move-perform function:
            # the picker cannot express `..`, but the daemon still has to refuse it.
            page.evaluate(f"() => {SH}.__flash = ''")
            page.evaluate(f"() => {SH}.performMove('dst/a.txt', '../escaped.txt')")
            page.wait_for_function(
                f"() => !!{SH}.__flash && /refused|escapes|confined/i.test({SH}.__flash)",
                timeout=15000,
            )
            check(
                "a destination outside the repo root is refused, and nothing lands beside it",
                not Path(fixture).parent.joinpath("escaped.txt").exists()
                and Path(fixture, "dst", "a.txt").exists(),
                "flash={!r}".format(page.evaluate(f"() => {SH}.__flash")),
            )
            check(
                "…and the refused moves left the tab strip alone",
                [t["path"] for t in page.evaluate(TABS)] == ["dst/a.txt"],
                "tabs={}".format(page.evaluate(TABS)),
            )

            # REL PATHS, not row titles: the reveal expanded `dst/`, so a flat
            # title list still contains `a.txt` and `src` — as the CHILDREN of
            # the destination. Only the rel path can tell the two apart.
            rels = page.evaluate(REL_PATHS)
            check(
                "the tree ends with the moved entries under `dst/`, gone from the root",
                "a.txt" not in rels and "src" not in rels
                and "dst/a.txt" in rels and "dst/src" in rels,
                "rels={}".format(rels),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 35
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
