"""#309 browser acceptance: the Changes section's expandable row list.

One Playwright pass over a REAL daemon proving the list expands on a header
click, renders one row per changed path with a per-status marker, shows a
rename's original path, includes untracked files, scrolls without hiding the
tree, and stays usable on a narrow viewport.

Scenario 1  collapsed on load; a header click renders exactly 5 rows in git's
            own order
Scenario 2  each row carries a distinct per-status marker + class
Scenario 3  a renamed path shows `← old.txt` and a rename title, no bare
            `old.txt` row
Scenario 4  untracked files appear in the list
Scenario 5  a 60-file fixture: both the tree and the list scroll; the tree
            keeps its floor
Scenario 6  a 390x844 viewport keeps the badge and every row visible with no
            horizontal bleed

Boots a Localhost daemon on 7409 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/309-changes-list-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_309.py   (exit 0 = all pass)
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

PORT = 7409
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_309.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb309_empty_")
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


def make_fixture_repo(tag, dirt):
    """A committed git repo, then `dirt` applied on top. `.gitignore` hides
    `.ralphy/`, so the fixture's change count is exactly what `dirt` made."""
    d = tempfile.mkdtemp(prefix=f"wb309_{tag}_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #309 changes fixture repo.\n", encoding="utf-8")
    (p / "tracked.txt").write_text("committed\n", encoding="utf-8")
    (p / "old.txt").write_text("original\n", encoding="utf-8")
    (p / "gone.txt").write_text("doomed\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb309@example.com"],
        ["git", "config", "user.name", "wb309"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    dirt(p, d)
    return d


def five_changes(p, d):
    # Mirrors changes.rs:146-161's dirt shapes: rewrite a tracked file, stage a
    # new one, `git rm` a tracked one, `git mv` a tracked one (rename is only
    # recorded when staged), and drop one loose file.
    (p / "README.md").write_text("# edited\n", encoding="utf-8")  # modified
    (p / "added.txt").write_text("staged\n", encoding="utf-8")  # added
    subprocess.run(["git", "add", "added.txt"], cwd=d, check=True, capture_output=True)
    subprocess.run(["git", "rm", "-q", "gone.txt"], cwd=d, check=True, capture_output=True)  # deleted
    subprocess.run(["git", "mv", "old.txt", "new.txt"], cwd=d, check=True, capture_output=True)  # renamed
    (p / "untracked.txt").write_text("loose\n", encoding="utf-8")  # untracked


def sixty_untracked(p, d):
    for i in range(60):
        (p / f"loose-{i:02d}.txt").write_text(f"loose {i}\n", encoding="utf-8")


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


VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('.changes-sec')).filter(e => e.offsetParent !== null)"
)


def badge_text(page):
    return page.evaluate(
        f"() => {{ const els = {VISIBLE_SECS};"
        " if (els.length > 1) return 'MULTI';"
        " return els.length ? els[0].querySelector('.count').textContent.trim() : null; }"
    )


def wait_badge(page, expected, timeout=15000):
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].querySelector('.count').textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


def open_project(page, slug, expected):
    page.evaluate(f"() => {SH}.toggle('{slug}')")
    wait_badge(page, expected)


def rows_in_open_project(page):
    """Plain data per visible row — never a DOM handle across the JSON
    boundary, which Playwright's sync `evaluate` cannot round-trip."""
    return page.evaluate(
        "() => { const li = Array.from(document.querySelectorAll('li.project'))"
        ".find(e => e.querySelector('.changes-sec') && e.querySelector('.changes-sec').offsetParent !== null);"
        " return Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null)"
        ".map(r => { const mark = r.querySelector('.chg-mark'); const from = r.querySelector('.chg-from');"
        "   return { path: r.querySelector('.chg-path').textContent.trim(),"
        "            mark: mark.textContent.trim(), markClass: mark.className,"
        "            from: from ? from.textContent.trim() : null,"
        "            title: r.getAttribute('title') }; }); }"
    )


