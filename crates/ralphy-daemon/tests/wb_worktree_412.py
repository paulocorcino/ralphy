"""#412 browser acceptance: switch a console between checkouts from its
title bar — shown only when the repo has a worktree.

One Playwright pass over a REAL daemon proving the ADR-0063 #412 amendment
end to end: an agent console on a repo with no worktree has a flat title and
no switcher; once `wt-a` exists the title's worktree segment is a button
reading `primary`; its menu lists `primary` + `wt-a` with the current one
marked; picking `wt-a` asks, then relaunches the console in the worktree
(the helper child's `CWD:` line is the oracle) with the record written first;
the picker's own selection never moves; switching back lands on the primary.

Fixture: a repo on `main` with `README.md`, NO pre-made worktree (the picker
creates `wt-a`), registered through `ralphy daemon add`.

Scenario 1  the daemon is listening
Scenario 2  a console on a repo WITHOUT worktrees: flat title, no
            `.session-checkout` button
Scenario 3  create `wt-a` from the picker (selection stays `primary`) → the
            open console's title now carries a `primary` switcher
Scenario 4  open the menu: rows primary (current) + wt-a · wt-a; Escape
            closes it; picking `primary` (current) is a no-op
Scenario 5  pick `wt-a`, cancel the confirm → nothing changes
Scenario 6  pick `wt-a`, confirm → the desk record reads wt-a, the console
            relaunches: title `claude · wt-a · …`, CWD inside wt-a, the
            launch socket carries `checkout=wt-a`; the picker's selection is
            still unset; screenshot HERE
Scenario 7  switch back to `primary` → title without wt-a, CWD is the root
Scenario 8  a plain shell console never gets a switcher
Scenario 9  no page errors

Boots a Localhost daemon on 7458 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name.

Portability knobs: `RALPHY_TEST_SKIP_BUILD=1` skips the cargo builds and
`RALPHY_TEST_TARGET_DIR=<dir>` (relative to the repo root) names where the
binaries are.

Writes docs/screenshots/412-worktree-switch-2026-09-15.png (`-linux` off Windows).
Run: python crates/ralphy-daemon/tests/wb_worktree_412.py   (exit 0 = all pass)
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

PORT = 7458
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_409.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, os.environ.get("RALPHY_TEST_TARGET_DIR", os.path.join("target", "debug")))
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "412-worktree-switch-2026-09-15" + ("" if os.name == "nt" else "-linux") + ".png"
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
    empty = tempfile.mkdtemp(prefix="wb412_empty_")
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
    (d / "README.md").write_bytes(f"# {name}\n\nThe #412 switcher fixture repo.\n".encode())
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb412@example.com")
    git(d, "config", "user.name", "wb412")
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


SWITCHER = "() => document.querySelector('.session-window .session-checkout')"
SWITCHER_TEXT = "() => (document.querySelector('.session-window .session-checkout') || {}).textContent"
MENU_ROWS = (
    "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item')]"
    "  .map(e => ({ name: e.querySelector('.session-checkout-name').textContent.trim(),"
    "    branch: (e.querySelector('.session-checkout-branch') || {}).textContent || '',"
    "    current: e.classList.contains('current') }))"
)
MENU_OPEN = "() => !!document.querySelector('.session-checkout-menu')"
CONFIRM = ".wb-confirm .btn.accent, .wb-confirm .btn.danger"
CONFIRM_OPEN = "() => !!document.querySelector('.wb-confirm')"


def click_menu_row(page, name):
    page.evaluate(
        "(n) => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item')]"
        "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === n).click()",
        arg=name,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb412_reg_")
    fixture = seed("wb412_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))
    wt = fixture / ".ralphy" / "worktrees" / "wt-a"
    wt_title = f"claude · wt-a · {slug} · {ENV_LABEL}"
    primary_title = f"claude · {slug} · {ENV_LABEL}"
    # Once the repo has a worktree the segment exists and reads `primary`.
    primary_sw_title = f"claude · primary · {slug} · {ENV_LABEL}"

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

            # --- scenario 2: no worktree → flat title, no switcher --------------
            open_project(page, slug)
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=primary_title, timeout=15000)
            check("the child printed READY", wait_flat_contains(page, 0, READY))
            # The console module's own listing read must have landed before the
            # absence of a switcher means anything.
            page.wait_for_timeout(1500)
            check("a repo without worktrees shows no switcher", page.evaluate(SWITCHER) is None)
            check("…and the flat title", page.evaluate(TITLES) == [primary_title], f"got={page.evaluate(TITLES)!r}")

            # --- scenario 3: create wt-a → the title grows a `primary` switcher --
            open_picker(page, slug)
            type_name_and_enter(page, "wt-a")
            page.wait_for_function(ROW_COUNT_IS, arg=2, timeout=20000)
            check("the wt-a directory exists", wt.is_dir(), str(wt))
            page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
            page.wait_for_function(f"() => {SH}.branchOpen === false", timeout=10000)
            page.wait_for_function(f"() => !!({SWITCHER})()", timeout=15000)
            text = page.evaluate(SWITCHER_TEXT)
            check("the open console's title now carries a switcher reading `primary`", (text or "").strip() == "primary", f"got={text!r}")
            check("the picker's selection is unset", page.evaluate(f"(s) => {SH}.checkoutOf(s) === null", arg=slug))

            # --- scenario 4: the menu ---------------------------------------------
            page.evaluate(f"() => ({SWITCHER})().click()")
            page.wait_for_function(MENU_OPEN, timeout=5000)
            rows = page.evaluate(MENU_ROWS)
            check(
                "the menu lists primary (current) then wt-a · wt-a",
                rows == [{"name": "primary", "branch": "", "current": True}, {"name": "wt-a", "branch": "wt-a", "current": False}],
                f"got={rows!r}",
            )
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => !({MENU_OPEN})()", timeout=5000)
            check("Escape closes the menu", True)
            page.evaluate(f"() => ({SWITCHER})().click()")
            page.wait_for_function(MENU_OPEN, timeout=5000)
            click_menu_row(page, "primary")
            page.wait_for_timeout(300)
            check("picking the current entry asks nothing", not page.evaluate(CONFIRM_OPEN) and not page.evaluate(MENU_OPEN))

            # --- scenario 5: cancel ----------------------------------------------
            launches.clear()
            page.evaluate(f"() => ({SWITCHER})().click()")
            page.wait_for_function(MENU_OPEN, timeout=5000)
            click_menu_row(page, "wt-a")
            page.wait_for_function(CONFIRM_OPEN, timeout=5000)
            check("picking wt-a asks first", True)
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => !({CONFIRM_OPEN})()", timeout=5000)
            page.wait_for_timeout(300)
            check("cancelling changes nothing: same title, no launch", page.evaluate(TITLES) == [primary_sw_title] and not launches, f"got={page.evaluate(TITLES)!r} launches={launches!r}")
            recs = desk_records(page)
            check("…and the record has no checkout", bool(recs) and "checkout" not in recs[0], f"got={recs!r}")

            # --- scenario 6: switch into wt-a -------------------------------------
            page.evaluate(f"() => ({SWITCHER})().click()")
            page.wait_for_function(MENU_OPEN, timeout=5000)
            click_menu_row(page, "wt-a")
            page.wait_for_function(CONFIRM_OPEN, timeout=5000)
            page.locator(CONFIRM).click()
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=wt_title, timeout=15000)
            check("the console relaunched with the wt-a title", True)
            check("…the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("…and its CWD: line is the worktree", ".ralphy/worktrees/wt-a" in buf, f"buffer={buf[:200]!r}")
            check("one launch socket, carrying checkout=wt-a", len(launches) == 1 and "checkout=wt-a" in launches[0], f"got={launches!r}")
            recs = desk_records(page)
            check("the record reads checkout wt-a", bool(recs) and recs[0].get("checkout") == "wt-a", f"got={recs!r}")
            check("the picker's selection is STILL unset", page.evaluate(f"(s) => {SH}.checkoutOf(s) === null", arg=slug))
            text = page.evaluate(SWITCHER_TEXT)
            check("the switcher now reads wt-a", (text or "").strip() == "wt-a", f"got={text!r}")
            page.evaluate(f"() => ({SWITCHER})().click()")
            page.wait_for_function(MENU_OPEN, timeout=5000)
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))

            # --- scenario 7: back to primary -------------------------------------
            launches.clear()
            click_menu_row(page, "primary")
            page.wait_for_function(CONFIRM_OPEN, timeout=5000)
            page.locator(CONFIRM).click()
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=primary_sw_title, timeout=15000)
            check("…the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("back on the primary: CWD is the fixture root", ".ralphy/worktrees" not in buf and "CWD:" in buf, f"buffer={buf[:200]!r}")
            check("the launch socket carries no checkout", len(launches) == 1 and "checkout=" not in launches[0], f"got={launches!r}")
            recs = desk_records(page)
            check("the record's checkout is cleared", bool(recs) and "checkout" not in recs[0], f"got={recs!r}")

            # --- scenario 8: a plain shell has no switcher ----------------------
            page.evaluate(f"() => {SH}.newPlainConsole()")
            page.wait_for_function(f"() => ({WINDOWS})() === 2", timeout=15000)
            page.wait_for_timeout(800)
            n = page.evaluate("() => document.querySelectorAll('.session-window .session-checkout').length")
            check("the plain console adds no switcher (still exactly one)", n == 1, f"got={n}")

            # --- scenario 9 ------------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    check_floor = 27
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
