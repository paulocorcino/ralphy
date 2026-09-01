"""Browser acceptance: a file tree that cannot be re-read keeps its rows.

The failure this exists to forbid: with the WSL peer unreachable, every relayed
`tree.list` came back `{status:"error"}`, `fetchTreeLevel` resolved `[]`, and a
reconcile replaced the whole level with nothing. The FILES panel emptied itself,
a refresh sometimes repainted it, and a file the operator had just created never
appeared (2026-09-01). An empty array is a STATEMENT — "this directory has no
entries" — and a failed read is not entitled to make it.

Scenario a  a `tree.dirty` reconcile whose `tree.list` is refused leaves every
            row exactly where it was, and says so in the FILES gutter
Scenario b  the next read that succeeds clears the notice
Scenario c  a directory that REALLY lost an entry still loses it — the guard must
            not be "never remove rows"

Boots a Localhost daemon on 7448 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

The refusal is injected at `WBDaemon.observe`, the seam the tree reads through,
so this proves the TREE's handling rather than the daemon's — the daemon-side
half is covered by the Rust suite.

Run: python crates/ralphy-daemon/tests/wb_tree_stale_read.py   (exit 0 = all pass)
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

PORT = 7448
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
    empty = tempfile.mkdtemp(prefix="wbstale_empty_")
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
    d = Path(tempfile.mkdtemp(prefix="wbstale_repo_")) / "stale-fixture"
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "keep-me.txt").write_text("keep\n", encoding="utf-8")
    (d / "doomed.txt").write_text("doomed\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbstale@example.com")
    git(d, "config", "user.name", "wbstale")
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


# Every LAID-OUT row title. A present-but-unlaid-out row is excluded, so
# "keep-me.txt is on screen" cannot pass on a zero-height row.
ROW_TITLES = (
    "() => [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)

# The gutter notice, only if it is actually laid out.
STALE_TEXT = (
    "() => { const el = document.querySelector('.project.open .files-stale');"
    "  return el && el.offsetParent !== null ? el.textContent.trim() : ''; }"
)

REFUSE_TREE_LIST = """
() => {
  const d = window.WBDaemon;
  if (!d.__origObserve) d.__origObserve = d.observe;
  d.observe = (verb, payload) =>
    verb === 'tree.list'
      ? Promise.resolve({ status: 'error', message: 'the peer did not answer' })
      : d.__origObserve(verb, payload);
}
"""

# The notice, LAID OUT and carrying its text. `clientWidth > 0` keeps a
# zero-width element from passing this vacuously (CONTEXT.md).
STALE_VISIBLE = (
    "() => { const el = document.querySelector('.project.open .files-stale');"
    "  return !!(el && el.offsetParent !== null && el.clientWidth > 0"
    "    && el.textContent.includes('showing the last listing')); }"
)

RESTORE_OBSERVE = "() => { if (window.WBDaemon.__origObserve) window.WBDaemon.observe = window.WBDaemon.__origObserve; }"


def main():
    build()
    fixture = seed()
    daemon_dir = tempfile.mkdtemp(prefix="wbstale_daemon_")
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
                "r.querySelector('.wb-title')?.textContent.trim() === 'keep-me.txt')",
                timeout=15000,
            )
            before = page.evaluate(ROW_TITLES)

            # --- scenario a: a refused re-read keeps every row -----------------
            page.evaluate(REFUSE_TREE_LIST)
            page.evaluate(f"async () => await {SH}.onTreeDirty('')")
            after = page.evaluate(ROW_TITLES)
            check(
                "a refused reconcile leaves the rows exactly as they were",
                after == before and "keep-me.txt" in after,
                f"before={before} after={after}",
            )
            # Awaited, not sampled: Alpine flushes its effects in a microtask, so
            # reading the DOM in the same turn as the state change measures the
            # frame BEFORE the render and would fail vacuously.
            page.wait_for_function(STALE_VISIBLE, timeout=5000)
            notice = page.evaluate(STALE_TEXT)
            check(
                "…and the FILES gutter says the listing is unconfirmed",
                "showing the last listing" in notice and "the peer did not answer" in notice,
                f"notice={notice!r}",
            )

            # --- scenario b: a read that lands clears the notice ---------------
            page.evaluate(RESTORE_OBSERVE)
            page.evaluate(f"async () => await {SH}.onTreeDirty('')")
            cleared = True
            try:
                page.wait_for_function(f"() => ({STALE_TEXT})() === ''", timeout=5000)
            except Exception:
                cleared = False
            check(
                "a read that succeeds clears the notice",
                cleared,
                "notice={!r}".format(page.evaluate(STALE_TEXT)),
            )

            # --- scenario c: a real removal is still a removal -----------------
            # The guard must be "a FAILED read may not empty the level", not
            # "rows never go away" — otherwise the tree stops telling the truth
            # in the other direction.
            (fixture / "doomed.txt").unlink()
            page.evaluate(f"async () => await {SH}.onTreeDirty('')")
            titles = page.evaluate(ROW_TITLES)
            check(
                "a file that really went away still leaves the tree",
                "doomed.txt" not in titles and "keep-me.txt" in titles,
                f"titles={titles}",
            )

            check("no uncaught page errors", not thrown, f"thrown={thrown}")
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