def wait_row_count(page, expected, timeout=8000):
    # Gate on VISIBLE rows (offsetParent), not the raw querySelectorAll count:
    # rows exist in the DOM as soon as changesEntries loads, well before the
    # x-show flip on `.changes-list` makes them visible — a plain count check
    # here would resolve before the flip, and the very next evaluate would
    # still read the pre-flip (invisible) state (handoffs.md #307).
    page.wait_for_function(
        "(want) => { const li = Array.from(document.querySelectorAll('li.project'))"
        ".find(e => e.querySelector('.changes-sec') && e.querySelector('.changes-sec').offsetParent !== null);"
        " return !!li && Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null).length === want; }",
        arg=expected,
        timeout=timeout,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb309_reg_")
    dir_a = make_fixture_repo("five", five_changes)
    dir_b = make_fixture_repo("sixty", sixty_untracked)
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)

            # --- scenario 1: collapsed on load; a click renders 5 rows --------
            open_project(page, slug_a, "5")
            check("no rows rendered while collapsed", len(rows_in_open_project(page)) == 0)
            page.click(".changes-sec:visible")
            wait_row_count(page, 5)
            rows = rows_in_open_project(page)
            paths = [r["path"] for r in rows]
            check(
                "one click renders exactly 5 rows in git's own order",
                paths == ["README.md", "added.txt", "gone.txt", "new.txt", "untracked.txt"],
                f"got={paths}",
            )

            # --- scenario 2: distinct per-status markers -----------------------
            marks = [r["mark"] for r in rows]
            classes = [r["markClass"] for r in rows]
            check(
                "each row carries its own distinct status marker",
                marks == ["M", "A", "D", "R", "U"] and len(set(marks)) == 5,
                f"marks={marks}",
            )
            check(
                "each mark carries its own st-* class",
                all(c and "chg-mark" in c and any(f"st-{s}" in c for s in ("modified", "added", "deleted", "renamed", "untracked")) for c in classes),
                f"classes={classes}",
            )

            # --- scenario 3: rename shows the original path ---------------------
            rename_row = next(r for r in rows if r["path"] == "new.txt")
            check("the renamed row shows ← old.txt", rename_row["from"] == "← old.txt", f"got={rename_row['from']!r}")
            check(
                "the renamed row's title is old.txt → new.txt",
                rename_row["title"] == "old.txt → new.txt",
                f"got={rename_row['title']!r}",
            )
            check("no separate old.txt row exists", "old.txt" not in paths, f"paths={paths}")

            # --- scenario 4: untracked files appear ------------------------------
            untracked_row = next((r for r in rows if r["path"] == "untracked.txt"), None)
            check("untracked.txt is present", untracked_row is not None)
            if untracked_row is not None:
                check("untracked.txt carries mark U", untracked_row["mark"] == "U", f"got={untracked_row['mark']!r}")

            page.screenshot(path=os.path.join(SHOT_DIR, "309-changes-list-2026-07-25.png"))

            # --- scenario 5: the tree and list both scroll on a long list -------
            open_project(page, slug_b, "60")
            page.click(".changes-sec:visible")
            wait_row_count(page, 60)
            geom = page.evaluate(
                "() => { const li = Array.from(document.querySelectorAll('li.project'))"
                ".find(e => e.querySelector('.changes-sec') && e.querySelector('.changes-sec').offsetParent !== null);"
                " const host = li.querySelector('.wb-host'); const list = li.querySelector('.changes-list');"
                " return { hostH: host.getBoundingClientRect().height, hostTop: host.getBoundingClientRect().top,"
                "          listTop: list.getBoundingClientRect().top,"
                "          hostScrolls: host.scrollHeight > host.clientHeight,"
                "          listScrolls: list.scrollHeight > list.clientHeight }; }"
            )
            check("the file tree keeps its 120px floor", geom["hostH"] >= 120, f"got={geom['hostH']}")
            check("the list sits below the tree", geom["listTop"] > geom["hostTop"], f"geom={geom}")
            check("the file tree scrolls", geom["hostScrolls"], f"geom={geom}")
            check("the changes list scrolls", geom["listScrolls"], f"geom={geom}")

            # --- scenario 6: narrow viewport stays usable ------------------------
            page.set_viewport_size({"width": 390, "height": 844})
            page.wait_for_function(
                "() => { const li = Array.from(document.querySelectorAll('li.project'))"
                ".find(e => e.querySelector('.changes-sec') && e.querySelector('.changes-sec').offsetParent !== null);"
                " const sec = li.querySelector('.changes-sec'); const list = li.querySelector('.changes-list');"
                " const marks = Array.from(list.querySelectorAll('.chg-mark'));"
                " const paths = Array.from(list.querySelectorAll('.chg-path'));"
                " return sec.querySelector('.count').offsetParent !== null"
                "   && marks.length > 0 && marks.every(m => m.offsetParent !== null)"
                "   && paths.length > 0 && paths.every(p => p.offsetParent !== null)"
                "   && list.scrollWidth === list.clientWidth; }",
                timeout=8000,
            )
            check("the badge, marks and paths stay visible at 390x844 with no horizontal bleed", True)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES LIST LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
