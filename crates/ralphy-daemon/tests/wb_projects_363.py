"""#363 browser acceptance: removing a project from the Projects sidebar.

One Playwright pass over a REAL daemon with TWO registered fixture repos,
proving the sidebar's new Remove affordance: it exists and is laid out, it is
gated by the design-system confirm whose message states that the directory on
disk survives, Cancel makes NO daemon call at all, and Confirm drops the row
with no manual refresh while the daemon's own `/api/repos` agrees.

Scenario a  every project row carries a laid-out `.project-remove` control
Scenario b  clicking it opens the confirm, whose message contains the literal
            `The directory on disk is not deleted.`
Scenario c  Cancel opens ZERO `/ws/command` sockets (an `add_init_script`
            WebSocket spy sampled at document start) and leaves BOTH rows
Scenario d  Confirm makes the row vanish with no reload and no Refresh click
            (`wait_for_function` on row absence), and `GET /api/repos` then
            returns exactly 1 entry — the OTHER slug
Scenario e  a `project.remove` for a slug already gone is harmless: the reply
            path leaves no error state and the surviving row stays
Plus        the directory the removed project lived in is still on disk, which
            is the promise the confirm made.

Boots a Localhost daemon on 7441 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Every row assertion is gated on `offsetParent !== null && clientWidth > 0`:
a measurement of a zero-width element passes a "visible" test vacuously
(CONTEXT.md, the vacuous-geometry trap). The Remove button fades rather than
`display: none`s, so it keeps its box at rest and this gate is meaningful.

Writes docs/screenshots/363-projects-2026-07-30.png.
Run: python crates/ralphy-daemon/tests/wb_projects_363.py   (exit 0 = all pass)
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

PORT = 7441
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_projects_363.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = os.path.join(SHOT_DIR, "363-projects-2026-07-30.png")
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
    empty = tempfile.mkdtemp(prefix="wb363_empty_")
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
    """A committed git repo named `name`. Two of these make the sidebar's
    "the OTHER row survives" assertion meaningful."""
    d = Path(tempfile.mkdtemp(prefix="wb363_repo_")) / name
    d.mkdir()
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "a.txt").write_text("alpha\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb363@example.com")
    git(d, "config", "user.name", "wb363")
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


def add_plain_dir(daemon_dir, path, init=False):
    """Shell the REAL `ralphy daemon add` on a directory that is not a repo, with
    stdin PIPED — the non-interactive path. Returns (exit code, stderr)."""
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    argv = [EXE, "daemon", "add"] + (["--init"] if init else []) + [path]
    result = subprocess.run(
        argv, env=env, capture_output=True, encoding="utf-8", stdin=subprocess.DEVNULL
    )
    return result.returncode, (result.stderr or "") + (result.stdout or "")


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


# Every LAID-OUT project row: its slug title and whether its Remove control is
# itself laid out. A row present but not laid out is excluded, so "the button is
# on the row" cannot pass on a zero-width box.
PROJECT_ROWS = """
() => [...document.querySelectorAll('li.project')]
  .filter(li => li.offsetParent !== null && li.clientWidth > 0)
  .map(li => {
    const head = li.querySelector('.project-head');
    const btn = li.querySelector('.project-remove');
    return {
      slug: head?.querySelector('.project-slug')?.getAttribute('title') || null,
      hasRemove: !!btn,
      removeLaid: !!btn && btn.offsetParent !== null && btn.clientWidth > 0,
      removeLabel: btn?.getAttribute('aria-label') || null,
    };
  })
