"""Browser acceptance: the FILES search narrows the tree and puts it back.

ADR-0036 amendment 2026-09-15. The tree is lazy, so a client-side filter can
only see loaded levels; the search asks the daemon (`tree.find` / `tree.grep`),
loads the ancestors of every hit, and narrows the tree with Wunderbaum's
filter. Nothing else is on screen — there is no results panel.

Scenario a  a NAME search narrows the tree to the hits and their ancestors,
            loading a level that was never expanded
Scenario b  Escape restores the tree to the operator's own expansion
Scenario c  a CONTENT search honours `.gitignore`, always searches `.ralphy/`,
            and shows the occurrence count on the row
Scenario d  opening a content hit lands the editor on the term
Scenario e  a `tree.dirty` reconcile under an active search keeps the level
            narrowed (the reloaded rows are re-marked, not blanked)
Scenario f  a word typed slowly is several searches in a row on a tree deep
            enough to scroll; the last one paints a full viewport, and erasing
            the query paints the whole tree back (the 2026-09-15 blank: a
            paint through Alpine's proxy of the tree left two rows at a stale
            offset and nothing else)

Boots a Localhost daemon on 7449 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_file_search.py   (exit 0 = all pass)
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
    empty = tempfile.mkdtemp(prefix="wbsearch_empty_")
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
    d = Path(tempfile.mkdtemp(prefix="wbsearch_repo_")) / "search-fixture"
    (d / "src" / "deep").mkdir(parents=True)
    (d / "build").mkdir()
    (d / ".ralphy").mkdir()
    (d / ".gitignore").write_text("build/\n.ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text("# fixture\n", encoding="utf-8")
    (d / "src" / "main.rs").write_text('fn main() { println!("needle one"); }\n', encoding="utf-8")
    (d / "src" / "deep" / "task_list.md").write_text("needle, NEEDLE.\n", encoding="utf-8")
    (d / "build" / "out.js").write_text("needle needle needle\n", encoding="utf-8")
    (d / ".ralphy" / "plan.md").write_text("# plan\nthe needle ahead\n", encoding="utf-8")
    # Deep enough that the unfiltered tree scrolls and a broad query hits the
    # 200 cap: 60 dirs of three files, two of which match "ta".
    for i in range(60):
        sub = d / "big" / f"d{i:02d}"
        sub.mkdir(parents=True)
        (sub / "task.md").write_text("nothing\n", encoding="utf-8")
        (sub / "data.yaml").write_text("nothing\n", encoding="utf-8")
        (sub / "other.txt").write_text("nothing\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbsearch@example.com")
    git(d, "config", "user.name", "wbsearch")
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
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


# Every LAID-OUT row title. A present-but-unlaid-out row is excluded, so a
# hidden (filtered) row never counts as on screen.
ROW_TITLES = (
    "() => [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)

# Laid-out rows with their badge, if any.
ROW_BADGES = (
    "() => Object.fromEntries([...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => [r.querySelector('.wb-title')?.textContent.trim(),"
    "             r.querySelector('.wb-hits')?.textContent.trim() ?? null]))"
)

NOTE_TEXT = (
    "() => { const els = [...document.querySelectorAll('.project.open .files-stale')]"
    "  .filter(el => el.offsetParent !== null);"
    "  return els.map(el => el.textContent.trim()).join(' | '); }"
)


def main():
    build()
    fixture = seed()
    daemon_dir = tempfile.mkdtemp(prefix="wbsearch_daemon_")
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
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'README.md')",
                timeout=15000,
            )
            before = page.evaluate(ROW_TITLES)
            check(
                "the tree opens collapsed: top level only",
                "src" in before and "big" in before and "task_list.md" not in before,
                f"rows={before}",
            )

            # --- scenario a: a NAME search narrows the tree -------------------
            page.keyboard.press("Control+Shift+F")
            page.wait_for_function(f"() => {SH}.fileSearch.open === true", timeout=5000)
            page.evaluate(f"() => {{ {SH}.fileSearch.query = 'task_l'; }}")
            page.evaluate(f"async () => await {SH}.fileSearchNow()")
            page.wait_for_function(
                f"() => ({ROW_TITLES})().includes('task_list.md')", timeout=10000
            )
            rows = page.evaluate(ROW_TITLES)
            check(
                "a name search shows the hit and its ancestors, nothing else",
                rows == ["src", "deep", "task_list.md"],
                f"rows={rows}",
            )
            page.screenshot(path=os.path.join(REPO_ROOT, "docs", "screenshots", "file-search-name.png"))

            # --- scenario b: Escape puts the tree back -------------------------
            page.evaluate(f"async () => await {SH}.closeFileSearch()")
            page.wait_for_function(
                f"() => JSON.stringify(({ROW_TITLES})()) === JSON.stringify({before!r})",
                timeout=10000,
            )
            after = page.evaluate(ROW_TITLES)
            check(
                "closing the search restores the operator's own expansion",
                after == before,
                f"before={before} after={after}",
            )
            check(
                "…and the remembered expansion never learned the search's expands",
                page.evaluate(f"() => ({SH}._treeExpanded?.get({SH}.openSlug) || []).length") == 0,
            )

            # --- scenario c: a CONTENT search honours gitignore, keeps .ralphy --
            page.evaluate(f"() => {SH}.openFileSearch()")
            page.evaluate(f"() => {{ {SH}.fileSearch.query = 'needle'; }}")
            page.evaluate(f"async () => await {SH}.setFileSearchMode('content')")
            page.wait_for_function(
                f"() => ({ROW_TITLES})().includes('plan.md')", timeout=10000
            )
            badges = page.evaluate(ROW_BADGES)
            rows = list(badges.keys())
            check(
                "a content search skips the ignored build/ and searches .ralphy/",
                "plan.md" in rows and "main.rs" in rows and "task_list.md" in rows and "out.js" not in rows,
                f"rows={rows}",
            )
            check(
                "…and each hit wears its occurrence count",
                badges.get("task_list.md") == "2" and badges.get("main.rs") == "1" and badges.get("plan.md") == "1",
                f"badges={badges}",
            )
            page.screenshot(path=os.path.join(REPO_ROOT, "docs", "screenshots", "file-search-content.png"))

            # --- scenario d: opening a content hit lands on the term ----------
            page.evaluate(
                f"() => {{ const n = window.Alpine.raw({SH}._tree).findFirst(x => x.title === 'main.rs'); {SH}.openFile(n); }}"
            )
            page.wait_for_function(
                "() => !!document.querySelector('.code-viewer .monaco-editor')", timeout=20000
            )
            page.wait_for_function(
                "() => { const w = document.querySelector('.code-viewer .find-widget');"
                "  return !!(w && w.classList.contains('visible')); }",
                timeout=10000,
            )
            seeded = page.evaluate(
                "() => document.querySelector('.code-viewer .find-widget textarea, .code-viewer .find-widget input')?.value"
            )
            check("the editor's find widget opens seeded with the term", seeded == "needle", f"seeded={seeded!r}")

            # --- scenario e: a reconcile under the filter keeps it narrowed ----
            (fixture / "src" / "extra.txt").write_text("nothing\n", encoding="utf-8")
            page.evaluate(f"async () => await {SH}.onTreeDirty('src')")
            time.sleep(0.3)
            rows = page.evaluate(ROW_TITLES)
            check(
                "a reloaded level under an active search stays narrowed",
                "main.rs" in rows and "extra.txt" not in rows,
                f"rows={rows}",
            )

            # --- scenario f: slow typing, then erasing, on a scrolling tree --
            page.evaluate(f"async () => await {SH}.closeFileSearch()")
            page.wait_for_function(f"() => !({SH}._tree.isFilterActive())", timeout=5000)
            page.evaluate(f"() => {SH}.openFileSearch()")
            page.evaluate(f"async () => await {SH}.setFileSearchMode('name')")
            page.click(".project.open .files-search input")
            for ch in "task":
                page.keyboard.type(ch)
                time.sleep(1.2)  # past the debounce: every letter from the 2nd is a search
            time.sleep(3)
            painted = page.evaluate(
                f"() => {{ const t = window.Alpine.raw({SH}._tree);"
                "  return { rows: t.nodeListElement.childNodes.length, total: t.treeRowCount,"
                "    fit: Math.floor(t.element.clientHeight / t.options.rowHeightPx) }; }"
            )
            check(
                "a search that follows a search paints a full viewport",
                painted["rows"] >= min(painted["total"], painted["fit"]) and painted["total"] > 60,
                f"painted={painted}",
            )
            for _ in range(4):
                page.keyboard.press("Backspace")
                time.sleep(0.4)
            page.wait_for_function(
                f"() => JSON.stringify(({ROW_TITLES})()) === JSON.stringify({before!r})",
                timeout=10000,
            )
            check("erasing the query paints the whole tree back", page.evaluate(ROW_TITLES) == before)

            check("no uncaught page errors", not thrown, f"thrown={thrown}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
