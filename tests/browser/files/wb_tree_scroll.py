"""Browser acceptance: a `tree.dirty` nudge keeps the file tree's scroll position.

The failure this exists to forbid: with the tree scrolled to the bottom, every
nudge for the root tore the level down and rebuilt it. The teardown shrank the
list, the browser clamped `scrollTop`, and the re-activation of the selected
row scrolled to that row. The scroll bar jumped up each time something wrote a
file in a visible directory (2026-10-01).

Scenario a  a nudge whose listing matches the rows on screen keeps the same row
            objects (no rebuild) and the same scroll offset
Scenario b  a nudge that really adds an entry shows it, keeps the selection,
            and keeps the scroll offset
Scenario c  a real write to a listed file, through the daemon's watcher, does
            not move the scroll bar

Boots a Localhost daemon on 7449 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Run: python tests/browser/files/wb_tree_scroll.py   (exit 0 = all pass)
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

PORT = 7449
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
)
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
TREE = f"{SH}.rawTree()"
FILES = 80

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
    empty = tempfile.mkdtemp(prefix="wbscroll_empty_")
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
    d = Path(tempfile.mkdtemp(prefix="wbscroll_repo_")) / "scroll-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    # The daemon creates `.ralphy/` after the project opens; made here, the
    # root listing of scenario a does not change under it.
    (d / ".ralphy").mkdir()
    (d / ".ralphy" / "keep").write_text("", encoding="utf-8")
    # Enough rows that the root level alone scrolls in a 900px viewport.
    for i in range(FILES):
        (d / f"f{i:03}.txt").write_text(f"{i}\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbscroll@example.com")
    git(d, "config", "user.name", "wbscroll")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", str(fixture_dir)],
        env=env,
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # yesterday's app.js and the whole run is vacuous.
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


SCROLL_TOP = f"() => {TREE}.element.scrollTop"

# Every LAID-OUT row title, so a zero-height row cannot pass.
ROW_TITLES = (
    "() => [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)


def main():
    build()
    fixture = seed()
    daemon_dir = tempfile.mkdtemp(prefix="wbscroll_daemon_")
    slug = register_fixture(daemon_dir, fixture)
    proc = subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
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

            page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)
            page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
            page.wait_for_function(f"() => ({ROW_TITLES})().includes('f001.txt')", timeout=15000)

            # Select a row near the TOP, then scroll to the bottom: a restore
            # of the selection that scrolls to it is half of the bug.
            page.evaluate(f"async () => await {SH}.revealRel('f001.txt')")
            page.evaluate(f"() => {{ const el = {TREE}.element; el.scrollTop = el.scrollHeight; }}")
            page.wait_for_function(f"() => ({ROW_TITLES})().includes('f{FILES - 1:03}.txt')", timeout=5000)
            bottom = page.evaluate(SCROLL_TOP)
            check("the tree scrolls", bottom > 100, f"scrollTop={bottom}")

            # --- scenario a: an unchanged listing rebuilds nothing -------------
            page.evaluate(f"() => {{ window.__firstRow = {TREE}.root.children[0]; }}")
            page.evaluate(f"async () => await {SH}.onTreeDirty('')")
            same_rows = page.evaluate(f"() => {TREE}.root.children[0] === window.__firstRow")
            top_a = page.evaluate(SCROLL_TOP)
            check("an unchanged listing keeps the same rows", same_rows)
            check("…and the same scroll offset", top_a == bottom, f"before={bottom} after={top_a}")

            # --- scenario b: a real change keeps the offset and selection ------
            (fixture / "g-new.txt").write_text("new\n", encoding="utf-8")
            page.evaluate(f"async () => await {SH}.onTreeDirty('')")
            titles = [n for n in page.evaluate(f"() => {TREE}.root.children.map(n => n.title)")]
            top_b = page.evaluate(SCROLL_TOP)
            active = page.evaluate(f"() => {SH}.relPath({TREE}.getActiveNode())")
            check("a new entry is shown", "g-new.txt" in titles, f"titles[-3:]={titles[-3:]}")
            check("…the scroll offset stays", abs(top_b - bottom) <= 1, f"before={bottom} after={top_b}")
            check("…and the selection stays", active == "f001.txt", f"active={active!r}")

            # --- scenario c: the real watcher path -----------------------------
            before_c = page.evaluate(SCROLL_TOP)
            page.evaluate(f"() => {{ window.__firstRow = {TREE}.root.children[0]; }}")
            with open(fixture / f"f{FILES - 1:03}.txt", "a", encoding="utf-8") as fh:
                fh.write("more\n")
            # The watcher debounces 300 ms; give the nudge time to arrive.
            time.sleep(2.5)
            top_c = page.evaluate(SCROLL_TOP)
            same_c = page.evaluate(f"() => {TREE}.root.children[0] === window.__firstRow")
            check("a write to a listed file does not move the scroll bar", top_c == before_c, f"before={before_c} after={top_c}")
            check("…and rebuilds no row", same_c)

            check("no uncaught page errors", not thrown, f"thrown={thrown}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
