"""#411 browser acceptance: a console opened in a worktree comes back in that
worktree after the daemon restarts — and says so when the worktree is gone.

One Playwright pass over a REAL daemon proving the ADR-0050/ADR-0063 #411
amendments end to end: the desk record carries the console's `checkout`; the
placeholder a restarted daemon leaves behind relaunches INTO that worktree
(the helper child's `CWD:` line is the oracle); and once the worktree has been
removed by hand while the daemon was down, the placeholder names it and its
one button relaunches on the primary — never silently.

Fixture: a repo on `main` with `README.md`, NO pre-made worktree (the picker
creates `wt-a`), registered through `ralphy daemon add`.

Scenario 1  the daemon is listening
Scenario 2  create `wt-a` from the picker, select it, open a console in it:
            title `claude · wt-a · <slug> · <env>`, CWD inside wt-a
Scenario 3  the desk record on the daemon carries `checkout: "wt-a"`
Scenario 4  daemon restarted → reload → the agent console waits as a
            placeholder (the launch opt-in is off), still recording wt-a;
            click reconnect → the console relaunches IN wt-a: same title,
            CWD inside wt-a; screenshot HERE
Scenario 5  daemon stopped, `git worktree remove --force wt-a` by hand,
            daemon relaunched → reload → the placeholder reads
            `worktree wt-a no longer exists`, its button `relaunch in
            primary`, no `/ws/session?agent=` socket was opened
Scenario 6  click it → a console on the primary: title without wt-a, CWD is
            the fixture root, the record's checkout is cleared
Scenario 7  no page errors

Boots a Localhost daemon on 7457 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name.

Portability knobs: `RALPHY_TEST_SKIP_BUILD=1` skips the cargo builds and
`RALPHY_TEST_TARGET_DIR=<dir>` (relative to the repo root) names where the
binaries are.

Writes docs/screenshots/411-worktree-restart-2026-09-15.png (`-linux` off Windows).
Run: python crates/ralphy-daemon/tests/wb_worktree_411.py   (exit 0 = all pass)
"""

import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7457
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_409.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, os.environ.get("RALPHY_TEST_TARGET_DIR", os.path.join("target", "debug")))
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "411-worktree-restart-2026-09-15" + ("" if os.name == "nt" else "-linux") + ".png"
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The local environment label, as `peer::environment_label` spells it.
_distro = os.environ.get("WSL_DISTRO_NAME")
ENV_LABEL = f"WSL: {_distro}" if _distro else ("Windows" if os.name == "nt" else "Linux")
NAME_INPUT = ".branch-modal .worktree-create-name"
READY = "READY"

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
    """A scratch registry + empty vendor stores, plus the deterministic helper
    child: `RALPHY_DAEMON_AGENT_OVERRIDE` makes every agent console a
    `session_test_child`, whose `CWD:<dir>` line is the oracle for "the child
    was spawned in the worktree"."""
    empty = tempfile.mkdtemp(prefix="wb411_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, encoding="utf-8").stdout.strip()


def seed(parent_prefix, name):
    """A committed git repo on `main` at a CHOSEN directory name, `.ralphy/`
    gitignored so a worktree under it never dirties the primary tree."""
    d = Path(tempfile.mkdtemp(prefix=parent_prefix)) / name
    d.mkdir()
    (d / ".gitignore").write_bytes(b".ralphy/\n")
    (d / "README.md").write_bytes(f"# {name}\n\nThe #411 restart fixture repo.\n".encode())
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb411@example.com")
    git(d, "config", "user.name", "wb411")
    git(d, "config", "core.autocrlf", "false")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's panel.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def sessions():
    with urllib.request.urlopen(BASE + "api/sessions", timeout=5) as r:
        return json.load(r)


# A laid-out picker row by its `.worktree-name`.
ROW_BY_NAME = (
    "(n) => [...document.querySelectorAll('.branch-modal .worktree-item')]"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0)"
    "  .find(e => e.querySelector('.worktree-name')?.textContent.trim() === n) || null"
)
REMOVE_BTN = "(n) => (" + ROW_BY_NAME + ")(n)?.querySelector('.worktree-remove') || null"
ROWS_EXPR = (
    "() => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0)"
    "  .map(e => ({ name: e.querySelector('.worktree-name').textContent.trim(),"
    "    branch: e.querySelector('.worktree-branch').textContent.trim() }))"
)
ROW_COUNT_IS = (
    "(n) => Array.from(document.querySelectorAll('.branch-modal .worktree-item'))"
    "  .filter(e => e.offsetParent !== null && e.clientWidth > 0).length === n"
)
WORKTREE_COUNT_IS = f"(n) => (({SH}.branchModal.checkouts || {{}}).worktrees || []).length === n"
CHIP_TEXT = (
    "() => ((document.querySelector('li.project.open .files-sec .branch-chip-name') || {}).textContent || '')"
    "  .trim()"
)
# Every console window's title, trimmed.
TITLES = "() => [...document.querySelectorAll('.session-title')].map(e => e.textContent.trim())"
WINDOWS = "() => document.querySelectorAll('.session-window').length"
# The refusal as the modal renders it (a laid-out `.worktree-create-error`).
SHOWN_ERROR = (
    "() => { const e = document.querySelector('.branch-modal .worktree-create-error');"
    "  return e && e.offsetParent !== null && e.clientWidth > 0 ? e.textContent.trim() : null; }"
)