"""

# The confirm dialog as the operator sees it: open only when its box is actually
# laid out (Alpine's x-show display flip lands after the property write).
CONFIRM_STATE = """
() => {
  const box = document.querySelector('.modal.confirm-modal');
  const body = document.querySelector('.confirm-modal .confirm-body');
  return {
    laid: !!box && box.offsetParent !== null && box.clientWidth > 0,
    title: document.querySelector('.confirm-modal .modal-title')?.textContent.trim() || '',
    message: body ? body.textContent.trim() : '',
    confirmLabel: [...document.querySelectorAll('.confirm-modal .modal-foot .btn')]
      .map(b => b.textContent.trim()),
  };
}
"""

# Sampled at DOCUMENT START, before the app runs: a socket opened and closed
# before the assertion would be invisible to any later read (#334 trap).
WS_SPY = """
window.__wbSockets = [];
const Native = window.WebSocket;
window.WebSocket = function (url, protocols) {
  window.__wbSockets.push(String(url));
  return protocols === undefined ? new Native(url) : new Native(url, protocols);
};
window.WebSocket.prototype = Native.prototype;
Object.assign(window.WebSocket, {
  CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3,
});
"""


def command_sockets(page):
    return page.evaluate("() => (window.__wbSockets || []).filter(u => u.includes('/ws/command'))")


def row_locator_for(page, slug):
    return page.locator(f"li.project:has(.project-slug[title='{slug}']) .project-remove")


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb363_reg_")
    keep_dir = seed("keeper-fixture")
    drop_dir = seed("dropped-fixture")
    keep_slug = register_fixture(daemon_dir, keep_dir)
    drop_slug = register_fixture(daemon_dir, drop_dir)
    check("two fixture repos register under distinct slugs", keep_slug != drop_slug,
          "keep={} drop={}".format(keep_slug, drop_slug))

    # --- the CLI half, end to end over the REAL binary --------------------
    # A piped stdin must DECLINE rather than read EOF as the `[Y/n]` default —
    # the whole reason `--init` exists. Proved against the shipped binary, not
    # against the injected-answer core.
    plain = os.path.join(tempfile.mkdtemp(prefix="wb363_plain_"), "not-a-repo")
    os.makedirs(plain)
    code, out = add_plain_dir(daemon_dir, plain)
    check(
        "`daemon add` on a plain directory with piped stdin declines, creating nothing",
        code != 0 and "not a git repository" in out and not os.path.isdir(os.path.join(plain, ".git")),
        "code={} out={!r}".format(code, out.strip()[:160]),
    )
    check("…and the error points at the flag that would proceed", "--init" in out, "out={!r}".format(out.strip()[:160]))
    code, out = add_plain_dir(daemon_dir, plain, init=True)
    check(
        "`daemon add --init` initializes it and registers, no question asked",
        code == 0 and os.path.isdir(os.path.join(plain, ".git")),
        "code={} out={!r}".format(code, out.strip()[:160]),
    )
    # Undo it: the sidebar assertions below count on exactly TWO projects.
    subprocess.run(
        [EXE, "daemon", "remove", out.strip().split("registered ", 1)[1].split(" →")[0].strip()],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            ctx.add_init_script(WS_SPY)
            page = ctx.new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function(f"() => {SH}.projects.length === 2", timeout=15000)

            # --- scenario a: the control is on the row ------------------------
            page.wait_for_function(
                "() => [...document.querySelectorAll('li.project')].filter("
                "li => li.offsetParent !== null && li.clientWidth > 0).length === 2",
                timeout=15000,
            )
            rows = page.evaluate(PROJECT_ROWS)
            check("both projects render as laid-out rows", len(rows) == 2, "rows={}".format(rows))
            check(
                "every row carries a laid-out Remove control",
                len(rows) == 2 and all(r["hasRemove"] and r["removeLaid"] for r in rows),
                "rows={}".format(rows),
            )
            check(
                "…named for a screen reader, not just an icon",
                all(r["removeLabel"] == "Remove project" for r in rows),
                "labels={}".format([r["removeLabel"] for r in rows]),
            )

            # --- scenario b: the confirm, and what it promises ----------------
            # The baseline is sampled BEFORE the click, not before the Cancel:
            # an implementation that fired `project.remove` at click time and
            # only then confirmed would open its socket between the two, and a
            # delta measured from after-the-click would still read zero. The
            # invariant is "no socket across the click AND the cancel".
            before = len(command_sockets(page))
            row_locator_for(page, drop_slug).click()
            page.wait_for_function(f"() => {SH}.confirmModal.open === true", timeout=10000)
            page.wait_for_function(
                "() => { const b = document.querySelector('.modal.confirm-modal');"
                "  return !!b && b.offsetParent !== null && b.clientWidth > 0; }",
                timeout=10000,
            )
            confirm = page.evaluate(CONFIRM_STATE)
            check("clicking Remove opens the design-system confirm", confirm["laid"],
                  "confirm={}".format(confirm))
            check(
                "…whose message promises the directory on disk survives",
                "The directory on disk is not deleted." in confirm["message"],
                "message={!r}".format(confirm["message"]),
            )
            check(
                "…and names the project it is about",
                drop_slug in confirm["message"],
                "message={!r}".format(confirm["message"]),
            )
            check("…with a Remove action, not a bare OK", "Remove" in confirm["confirmLabel"],
                  "labels={}".format(confirm["confirmLabel"]))

            # --- scenario c: Cancel makes NO daemon call ----------------------
            mid = len(command_sockets(page))
            check(
                "opening the confirm opens no socket by itself",
                mid == before,
                "before={} after-click={}".format(before, mid),
            )
            page.evaluate(f"() => {SH}.confirmRespond(false)")
            page.wait_for_function(f"() => {SH}.confirmModal.open === false", timeout=10000)
            # Give a would-be socket a real chance to appear: an assertion that
            # samples in the same tick passes even against a broken order.
            page.wait_for_timeout(600)
            after = len(command_sockets(page))
            check(
                "Cancel opens ZERO /ws/command sockets, across the click AND the cancel",
                after == before,
                "before-click={} after-cancel={}".format(before, after),
            )
            rows = page.evaluate(PROJECT_ROWS)
            check("…and leaves both rows standing", len(rows) == 2, "rows={}".format(rows))

            page.screenshot(path=SHOT)
            print(f"[INFO] screenshot {SHOT}", flush=True)

            # --- scenario d: Confirm removes, with no manual refresh ----------
            row_locator_for(page, drop_slug).click()
            page.wait_for_function(f"() => {SH}.confirmModal.open === true", timeout=10000)
            page.evaluate(f"() => {SH}.confirmRespond(true)")
            gone = False
            try:
                page.wait_for_function(
                    "(slug) => ![...document.querySelectorAll('li.project')].some("
                    "li => li.querySelector('.project-slug')?.getAttribute('title') === slug)",
                    arg=drop_slug,
                    timeout=20000,
                )
                gone = True
            except Exception as e:
                # A raised TimeoutError here would abort the run BEFORE the
                # check_floor reconciliation, turning a real regression into a
                # crash with no [FAIL] line. Catch it and fail honestly.
                print("[INFO] row-absence wait raised: {}".format(type(e).__name__), flush=True)
            check("the confirmed row disappears with no reload and no Refresh click", gone)
            rows = page.evaluate(PROJECT_ROWS)
            check(
                "…and the surviving row is the OTHER project",
                len(rows) == 1 and rows[0]["slug"] == keep_slug,
                "rows={}".format(rows),
            )
            # The daemon is the source of truth: the optimistic splice must be
            # backed by a registry that actually lost the entry.
            page.wait_for_function(
                "() => fetch('/api/repos').then(r => r.json()).then(x => x.length === 1)",
                timeout=20000,
            )
            repos = page.evaluate("() => fetch('/api/repos').then(r => r.json())")
            check(
                "/api/repos serves exactly the surviving project",
                len(repos) == 1 and repos[0].get("slug") == keep_slug,
                "repos={}".format([r.get("slug") for r in repos]),
            )
            check(
                "the directory on disk survived the removal, as promised",
                os.path.isdir(drop_dir) and os.path.isdir(os.path.join(drop_dir, ".git")),
                "dir={}".format(drop_dir),
            )

            # --- scenario e: already-absent is harmless -----------------------
            # Auto-confirm the dialog, then ask to remove the slug that is
            # already gone. The routing lookup answers `unknown repo`; the UI
            # must treat that as the state it asked for, not an error.
            page.evaluate(
                "() => { const c = " + SH + ";"
                "  const orig = c.askConfirm.bind(c);"
                "  c.askConfirm = () => Promise.resolve(true);"
                "  c.__origAskConfirm = orig; }"
            )
            sockets_before = len(command_sockets(page))
            page.evaluate(f"() => {{ {SH}.runsActionMsg = ''; }}")
            page.evaluate(
                "(slug) => " + SH + ".removeProject({ slug: slug, path: slug })",
                arg=drop_slug,
            )
            page.wait_for_timeout(1500)
            check(
                "…and it really reached the daemon (a /ws/command socket was opened)",
                len(command_sockets(page)) > sockets_before,
                "before={} after={}".format(sockets_before, len(command_sockets(page))),
            )
            rows = page.evaluate(PROJECT_ROWS)
            check(
                "removing an already-absent project leaves the list unharmed",
                len(rows) == 1 and rows[0]["slug"] == keep_slug,
                "rows={}".format(rows),
            )
            # THIS is the scenario: the `unknown repo` reply must be folded as
            # success, not flashed as a refusal. Without it the check above
            # passes even when the UI surfaces "remove refused" — the list is
            # unharmed either way. Couples deliberately to the daemon's literal
            # `unknown repo` string (lib.rs), which is what `removeProject`
            # matches on; if that text ever drifts, this reds.
            flash = page.evaluate(f"() => {SH}.runsActionMsg")
            check(
                "…and is folded as success, never flashed as a refusal",
                not flash,
                "runsActionMsg={!r}".format(flash),
            )

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 23
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


if __name__ == "__main__":
    main()
