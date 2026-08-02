"""Browser acceptance: re-opening a project costs neither the reads nor the wait.

One Playwright pass over a REAL daemon proving the four changes that made opening
a project cheap, each asserted by its CONSEQUENCE rather than by its mechanism:

Scenario a  opening a project reads `changes.list` and `sync.status` ONCE each.
            Both spawn the `ralphy` CLI, which spawns `git`, and both used to run
            TWICE — `toggle()` issued them and the changes subscription's `onopen`
            synthesized a catch-up frame that issued them again. This is the
            dominant cost of the click, so the count IS the fix
Scenario b  the FILES panel says it is working: with the `tree.list` reply held
            back, the spinner is laid out during the read and gone after it. The
            panel used to render a blank box that was indistinguishable from an
            empty repository
Scenario c  re-opening a project paints from memory. The tree is torn down on
            close, so this is proven the only way that cannot pass by being fast:
            the daemon is STOPPED, and the rows still appear — bytes no longer on
            any socket can only have come from the level cache. The folder that
            was expanded before the close is expanded again, and the panel reports
            no error, because a cached level that failed to revalidate is stale,
            not broken
Scenario d  …and the cache is not a blindfold: a project that was NEVER opened,
            opened against that same dead daemon, says so. An unreadable tree and
            an empty one must not look alike — the defect scenario c's mechanism
            would otherwise introduce

The revalidation that scenario c leaves failing is the reason the cache is honest:
a cached level is re-read against the disk once per open, and the re-read touches
the DOM only when the directory actually changed. `tree.list` itself still reads
the disk fresh on every request — nothing is cached on the daemon (ADR-0036).

Two fixture repos are registered: one is opened and re-opened, the second is the
never-opened project scenario d needs.

Boots a Localhost daemon on 7447 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every row assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap).

Writes docs/screenshots/reopen-cache-2026-08-02.png.
Run: python crates/ralphy-daemon/tests/wb_reopen.py   (exit 0 = all pass)
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

PORT = 7447
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_reopen.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "reopen-cache-2026-08-02.png")
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


def wait_gone(base, timeout=15):
    """The inverse: scenario c is worthless unless the daemon is really down."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            time.sleep(0.3)
        except Exception:
            return True
    return False


def stop(proc):
    if proc is None or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def empty_env(daemon_dir):
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wbreopen_empty_")
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


def seed(name):
    """A committed git repo with a top-level file and a `deep/` directory. `deep`
    starts collapsed; scenario c expands it, closes the project, and expects it
    back."""
    d = Path(tempfile.mkdtemp(prefix="wbreopen_repo_")) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    (d / "deep").mkdir()
    (d / "deep" / "inner.txt").write_text("inner\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbreopen@example.com")
    git(d, "config", "user.name", "wbreopen")
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
    # after any assets/ui edit or the browser loads yesterday's workbench.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# Count every read by verb, and optionally HOLD BACK the reply to `tree.list`.
# Wrapping `WBDaemon.observe` rather than the WebSocket keeps the subject at the
# level the fix lives: how many reads the UI issues, not how they are framed. The
# delay is what makes the spinner observable — against a loopback daemon the read
# lands within a frame, and a scenario that has to win a race is not a test.
INSTRUMENT = """
() => {
  window.__reads = {};
  window.__delayMs = 0;
  const real = window.WBDaemon.observe;
  window.WBDaemon.observe = function (verb, payload) {
    window.__reads[verb] = (window.__reads[verb] || 0) + 1;
    const p = real.call(window.WBDaemon, verb, payload);
    if (verb !== "tree.list" || !window.__delayMs) return p;
    return p.then((r) => new Promise((res) => setTimeout(() => res(r), window.__delayMs)));
  };
}
"""

# Every LAID-OUT tree row's title. A row that is present but not laid out is
# excluded, so "the tree came back" cannot pass on a zero-height row.
ROW_TITLES = (
    "() => [...document.querySelectorAll('.wb-host .wb-row')]"
    "  .filter(r => r.offsetParent !== null && r.clientWidth > 0)"
    "  .map(r => r.querySelector('.wb-title')?.textContent.trim())"
)

# The spinner and the error line, measured the same way. Scoped to `.project.open`
# because both live INSIDE each project row: an unscoped selector measures the
# first row in the sidebar, which is usually a closed one, and reports the open
# project's panel as invisible.
LAID = """
(sel) => {
  const el = document.querySelector(sel);
  if (!el) return { present: false, laid: false, text: "" };
  return {
    present: true,
    laid: el.offsetParent !== null && el.clientWidth > 0,
    text: (el.textContent || "").trim(),
  };
}
"""


def reads(page):
    return page.evaluate("() => ({...window.__reads})")


def reset_reads(page):
    page.evaluate("() => { window.__reads = {}; }")


def toggle(page, slug):
    page.evaluate(f"(s) => {SH}.toggle(s)", arg=slug)


def expand(page, slug, rel):
    """Expand a folder through the tree's own API and wait for its children."""
    page.evaluate(
        "async (rel) => { const c = " + SH + ";"
        "  const n = c._tree.findFirst(x => c.relPath(x) === rel);"
        "  if (n) await n.setExpanded(true); }",
        arg=rel,
    )


