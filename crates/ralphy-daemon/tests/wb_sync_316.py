"""#316 browser acceptance: the sync row — counts, staleness, fetch and pull.

One Playwright pass over a REAL daemon proving the branch's ahead/behind reach
the sidebar, that NOTHING fetches until the operator clicks, that the click then
moves the counts, that a fast-forward pull absorbs the upstream, and that the two
stateless cases render as their own words rather than as zeroed counts.

Scenario a  a tracking clone reads `↑0 ↓0` + `never fetched`, and `.git/FETCH_HEAD`
            is still ABSENT 3s after the panel opened and after a sidebar refresh
Scenario b  clicking Fetch creates FETCH_HEAD, the row becomes `↑0 ↓2` and the
            staleness label starts with `fetched `
Scenario c  clicking Pull returns the row to `↑0 ↓0` and lands the remote's file
Scenario d  a repo with no upstream reads `no upstream` and never `↓`
Scenario e  a detached HEAD reads `detached HEAD` and the page throws nothing

Every "remote" is a LOCAL directory cloned by path — no network.

Boots a Localhost daemon on 7416 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/316-sync-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_sync_316.py   (exit 0 = all pass)
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

PORT = 7416
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_sync_316.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wb316_empty_")
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


def seed(d, tag):
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #316 sync fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb316@example.com")
    git(d, "config", "user.name", "wb316")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return p


def make_tracking():
    """A clone two commits BEHIND its (local-path) remote, never fetched."""
    remote = tempfile.mkdtemp(prefix="wb316_remote_")
    rp = seed(remote, "remote")
    clone = tempfile.mkdtemp(prefix="wb316_tracking_")
    # Clone into an existing empty dir by PATH — no network in this suite.
    git(clone, "clone", "--quiet", remote, clone)
    git(clone, "config", "user.email", "wb316@example.com")
    git(clone, "config", "user.name", "wb316")
    for name in ("first.txt", "second.txt"):
        (rp / name).write_text(f"{name}\n", encoding="utf-8")
        git(remote, "add", "-A")
        git(remote, "commit", "-m", f"add {name}")
    return clone


def make_no_upstream():
    d = tempfile.mkdtemp(prefix="wb316_noup_")
    seed(d, "noup")
    return d


def make_detached():
    d = tempfile.mkdtemp(prefix="wb316_detached_")
    p = seed(d, "detached")
    (p / "second.txt").write_text("second\n", encoding="utf-8")
    git(d, "add", "-A")
    git(d, "commit", "-m", "second")
    git(d, "checkout", "--quiet", "--detach", "HEAD~1")
    return d


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


# The Changes surface, which #317 promoted out of the project accordion into a
# rail VIEW of the sidebar. It is scoped to the open project by construction, so
# there is one of it — but it is still `x-show`-gated, hence the offsetParent
# check that every downstream wait depends on.
OPEN_LI = (
    "(() => { const v = document.querySelector('.changes-view');"
    " return v && v.offsetParent !== null ? v : null; })()"
)

# The OPEN project's sync row as plain data — Playwright's sync `evaluate`
# cannot round-trip a DOM handle, so every span is read inside this one
# expression.
ROW_EXPR = (
    "(() => {"
    f" const li = {OPEN_LI}; if (!li) return null;"
    " const row = li.querySelector('.sync-row');"
    " if (!row || row.offsetParent === null) return null;"
    " const t = (sel) => { const e = row.querySelector(sel);"
    "   return e && e.offsetParent !== null ? e.textContent.trim() : ''; };"
    " return { branch: t('.sync-branch'), counts: t('.sync-counts'),"
    "          note: t('.sync-note'), fetched: t('.sync-fetched'),"
    "          text: row.innerText.trim() };"
    " })()"
)


def sync_row(page):
    return page.evaluate(f"() => {ROW_EXPR}")


def wait_sync(page, field, want, timeout=15000):
    # An Alpine x-show/x-text flip is NOT visible to the very next evaluate, so
    # every assertion polls rather than reading once (KNOWLEDGE.md #307/#309).
    page.wait_for_function(
        f"(arg) => {{ const r = {ROW_EXPR}; return !!r && r[arg.field] === arg.want; }}",
        arg={"field": field, "want": want},
        timeout=timeout,
    )


def open_and_expand(page, slug):
    # The slug rides as an ARGUMENT, never interpolated into the source: a clone
    # of a local path is registered under a slug carrying Windows backslashes,
    # which a string literal would swallow as escapes and silently open nothing.
    # `toggle` is a TOGGLE — calling it on the already-open project closes it.
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    # Reaching the change set is now a rail click, not an accordion expand (#317);
    # clicking the rail button of the view already showing would COLLAPSE the
    # sidebar, so this is guarded on the view's own visibility.
    page.evaluate(
        "() => { const v = document.querySelector('.changes-view');"
        " if (!v || v.offsetParent === null)"
        "   document.querySelector('nav.rail button[title=\"Changes\"]').click(); }"
    )
    page.wait_for_function(
        f"() => {{ const li = {OPEN_LI}; if (!li) return false;"
        " const row = li.querySelector('.sync-row');"
        " return !!row && row.offsetParent !== null; }",
        timeout=15000,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb316_reg_")
    dir_tracking = make_tracking()
    dir_noup = make_no_upstream()
    dir_detached = make_detached()
    slug_tracking = register_fixture(daemon_dir, dir_tracking)
    slug_noup = register_fixture(daemon_dir, dir_noup)
    slug_detached = register_fixture(daemon_dir, dir_detached)
    fetch_head = Path(dir_tracking) / ".git" / "FETCH_HEAD"

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 3", timeout=15000)

            # --- scenario a: the counts arrive, and NOTHING fetched ------------
            check(
                "the fixture starts with no FETCH_HEAD (a clone leaves none)",
                not fetch_head.exists(),
            )
            open_and_expand(page, slug_tracking)
            wait_sync(page, "counts", "↑0 ↓0")
            row = sync_row(page)
            check(
                "a tracking branch reads its stale counts",
                row and row["counts"] == "↑0 ↓0",
                f"got={row}",
            )
            check(
                "…labelled with how stale they are",
                row and row["fetched"] == "never fetched",
                f"got={row and row['fetched']!r}",
            )
            check("…and names the branch", row and row["branch"] == "main", f"got={row}")

            # The negative control for "no fetch on a timer or on panel open":
            # sit on the open panel, then force the sidebar refresh path too.
            page.wait_for_timeout(3000)
            page.evaluate(f"() => {SH}.loadRepos()")
            page.wait_for_timeout(1500)
            check(
                "no fetch happens on panel open, on a refresh, or on a timer",
                not fetch_head.exists(),
                f"FETCH_HEAD={fetch_head}",
            )
            check(
                "…and the counts are still the stale ones",
                (sync_row(page) or {}).get("counts") == "↑0 ↓0",
                f"got={sync_row(page)}",
            )

            # --- scenario b: the operator's own fetch --------------------------
            page.click('[data-act="fetch"]:visible')
            wait_sync(page, "counts", "↑0 ↓2")
            after = sync_row(page)
            check(
                "clicking Fetch really fetched (FETCH_HEAD exists)",
                fetch_head.exists(),
                f"FETCH_HEAD={fetch_head}",
            )
            check(
                "…and the row now reads the two commits it is behind",
                after and after["counts"] == "↑0 ↓2",
                f"got={after}",
            )
            check(
                "…and the staleness label became a real stamp",
                after and after["fetched"].startswith("fetched "),
                f"got={after and after['fetched']!r}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "316-sync-2026-07-25.png"))

            # --- scenario c: a fast-forward pull -------------------------------
            landed = Path(dir_tracking) / "second.txt"
            check("the upstream's file is not on disk before the pull", not landed.exists())
            page.click('[data-act="pull"]:visible')
            wait_sync(page, "counts", "↑0 ↓0")
            check(
                "clicking Pull fast-forwards the branch back to level",
                (sync_row(page) or {}).get("counts") == "↑0 ↓0",
                f"got={sync_row(page)}",
            )
            check(
                "…and the upstream's commits are on disk",
                landed.exists(),
                f"{landed} exists={landed.exists()}",
            )

            # --- scenario d: no upstream is its own state ----------------------
            open_and_expand(page, slug_noup)
            wait_sync(page, "note", "no upstream")
            noup = sync_row(page)
            check(
                "a branch with no upstream says so",
                noup and "no upstream" in noup["text"],
                f"got={noup}",
            )
            check(
                "…and never renders as zeroed counts",
                noup and "↓" not in noup["text"] and noup["counts"] == "",
                f"got={noup}",
            )

            # --- scenario e: a detached HEAD is a state, not an error ----------
            open_and_expand(page, slug_detached)
            wait_sync(page, "note", "detached HEAD")
            det = sync_row(page)
            check(
                "a detached HEAD says so",
                det and "detached HEAD" in det["text"],
                f"got={det}",
            )
            check("…with no counts", det and det["counts"] == "", f"got={det}")
            check("…and the panel threw nothing", not thrown, f"pageerrors={thrown}")

            info("fixtures", f"tracking={dir_tracking} noup={dir_noup} detached={dir_detached}")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 15
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("SYNC ROW LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
