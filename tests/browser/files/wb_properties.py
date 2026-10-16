"""Browser acceptance: the Properties dialog of the file tree.

One Playwright pass over a REAL daemon proving that the tree menu and the
keyboard open a dialog that shows the facts of a file, the size of a folder
and the size of the project root, in the order the daemon answers them.

Scenario a  the context menu carries a `Properties` row over a FILE, a FOLDER
            and EMPTY SPACE (the project root)
Scenario b  a file shows its size, type, modified time, read-only and ignored
            facts, as the daemon read them from the disk
Scenario c  a folder shows a spinner and `Calculating…` while the count runs,
            then its size, what it contains and the ignored part (the count is
            held back in the page, so the wait can be seen)
Scenario d  the project root is a folder named for the project, with its full
            path, and the ignored folder counts as ignored contents
Scenario e  `Alt+Enter` on a clicked row opens the dialog, and Enter alone does
            not (the unit tests pin that plain Enter reaches the tree)
Scenario f  Escape and Close end it, and a reply that comes after it was closed
            changes nothing

Boots a Localhost daemon on 7455 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes .ralphy/screenshots/properties-2026-10-15.png.
Run: python tests/browser/files/wb_properties.py   (exit 0 = all pass)
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

PORT = 7455
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/files/wb_properties.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SHOT = os.path.join(SHOT_DIR, "properties-2026-10-15.png")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
FILES = "Alpine.$data(document.querySelector('[x-data=\"wbFiles\"]'))"
PROPS = "Alpine.$data(document.querySelector('[x-data=\"wbPropertiesDialog\"]'))"

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
    empty = tempfile.mkdtemp(prefix="wbprops_empty_")
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
    """A committed git repo: a text file of 5 bytes, a folder of 3 files, an
    ignored folder of 100 bytes, and a PNG."""
    d = Path(tempfile.mkdtemp(prefix="wbprops_repo_")) / "props-fixture"
    d.mkdir()
    (d / ".gitignore").write_text("ign/\n", encoding="utf-8")
    (d / "a.txt").write_text("hello", encoding="utf-8")
    (d / "pack").mkdir()
    (d / "pack" / "one.bin").write_bytes(b"x" * 10)
    (d / "pack" / "two.bin").write_bytes(b"x" * 20)
    (d / "pack" / "inner").mkdir()
    (d / "pack" / "inner" / "three.bin").write_bytes(b"x" * 30)
    (d / "logo.png").write_bytes(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "props@example.com")
    git(d, "config", "user.name", "props")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    (d / "ign").mkdir()
    (d / "ign" / "blob").write_bytes(b"x" * 100)
    return str(d)


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's workbench.
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


MENU_LABELS = (
    "() => { const m = document.getElementById('ctxmenu');"
    "  return { open: m.style.display === 'block',"
    "    labels: [...m.querySelectorAll(':scope > button.ctx-item')]"
    "      .filter(b => b.offsetParent !== null && b.clientWidth > 0)"
    "      .map(b => b.querySelector('span')?.textContent.trim()) }; }"
)

# The dialog as the operator sees it: laid out, each row's label and value, and
# whether its spinner is drawn.
DIALOG = """
() => {
  const modal = document.querySelector('.properties-modal');
  const laid = !!modal && modal.offsetParent !== null && modal.clientWidth > 0;
  const rows = [...document.querySelectorAll('.props-row')]
    .filter(r => r.offsetParent !== null)
    .map(r => ({
      label: r.querySelector('dt').textContent.trim(),
      value: r.querySelector('dd > span:not(.files-spinner)').textContent.trim(),
      spinner: (() => { const s = r.querySelector('.files-spinner');
        return !!s && s.offsetParent !== null && s.clientWidth > 0; })(),
    }));
  return { laid, rows };
}
"""


def row(state, label):
    return next((r for r in state["rows"] if r["label"] == label), None)


def open_menu_on(page, rel):
    """Open the context menu over the tree node at `rel` (`None`: empty space),
    the way the tree's own `contextmenu` listener does."""
    return page.evaluate(
        "(rel) => { const c = " + FILES + ";"
        "  const n = rel === null ? null : c._tree.findFirst(x => c.relPath(x) === rel);"
        "  if (rel !== null && !n) return false; if (n) n.setActive(); c.showMenu(120, 120, n); return true; }",
        arg=rel,
    )