def wait_shell(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function(f"() => {SH}.projects.length === 1", timeout=15000)
    page.wait_for_function(
        "() => Array.from(document.querySelectorAll('li.project'))"
        "  .filter(e => e.offsetParent !== null).length === 1",
        timeout=15000,
    )


def show_view(page, view):
    """Clicking the rail button of the view already showing COLLAPSES the
    sidebar, so every switch is guarded on that view's own visibility (#317)."""
    page.evaluate(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " if (!el || el.offsetParent === null)"
        "   document.querySelector(`nav.rail button[title=\"${v === 'changes' ? 'Changes' : 'Projects'}\"]`)"
        "     .click(); }",
        arg=view,
    )
    page.wait_for_function(
        "(v) => { const el = document.querySelector('.' + v + '-view');"
        " return !!el && el.offsetParent !== null; }",
        arg=view,
        timeout=15000,
    )


def open_project(page, slug):
    show_view(page, "projects")
    # The slug rides as an ARGUMENT, never interpolated: a repo registered from
    # a Windows path carries backslashes a string literal would swallow (#316).
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(
        "() => { const c = document.querySelector('li.project.open .files-sec .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )


def open_picker(page, slug):
    open_project(page, slug)
    if not page.evaluate(f"() => {SH}.branchOpen === true"):
        page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # The listing arrives by round trip: gate on the reply having landed, not
    # on the modal being open (an open modal samples an empty section).
    page.wait_for_function(f"() => {SH}.branchModal.checkouts !== null", timeout=15000)


def click_row(page, name):
    page.wait_for_function(f"(n) => !!({ROW_BY_NAME})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({ROW_BY_NAME})(n).click()", arg=name)
    page.wait_for_function(f"() => {SH}.branchOpen === false", timeout=10000)


def click_remove(page, name):
    page.wait_for_function(f"(n) => !!({REMOVE_BTN})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({REMOVE_BTN})(n).click()", arg=name)


def wait_settled(page):
    """`removing` is cleared only after the `loadWorktrees` re-read landed, so
    this is the value to gate on before anyone counts rows (#405 trap)."""
    page.wait_for_function(f"() => {SH}.branchModal.removing === null", timeout=15000)


def refusal(page):
    """Wait for `branchError` to land AND the re-read to settle."""
    page.wait_for_function(f"() => {SH}.branchError !== ''", timeout=15000)
    wait_settled(page)
    return {"err": page.evaluate(f"() => {SH}.branchError"), "shown": page.evaluate(SHOWN_ERROR)}


def type_name_and_enter(page, name):
    page.fill(NAME_INPUT, name)
    page.press(NAME_INPUT, "Enter")


def screen(page, i=0):
    """The i-th console window's whole terminal buffer as text."""
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w._term && w._term.term && w._term.term.buffer.active;"
        " if (!b) return '';"
        " let out = '';"
        " for (let y = 0; y < b.length; y++) {"
        "   const line = b.getLine(y);"
        "   if (line) out += line.translateToString(true) + '\\n';"
        " }"
        " return out; }",
        i,
    )


def flat(page, i=0):
    """The buffer with its line breaks removed: xterm hard-wraps at the terminal
    width, so a long `CWD:` path is split mid-word in the buffer."""
    return screen(page, i).replace("\n", "")


def wait_flat_contains(page, i, needle, timeout=15000):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if needle in flat(page, i):
            return True
        page.wait_for_timeout(250)
    return needle in flat(page, i)


def wait_sessions_empty(timeout=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if not sessions():
            return True
        time.sleep(0.25)
    return not sessions()


def desk_records(page):
    """The saved desk, read from the DAEMON. Settles past the shell's 250 ms
    upload debounce first."""
    page.wait_for_timeout(400)
    return page.request.get(BASE + "api/desk").json()["windows"]


NO_PLACEHOLDER = "() => document.querySelectorAll('.session-window.placeholder').length === 0"


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb411_reg_")
    fixture = seed("wb411_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))
    wt = fixture / ".ralphy" / "worktrees" / "wt-a"
    expected_title = f"claude · wt-a · {slug} · {ENV_LABEL}"
    primary_title = f"claude · {slug} · {ENV_LABEL}"

    proc = launch(daemon_dir)
    try:
        # --- scenario 1 ---------------------------------------------------
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
            launches = []
            page.on("websocket", lambda ws: launches.append(ws.url) if "agent=" in ws.url else None)
            page.goto(BASE)
            wait_shell(page)

            # --- scenario 2: create wt-a, select, open a console in it -----------
            open_picker(page, slug)
            type_name_and_enter(page, "wt-a")
            page.wait_for_function(ROW_COUNT_IS, arg=2, timeout=20000)
            check("the wt-a directory exists", wt.is_dir(), str(wt))
            click_row(page, "wt-a")
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=10000)
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=expected_title, timeout=15000)
            check("the console title names wt-a", page.evaluate(TITLES) == [expected_title])
            check("the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("the child's CWD: line is the worktree", ".ralphy/worktrees/wt-a" in buf, f"buffer={buf[:200]!r}")

            # --- scenario 3: the desk record carries the checkout ----------------
            recs = desk_records(page)
            check("one desk record", len(recs) == 1, f"got={recs!r}")
            check(
                "the record carries `checkout: wt-a`",
                bool(recs) and recs[0].get("checkout") == "wt-a" and recs[0].get("kind") == "agent",
                f"got={recs!r}",
            )

            # --- scenario 4: daemon restart → placeholder → relaunch in wt-a ----
            stop(proc)
            proc = launch(daemon_dir)
            check("the daemon restarted on the same port", wait_listening(BASE))
            launches.clear()
            page.reload()
            wait_shell(page)
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            ph = page.locator(".session-window.placeholder")
            check("the agent console comes back as a placeholder", ph.count() == 1, f"got={ph.count()}")
            page.wait_for_timeout(600)
            note = ph.locator(".session-offline p").inner_text().strip()
            check("…still saying `not running` (the worktree is there)", note == "agent console — not running", f"got={note!r}")
            check("no agent socket was opened by the reload", not launches, f"got={launches!r}")
            recs = desk_records(page)
            check("the restored record still carries wt-a", bool(recs) and recs[0].get("checkout") == "wt-a", f"got={recs!r}")
            ph.locator(".session-reconnect").click()
            page.wait_for_function(NO_PLACEHOLDER, timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=expected_title, timeout=15000)
            check("the relaunched console's title names wt-a", page.evaluate(TITLES) == [expected_title])
            check("…the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("…and its CWD: line is the worktree again", ".ralphy/worktrees/wt-a" in buf, f"buffer={buf[:200]!r}")
            check("one launch socket, carrying checkout=wt-a", len(launches) == 1 and "checkout=wt-a" in launches[0], f"got={launches!r}")
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))

            # --- scenario 5: the worktree vanishes while the daemon is down ------
            stop(proc)
            git(fixture, "worktree", "remove", "--force", ".ralphy/worktrees/wt-a")
            check("wt-a is gone from disk", not wt.exists())
            proc = launch(daemon_dir)
            check("the daemon relaunched", wait_listening(BASE))
            launches.clear()
            page.reload()
            wait_shell(page)
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            page.wait_for_selector(".session-window.placeholder.missing-checkout", timeout=15000)
            ph = page.locator(".session-window.placeholder")
            note = ph.locator(".session-offline p").inner_text().strip()
            check("the placeholder names the missing worktree", note == "worktree wt-a no longer exists", f"got={note!r}")
            label = ph.locator(".session-reconnect").inner_text().strip()
            check("its one button reads `relaunch in primary`", label == "relaunch in primary", f"got={label!r}")
            check("no agent socket was opened", not launches, f"got={launches!r}")

            # --- scenario 6: relaunch in primary, by that label ------------------
            ph.locator(".session-reconnect").click()
            page.wait_for_function(NO_PLACEHOLDER, timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=primary_title, timeout=15000)
            check("the console title has no worktree segment", page.evaluate(TITLES) == [primary_title])
            check("…the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("…and its CWD: line is the primary tree", ".ralphy/worktrees" not in buf and "CWD:" in buf, f"buffer={buf[:200]!r}")
            check("the launch socket carries no checkout", len(launches) == 1 and "checkout=" not in launches[0], f"got={launches!r}")
            recs = desk_records(page)
            check("the record's checkout is cleared", bool(recs) and "checkout" not in recs[0], f"got={recs!r}")

            # --- scenario 7 ------------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    check_floor = 28
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
