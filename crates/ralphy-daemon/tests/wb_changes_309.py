"""#309 browser acceptance: the Changes row list.

One Playwright pass over a REAL daemon proving the list renders one row per
changed path with a per-status marker, shows a rename's original path, includes
untracked files, scrolls a long change set, and stays usable on a narrow
viewport. #317 promoted the list out of the sidebar accordion into a rail view;
reaching it is a rail click now, and the count lives on the Projects row.

Scenario 1  the rail view renders exactly 5 rows — since #315, staged group
            first, git's own order within each group
Scenario 2  each row carries a distinct per-status marker + class
Scenario 3  a renamed path shows `← old.txt` and a rename title, no bare
            `old.txt` row
Scenario 4  untracked files appear in the list
Scenario 5  (deleted by #317 — the tree and the list no longer share the
            sidebar's height; they are separate views)
Scenario 6  a 390x844 viewport keeps every row visible with no horizontal
            bleed

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


# The count moved to the Projects row (#317); the list moved to the rail view.
VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('li.project.open .chg-badge'))"
    ".filter(e => e.offsetParent !== null)"
)
OPEN_VIEW = (
    "(() => { const v = document.querySelector('.changes-view');"
    " return v && v.offsetParent !== null ? v : null; })()"
)


def show_view(page, view):
    """Clicking the rail button of the view already showing COLLAPSES the
    sidebar, so every switch is guarded on that view's own visibility."""
    page.evaluate(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " if (!el || el.offsetParent === null)"
        "   document.querySelector('nav.rail button[title=\"' +"
        "     (v === 'changes' ? 'Changes' : 'Projects') + '\"]').click(); }",
        arg=view,
    )
    page.wait_for_function(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " return !!el && el.offsetParent !== null; }",
        arg=view,
        timeout=15000,
    )


def badge_text(page):
    return page.evaluate(
        f"() => {{ const els = {VISIBLE_SECS};"
        " if (els.length > 1) return 'MULTI';"
        " return els.length ? els[0].textContent.trim() : null; }"
    )


def wait_badge(page, expected, timeout=15000):
    show_view(page, "projects")
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


def open_project(page, slug, expected):
    show_view(page, "projects")
    # `toggle` is a TOGGLE: calling it on the already-open project closes it.
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    wait_badge(page, expected)


def rows_in_open_project(page):
    """Plain data per visible row — never a DOM handle across the JSON
    boundary, which Playwright's sync `evaluate` cannot round-trip."""
    return page.evaluate(
        f"() => {{ const li = {OPEN_VIEW}; if (!li) return [];"
        " return Array.from(li.querySelectorAll('.chg-row')).filter(r => r.offsetParent !== null)"
        ".map(r => { const mark = r.querySelector('.chg-mark'); const from = r.querySelector('.chg-from');"
        "   return { path: r.querySelector('.chg-name').textContent.trim(),"
        "            mark: mark.textContent.trim(), markClass: mark.className,"
        "            from: from ? from.textContent.trim() : null,"
        "            title: r.getAttribute('title') }; }); }"
    )


def wait_row_count(page, expected, timeout=8000):
    # Gate on VISIBLE rows (offsetParent), not the raw querySelectorAll count:
    # rows exist in the DOM as soon as the group maps load, well before the
    # x-show flip on `.changes-list` makes them visible — a plain count check
    # here would resolve before the flip, and the very next evaluate would
    # still read the pre-flip (invisible) state (handoffs.md #307).
    page.wait_for_function(
        f"(want) => {{ const li = {OPEN_VIEW};"
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
            check(
                "the Changes view is not what the sidebar shows on load",
                page.evaluate(f"() => !{OPEN_VIEW}"),
            )
            show_view(page, "changes")
            wait_row_count(page, 5)
            rows = rows_in_open_project(page)
            paths = [r["path"] for r in rows]
            check(
                "the rail view renders exactly 5 rows, staged group first, git's order within each",
                # #315 split the flat list in two: the three staged paths
                # (`A.`/`D.`/`R.`) come first, then the two worktree-only ones —
                # each group still in git's own emission order.
                paths == ["added.txt", "gone.txt", "new.txt", "README.md", "untracked.txt"],
                f"got={paths}",
            )

            # --- scenario 2: distinct per-status markers -----------------------
            marks = [r["mark"] for r in rows]
            classes = [r["markClass"] for r in rows]
            check(
                "each row carries its own distinct status marker",
                marks == ["A", "D", "R", "M", "U"] and len(set(marks)) == 5,
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

            # --- scenario 5 DELETED by #317 -------------------------------------
            # It asserted the file tree and the changes list share the sidebar's
            # height (tree floor, list below it, both scrolling). They are two
            # separate rail views now, so that geometry no longer exists; the rail
            # view's own budget is measured by wb_changes_317.py scenarios 6-7.
            open_project(page, slug_b, "60")
            show_view(page, "changes")
            wait_row_count(page, 60)
            check(
                "a 60-row change set scrolls inside the view",
                page.evaluate(
                    f"() => {{ const l = {OPEN_VIEW}.querySelector('.changes-list');"
                    " return l.scrollHeight > l.clientHeight; }"
                ),
            )

            # --- scenario 6: narrow viewport stays usable ------------------------
            page.set_viewport_size({"width": 390, "height": 844})
            page.wait_for_function(
                f"() => {{ const li = {OPEN_VIEW}; if (!li) return false;"
                " const list = li.querySelector('.changes-list');"
                " const head = li.querySelector('.side-head .count');"
                " const marks = Array.from(list.querySelectorAll('.chg-mark'));"
                " const paths = Array.from(list.querySelectorAll('.chg-name'));"
                " return head.offsetParent !== null"
                "   && marks.length > 0 && marks.every(m => m.offsetParent !== null)"
                "   && paths.length > 0 && paths.every(p => p.offsetParent !== null)"
                "   && list.scrollWidth === list.clientWidth; }",
                timeout=8000,
            )
            check("the view's count, marks and paths stay visible at 390x844 with no horizontal bleed", True)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # 12, the ACTUAL count: #317 deleted scenario 5's four tree-and-list
    # geometry checks and replaced them with one scroll check on the view.
    ok = all(results) and len(results) >= 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES LIST LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
