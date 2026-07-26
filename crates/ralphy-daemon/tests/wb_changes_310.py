"""#310 browser acceptance: the Changes count invalidates when a run finishes.

One Playwright pass over a REAL daemon proving the count refreshes itself when a
daemon-spawned run child exits — and ONLY then (no poll, no repo-wide watch).

Scenario 1  opening project A shows its Changes count (literal `1`)
Scenario 2  a file written from OUTSIDE the browser leaves the badge at `1` for
            a 3s window AND issues no `changes.list` read — no polling timer of
            any period, no repo-wide filesystem watch
Scenario 3  a daemon-spawned run in A exits and the badge refreshes `1` -> `2`
            with NO click on `.side-refresh` and NO socket reopen (so the move
            came from the daemon's push, not the reconnect catch-up)
Scenario 4  a run finishing in project B (not open) leaves A's badge at `2` —
            and B's own count proves that run really did land
Scenario 5  manual refresh still works, and the move is bound to the click
Scenario 6  the count stays scoped — 2 badges in the DOM, 1 visible

The trigger is a REAL `ralphy run` in a remote-less fixture repo: it fails fast
(`no git remotes found`, ~1s) and that exit is the nudge. `RALPHY_EXE_OVERRIDE`
cannot be used here — `changes.list` resolves the same exe and would spawn the
fake child, destroying the very count under test.

Boots a Localhost daemon on 7410 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/310-changes-nudge-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_changes_310.py   (exit 0 = all pass)
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

PORT = 7410
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_changes_310.py -> repo root is 4 dirs up.
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
    empty = tempfile.mkdtemp(prefix="wb310_empty_")
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


def make_fixture_repo(tag):
    """A committed git repo with NO remote (so a dispatched `ralphy run` exits in
    about a second), then one modification on top — a starting count of 1.
    `.gitignore` hides `.ralphy/`, so a run's own scratch never moves the count."""
    d = tempfile.mkdtemp(prefix=f"wb310_{tag}_")
    p = Path(d)
    (p / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (p / "README.md").write_text(f"# {tag}\n\nThe #310 nudge fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb310@example.com"],
        ["git", "config", "user.name", "wb310"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
    (p / "README.md").write_text("# edited\n", encoding="utf-8")  # the 1st change
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


# #317 promoted the Changes section to a rail view and moved its count onto
# the Projects row. The nudge's subject is unchanged: this is still the OPEN
# project's count, and every other project's badge is hidden with its row.
VISIBLE_SECS = (
    "Array.from(document.querySelectorAll('li.project .chg-badge'))"
    ".filter(e => e.offsetParent !== null)"
)


def badge_text(page):
    """The OPEN project's change count, on its Projects row. Every other
    project's row is hidden while one is open; returning `MULTI` when more than
    one badge is visible keeps a scoping regression from reading as a pass."""
    return page.evaluate(
        f"() => {{ const els = {VISIBLE_SECS};"
        " if (els.length > 1) return 'MULTI';"
        " return els.length ? els[0].textContent.trim() : null; }"
    )


def wait_badge(page, expected, timeout=15000):
    # Never a bare `evaluate` after a state change: an Alpine `x-show` flip is not
    # visible to the very next read (KNOWLEDGE.md / #307).
    page.wait_for_function(
        f"(want) => {{ const els = {VISIBLE_SECS};"
        " return els.length === 1 && els[0].textContent.trim() === want; }",
        arg=expected,
        timeout=timeout,
    )


# Installed BEFORE any app script runs, so it sees every socket the page opens
# and every Observe call it makes. Two facts the scenarios below need:
#   __wsOpens  — how many `/ws/tree` sockets were constructed. `subscribeChanges`
#                synthesizes a local `changes.dirty` on each (re)open, so a badge
#                that moved without a new socket can only have been moved by a
#                frame that arrived over the wire.
#   __listReads — how many `changes.list` reads were issued, i.e. the mechanism a
#                polling timer would show up in.
INSTRUMENT = """
window.__wsOpens = 0;
window.__listReads = 0;
const RealWS = window.WebSocket;
window.WebSocket = function (url, protocols) {
  if (String(url).indexOf('/ws/tree') !== -1) window.__wsOpens++;
  return protocols ? new RealWS(url, protocols) : new RealWS(url);
};
window.WebSocket.prototype = RealWS.prototype;
Object.assign(window.WebSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
document.addEventListener('DOMContentLoaded', () => {
  const realObserve = window.WBDaemon.observe;
  window.WBDaemon.observe = (verb, payload) => {
    if (verb === 'changes.list') window.__listReads++;
    return realObserve(verb, payload);
  };
});
"""


def counters(page):
    return page.evaluate("() => ({ ws: window.__wsOpens, reads: window.__listReads })")


def fire_run(page, slug):
    """Dispatch the same `workbench:action` the run button emits — a real Spawn
    over `/ws/command`, whose child exit is the nudge under test."""
    page.evaluate(
        "(slug) => document.dispatchEvent(new CustomEvent('workbench:action',"
        " { detail: { action: 'run-start', project: slug, agent: 'claude', branchMode: 'new' } }))",
        slug,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb310_reg_")
    dir_a = make_fixture_repo("a")
    dir_b = make_fixture_repo("b")
    slug_a = register_fixture(daemon_dir, dir_a)
    slug_b = register_fixture(daemon_dir, dir_b)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            # A bare `return` here would skip the exit gate below and report
            # success with ZERO browser assertions run.
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            # DOM renderer, no WebGL: headless chromium's WebGL canvas reads
            # empty text even when content shows (KNOWLEDGE.md).
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1400, "height": 900})
            page = ctx.new_page()
            page.add_init_script(INSTRUMENT)
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)

            # --- scenario 1: the open project's count ------------------------
            page.evaluate(f"() => {SH}.toggle('{slug_a}')")
            wait_badge(page, "1")
            check("the open project's Changes count reads 1", badge_text(page) == "1")

            # --- scenario 2: no poll, no repo-wide watch ---------------------
            # Settle first: `toggle()` fires its own async `changes.list`, and an
            # in-flight read picking up the write would counterfeit this proof.
            page.wait_for_timeout(1000)
            before = counters(page)
            Path(dir_a, "nudge.txt").write_text("written outside the browser\n", encoding="utf-8")
            page.wait_for_timeout(3000)
            got = badge_text(page)
            check(
                "a write with no run leaves the count at 1 for 3s (no poll, no watch)",
                got == "1",
                f"got={got!r}",
            )
            # …and the MECHANISM, not just the number: a timer of ANY period would
            # have issued a `changes.list` in that window; zero reads means the
            # only triggers are the ones this branch wires by hand.
            after = counters(page)
            check(
                "no `changes.list` read was issued in that window (no timer at all)",
                after["reads"] == before["reads"],
                f"reads {before['reads']} -> {after['reads']}",
            )

            # --- scenario 3: a finished run refreshes the count --------------
            before = counters(page)
            fire_run(page, slug_a)
            wait_badge(page, "2", timeout=60000)
            check(
                "a finished run refreshes the count to 2 with no manual refresh",
                badge_text(page) == "2",
            )
            # The refresh must be the daemon's PUSH, not the socket reopening: a
            # reconnect synthesizes its own catch-up `changes.dirty` locally, so a
            # daemon that pushed nothing would still reach 2 if the socket flapped.
            after = counters(page)
            check(
                "the refresh came over the wire — the /ws/tree socket never reopened",
                after["ws"] == before["ws"],
                f"ws opens {before['ws']} -> {after['ws']}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "310-changes-nudge-2026-07-25.png"))

            # --- scenario 4: a nudge for a repo that is not open --------------
            Path(dir_b, "b-only.txt").write_text("only in B\n", encoding="utf-8")
            fire_run(page, slug_b)
            page.wait_for_timeout(5000)
            got = badge_text(page)
            check(
                "a run finishing in a project that is not open leaves the count at 2",
                got == "2",
                f"got={got!r}",
            )
            # …and B's run really did spawn AND exit — otherwise the check above
            # passes vacuously, proving only that nothing happened at all.
            page.evaluate(f"() => {SH}.toggle('{slug_a}')")  # close A
            page.evaluate(f"() => {SH}.toggle('{slug_b}')")  # open B
            wait_badge(page, "2")
            check("the un-opened project's own run did land (B reads 2)", badge_text(page) == "2")
            page.evaluate(f"() => {SH}.toggle('{slug_b}')")
            page.evaluate(f"() => {SH}.toggle('{slug_a}')")
            wait_badge(page, "2")

            # --- scenario 5: manual refresh still works ----------------------
            # Settle after the reopen above: `toggle()` and the subscription's
            # catch-up both read `changes.list`, and a read still in flight when
            # the file lands would move the badge with no refresh and no nudge.
            page.wait_for_timeout(1000)
            Path(dir_a, "late.txt").write_text("late arrival\n", encoding="utf-8")
            # Bind the move to the CLICK: with no run in flight nothing can nudge,
            # so a badge that moved before the click would be a poll.
            page.wait_for_timeout(1500)
            got = badge_text(page)
            check("the count is still 2 until the refresh is clicked", got == "2", f"got={got!r}")
            page.click(".side-refresh")
            wait_badge(page, "3")
            check("the sidebar refresh still reloads the count to 3", badge_text(page) == "3")

            # --- scenario 6: the count stays scoped --------------------------
            # Both projects have a section in the DOM; exactly one is visible.
            secs = page.evaluate(
                f"() => ({{ all: document.querySelectorAll('li.project .chg-badge').length,"
                f" visible: {VISIBLE_SECS}.length }})"
            )
            check(
                "both projects hold a badge but only the open one shows",
                secs["all"] == 2 and secs["visible"] == 1,
                f"got={secs}",
            )
            check("the daemon under test is still the one we launched", proc.poll() is None)

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("CHANGES NUDGE LIVE")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