def click_menu_row(page, label):
    page.evaluate(
        "(label) => [...document.querySelectorAll('#ctxmenu > button.ctx-item')]"
        "  .find(b => b.querySelector('span')?.textContent.trim() === label).click()",
        arg=label,
    )


def wait_dialog(page):
    page.wait_for_function(f"() => {PROPS}.props.open === true", timeout=10000)
    page.wait_for_function(
        "() => { const m = document.querySelector('.properties-modal');"
        "  return !!m && m.offsetParent !== null && m.clientWidth > 0; }",
        timeout=10000,
    )


def settled(page):
    page.wait_for_function(f"() => !{PROPS}.propsBusy()", timeout=15000)
    return page.evaluate(DIALOG)


def close_dialog(page):
    page.keyboard.press("Escape")
    page.wait_for_function(f"() => {PROPS}.props.open === false", timeout=5000)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbprops_reg_")
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
            page.wait_for_function("() => Alpine.store('projects').projects.length === 1", timeout=15000)
            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function("(s) => Alpine.store('projects').openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'a.txt')",
                timeout=15000,
            )

            # -- a: the menu row, over a file, a folder and empty space --------
            for rel, what in (("a.txt", "a file"), ("pack", "a folder"), (None, "empty space")):
                opened = open_menu_on(page, rel)
                menu = page.evaluate(MENU_LABELS)
                check(f"a: the menu opens over {what}", opened and menu["open"], str(menu))
                check(f"a: Properties is a row over {what}", "Properties" in menu["labels"], str(menu["labels"]))
                page.keyboard.press("Escape")

            # -- b: a file ------------------------------------------------------
            open_menu_on(page, "a.txt")
            click_menu_row(page, "Properties")
            wait_dialog(page)
            state = settled(page)
            check("b: the dialog is laid out", state["laid"])
            check("b: the name", (row(state, "Name") or {}).get("value") == "a.txt", str(state["rows"]))
            check("b: the size is 5 bytes", (row(state, "Size") or {}).get("value") == "5 bytes", str(row(state, "Size")))
            check("b: the type is UTF-8 text", (row(state, "Type") or {}).get("value") == "Text · UTF-8", str(row(state, "Type")))
            check("b: not ignored", (row(state, "Ignored by git") or {}).get("value") == "No", str(row(state, "Ignored by git")))
            check("b: not read-only", (row(state, "Read-only") or {}).get("value") == "No", str(row(state, "Read-only")))
            modified = (row(state, "Modified") or {}).get("value", "")
            check("b: a time of change", len(modified) > 8 and modified != "Unknown", modified)
            check("b: no spinner for a file", not any(r["spinner"] for r in state["rows"]), str(state["rows"]))
            full = (row(state, "Full path") or {}).get("value", "")
            check("b: the full path is the fixture's file", full.replace("\\", "/").endswith("props-fixture/a.txt"), full)
            close_dialog(page)

            # -- c: a folder, with the count held back so the wait can be seen ---
            page.evaluate(
                "() => { const o = WBDaemon.observe; window.__observe0 = o; window.__release = null;"
                "  WBDaemon.observe = (verb, payload) => verb !== 'tree.size' ? o(verb, payload)"
                "    : new Promise(res => { window.__release = () => res(o(verb, payload)); }); }"
            )
            open_menu_on(page, "pack")
            click_menu_row(page, "Properties")
            wait_dialog(page)
            page.wait_for_function("() => typeof window.__release === 'function'", timeout=10000)
            # The facts arrive first, and the spinner of a row that was already
            # drawn for the file follows one frame later: wait, do not sample.
            page.wait_for_function(f"() => {PROPS}.props.stat !== null", timeout=10000)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.props-row')].filter(r => r.querySelector('dt').textContent.trim() === 'Size'"
                " || r.querySelector('dt').textContent.trim() === 'Contains')"
                ".every(r => { const s = r.querySelector('.files-spinner'); return s.offsetParent !== null && s.clientWidth > 0; })",
                timeout=5000,
            )
            waiting = page.evaluate(DIALOG)
            size = row(waiting, "Size")
            check("c: Size reads Calculating… with a spinner", size and size["value"] == "Calculating…" and size["spinner"], str(size))
            contains = row(waiting, "Contains")
            check("c: Contains reads Calculating… with a spinner", contains and contains["value"] == "Calculating…" and contains["spinner"], str(contains))
            busy = page.evaluate("() => document.querySelector('.properties-modal').getAttribute('aria-busy')")
            check("c: the dialog is marked busy", busy == "true", str(busy))
            check("c: the facts did not wait for the count", (row(waiting, "Modified") or {}).get("value") not in (None, "…"), str(waiting["rows"]))
            page.evaluate("() => { WBDaemon.observe = window.__observe0; window.__release(); }")
            done = settled(page)
            check("c: Size is 60 bytes", (row(done, "Size") or {}).get("value") == "60 bytes", str(row(done, "Size")))
            check("c: Contains 3 files, 1 folder", (row(done, "Contains") or {}).get("value") == "3 files, 1 folder", str(row(done, "Contains")))
            check("c: nothing ignored inside", (row(done, "Ignored contents") or {}).get("value") == "0 bytes", str(row(done, "Ignored contents")))
            check("c: no spinner left", not any(r["spinner"] for r in done["rows"]), str(done["rows"]))
            busy = page.evaluate("() => document.querySelector('.properties-modal').getAttribute('aria-busy')")
            check("c: the dialog is no longer busy", busy in (None, "false"), str(busy))
            page.screenshot(path=SHOT)
            close_dialog(page)

            # -- d: the project root --------------------------------------------
            open_menu_on(page, None)
            click_menu_row(page, "Properties")
            wait_dialog(page)
            root = settled(page)
            name = (row(root, "Name") or {}).get("value", "")
            check("d: the root is named for the project", name.endswith("(project root)"), name)
            check("d: the root has no relative path row", row(root, "Path") is None, str(root["rows"]))
            full = (row(root, "Full path") or {}).get("value", "")
            check("d: the full path is the fixture folder", full.replace("\\", "/").endswith("props-fixture"), full)
            ignored = (row(root, "Ignored contents") or {}).get("value", "")
            check("d: the ignored folder is counted as ignored", ignored.startswith("100 bytes"), ignored)
            close_dialog(page)

            # -- e: Alt+Enter, and Enter alone --------------------------------
            page.locator(".wb-host .wb-row", has_text="a.txt").first.click()
            page.keyboard.press("Alt+Enter")
            wait_dialog(page)
            check("e: Alt+Enter opens the dialog for the row", page.evaluate(f"() => {PROPS}.props.path") == "a.txt")
            close_dialog(page)
            page.locator(".wb-host .wb-row", has_text="a.txt").first.click()
            page.keyboard.press("Enter")
            page.wait_for_timeout(800)
            opened = page.evaluate(f"() => {PROPS}.props.open")
            check("e: Enter alone does not open the dialog", opened is False, str(opened))

            # -- f: Close, and a reply after the dialog was closed --------------
            open_menu_on(page, "pack")
            click_menu_row(page, "Properties")
            wait_dialog(page)
            page.evaluate(
                "() => { const o = WBDaemon.observe; window.__late = null;"
                "  WBDaemon.observe = (verb, payload) => verb !== 'tree.size' ? o(verb, payload)"
                "    : new Promise(res => { window.__late = () => res(o(verb, payload)); }); }"
            )
            page.evaluate("() => { const b = [...document.querySelectorAll('.properties-modal .modal-foot .btn')].find(x => x.textContent.trim() === 'Close'); b.click(); }")
            page.wait_for_function(f"() => {PROPS}.props.open === false", timeout=5000)
            check("f: Close ends the dialog", True)
            page.evaluate("() => window.__late && window.__late()")
            page.wait_for_timeout(500)
            late = page.evaluate(f"() => {PROPS}.props.size")
            check("f: a reply after Close changes nothing", late is None, str(late))

            check("no page error was thrown", not thrown, str(thrown))
            browser.close()
    finally:
        stop(proc)

    failed = results.count(False)
    print(f"\n{len(results) - failed}/{len(results)} checks passed", flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