def main():
    if not os.path.exists(EXE):
        print(f"[FAIL] {EXE} not built", flush=True)
        sys.exit(1)
    build()
    os.makedirs(SHOT_DIR, exist_ok=True)
    daemon_dir = tempfile.mkdtemp(prefix="wbreopen_daemon_")
    fixture_a = seed("reopen-fixture")
    fixture_b = seed("never-opened")
    slug_a = register_fixture(daemon_dir, fixture_a)
    slug_b = register_fixture(daemon_dir, fixture_b)

    proc = launch(daemon_dir)
    if not wait_listening(BASE):
        check("daemon listening", False)
        stop(proc)
        sys.exit(1)
    check("daemon listening", True)

    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_function(f"() => !!{SH} && {SH}.projects.length >= 2", timeout=20000)
            page.evaluate(INSTRUMENT)

            # --- scenario a: one read each, not two ---------------------------
            reset_reads(page)
            toggle(page, slug_a)
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')]"
                "  .some(r => r.offsetParent !== null && r.clientWidth > 0)",
                timeout=20000,
            )
            # The subscription's catch-up (the read this fix removed) lands a beat
            # after `onopen`, so sampling immediately could pass by being early
            # rather than by being right. Give the socket time to have done the
            # wrong thing.
            page.wait_for_timeout(3000)
            got = reads(page)
            check(
                "opening a project reads changes.list ONCE, not twice",
                got.get("changes.list") == 1,
                f"count={got.get('changes.list')}",
            )
            check(
                "…and sync.status ONCE, not twice",
                got.get("sync.status") == 1,
                f"count={got.get('sync.status')}",
            )
            # …and the reads really happened — a count of 1 must not be a count of
            # 0 that the fix broke into silence.
            check(
                "the project's tree really loaded (the counts are not silence)",
                "a.txt" in page.evaluate(ROW_TITLES),
                "titles={}".format(page.evaluate(ROW_TITLES)),
            )

            # --- scenario b: the panel says it is working ---------------------
            # Close, clear the cache, and re-open with the reply held back: this is
            # the first-read path, the only one that waits.
            toggle(page, slug_a)
            page.evaluate(f"() => {SH}._treeCache.clear()")
            page.evaluate("() => { window.__delayMs = 1500; }")
            toggle(page, slug_a)
            page.wait_for_function(f"() => {SH}.treeLoading === true", timeout=10000)
            spin = page.evaluate(LAID, ".project.open .files-spinner")
            check(
                "the FILES panel shows a laid-out spinner while the read is in flight",
                spin["laid"],
                f"spinner={spin}",
            )
            page.wait_for_function(f"() => {SH}.treeLoading === false", timeout=20000)
            spin = page.evaluate(LAID, ".project.open .files-spinner")
            check("…and the spinner is gone once the tree is there", not spin["laid"], f"spinner={spin}")
            page.evaluate("() => { window.__delayMs = 0; }")

            # --- scenario c: re-opening paints from memory --------------------
            expand(page, slug_a, "deep")
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')].some("
                "r => r.offsetParent !== null && r.clientWidth > 0 && "
                "r.querySelector('.wb-title')?.textContent.trim() === 'inner.txt')",
                timeout=20000,
            )
            check("a folder expanded before the close shows its children", True)
            toggle(page, slug_a)  # close
            page.wait_for_function(f"() => {SH}.openSlug === null", timeout=10000)

            # Stop the daemon. From here nothing can be read: rows that appear were
            # painted from the browser's own memory, which is the whole claim.
            stop(proc)
            check("the daemon is really down", wait_gone(BASE))

            reset_reads(page)
            toggle(page, slug_a)  # re-open, against nothing
            page.wait_for_function(
                "() => [...document.querySelectorAll('.wb-host .wb-row')]"
                "  .some(r => r.offsetParent !== null && r.clientWidth > 0)",
                timeout=15000,
            )
            titles = page.evaluate(ROW_TITLES)
            check(
                "re-opening paints the tree with the daemon STOPPED — from the cache",
                "a.txt" in titles,
                "titles={}".format(titles),
            )
            check(
                "…and the folder left expanded is expanded again, from the same memory",
                "inner.txt" in titles,
                "titles={}".format(titles),
            )
            check(
                "…and the panel reports no error: a level that failed to revalidate is stale, not broken",
                page.evaluate(f"() => {SH}.treeError") == "",
                "error={}".format(page.evaluate(f"() => {SH}.treeError")),
            )
            # The revalidation was ATTEMPTED — otherwise the cache would be a
            # blindfold rather than a head start.
            check(
                "…and it did try to re-read the level against the disk",
                reads(page).get("tree.list", 0) >= 1,
                "reads={}".format(reads(page)),
            )
            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # --- scenario d: an unreadable tree is not an empty one -----------
            toggle(page, slug_a)  # close A
            toggle(page, slug_b)  # open a project that was NEVER opened
            page.wait_for_function(f"() => {SH}.treeError !== ''", timeout=25000)
            err = page.evaluate(LAID, ".project.open .files-error")
            check(
                "a project with nothing cached, opened against a dead daemon, says the read failed",
                err["laid"] and "could not read" in err["text"],
                f"error={err}",
            )
            check(
                "…and it renders no rows rather than another project's",
                page.evaluate(ROW_TITLES) == [],
                "titles={}".format(page.evaluate(ROW_TITLES)),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 15
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
