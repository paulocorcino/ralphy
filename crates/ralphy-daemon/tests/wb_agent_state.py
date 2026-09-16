"""ADR-0059 browser acceptance: the agent's hook-reported state as a dot on
the console's title bar, on the project row, and on the worktree row.

One Playwright pass over a REAL daemon. The helper child stands in for the
vendor; the script stands in for `ralphy hook status` by appending lines to
the session's `<store>/sessions/<id>.agent-status.jsonl` — the daemon's tail
folds them and `/api/sessions` carries `agent_state`, which the shell's poll
paints. Nothing here reads the terminal.

Fixture: a repo on `main` with `README.md`, NO pre-made worktree (the picker
creates `wt-a`), registered through `ralphy daemon add`.

Scenario 1  the daemon is listening
Scenario 2  a claude console on the primary: no dot before any hook line;
            the settings and status files exist under <store>/sessions/
Scenario 3  a `UserPromptSubmit` line → console dot `working`, project dot
            `live`, Go-to row dot `working`
Scenario 4  an `AskUserQuestion` line → console dot `waiting` with the
            question in its title, project dot `waiting`; screenshot HERE
Scenario 5  create `wt-a`, open a second console in it, `PermissionRequest`
            there → the picker's wt-a row shows `waiting`, the primary row
            shows the first console's state
Scenario 6  `Stop` on both → dots `done`, project dot back to `live`
Scenario 7  close a console → its files are gone
Scenario 8  no page errors

Boots a Localhost daemon on 7459 over a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon is stopped by its own subprocess handle, NEVER by name.

Portability knobs: `RALPHY_TEST_SKIP_BUILD=1` skips the cargo builds and
`RALPHY_TEST_TARGET_DIR=<dir>` (relative to the repo root) names where the
binaries are.

Writes docs/screenshots/agent-state-dots-2026-09-15.png (`-linux` off Windows).
Run: python crates/ralphy-daemon/tests/wb_agent_state.py   (exit 0 = all pass)
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

PORT = 7459
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_409.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, os.environ.get("RALPHY_TEST_TARGET_DIR", os.path.join("target", "debug")))
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "agent-state-dots-2026-09-15" + ("" if os.name == "nt" else "-linux") + ".png"
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The local environment label, as `peer::environment_label` spells it.
_distro = os.environ.get("WSL_DISTRO_NAME")
ENV_LABEL = f"WSL: {_distro}" if _distro else ("Windows" if os.name == "nt" else "Linux")
FILTER_INPUT = ".branch-modal .branch-search input"
CREATE_WT_ROW = ".branch-modal .branch-item.create-worktree"
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
    empty = tempfile.mkdtemp(prefix="wbstate_empty_")
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
    (d / "README.md").write_bytes(f"# {name}\n\nThe agent-state fixture repo.\n".encode())
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wbstate@example.com")
    git(d, "config", "user.name", "wbstate")
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


# Cut a worktree the console's way (ADR-0063, amendment 2026-09-16 b): the
# i-th console's title switcher → `new worktree…` → the prompt's name field →
# "Create & restart". Returns once the console's title names the worktree.
SWITCHER_OF = "(i) => document.querySelectorAll('.session-window')[i].querySelector('.session-checkout')"
CREATE_ITEM = "() => document.querySelector('.session-checkout-menu .session-checkout-item.create')"
PROMPT = ".wb-worktree"


def create_via_switcher(page, i, name, title):
    page.wait_for_function(f"(i) => !!({SWITCHER_OF})(i)", arg=i, timeout=15000)
    page.evaluate(f"(i) => ({SWITCHER_OF})(i).click()", arg=i)
    page.wait_for_function(f"() => !!({CREATE_ITEM})()", timeout=5000)
    page.evaluate(f"() => ({CREATE_ITEM})().click()")
    page.wait_for_selector(PROMPT, state="visible", timeout=10000)
    page.fill(PROMPT + " input.prompt-input", name)
    page.click(PROMPT + " .btn.accent")
    page.wait_for_function(f"(t) => ({TITLES})().includes(t)", arg=title, timeout=30000)


def type_name_and_enter(page, name):
    """Create a worktree the picker's way (reshaped 2026-09-16): the name goes
    in the search box and the `Create worktree` row is the act."""
    page.fill(FILTER_INPUT, name)
    page.wait_for_selector(CREATE_WT_ROW, state="visible", timeout=5000)
    page.click(CREATE_WT_ROW)


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


CONSOLE_DOT = (
    "(i) => { const w = document.querySelectorAll('.session-window')[i];"
    "  const d = w && w.querySelector('.session-state');"
    "  return d ? { hidden: d.hidden, cls: d.className, title: d.title } : null; }"
)
PROJECT_DOT = "() => (document.querySelector('li.project .dot') || {}).className || ''"
GOTO_STATES = (
    "() => [...document.querySelectorAll('.window-menu .window-item .session-state')]"
    "  .map(e => e.className.replace('session-state', '').trim())"
)
ROW_STATE = (
    "(n) => { const r = (" + ROW_BY_NAME + ")(n); const d = r && r.querySelector('.worktree-state');"
    "  return d && d.offsetParent !== null ? d.className.replace('worktree-state', '').trim() : null; }"
)


def status_file(daemon_dir, sid):
    return os.path.join(daemon_dir, "sessions", f"{sid}.agent-status.jsonl")


def hook(daemon_dir, sid, event, tool=None, tool_input=None):
    """Stand in for `ralphy hook status`: append one line."""
    line = json.dumps(
        {"event": event, "tool_name": tool, "tool_input": tool_input, "interrupted": False, "ts": "2026-09-15T10:00:00-03:00"}
    )
    with open(status_file(daemon_dir, sid), "a", encoding="utf-8") as f:
        f.write(line + "\n")


def wait_console_dot(page, i, cls, timeout=10000):
    page.wait_for_function(
        f"([i, c]) => {{ const d = ({CONSOLE_DOT})(i); return !!d && !d.hidden && d.cls === 'session-state ' + c; }}",
        arg=[i, cls],
        timeout=timeout,
    )


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbstate_reg_")
    fixture = seed("wbstate_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))

    proc = launch(daemon_dir)
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
            wait_shell(page)

            # --- scenario 2: a console, no dot yet ---------------------------------
            open_project(page, slug)
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            check("the child printed READY", wait_flat_contains(page, 0, READY))
            sid = sessions()[0]["id"]
            check("the session's status file exists", os.path.isfile(status_file(daemon_dir, sid)), status_file(daemon_dir, sid))
            check("…and its settings file", os.path.isfile(os.path.join(daemon_dir, "sessions", f"{sid}.settings.json")))
            page.wait_for_timeout(2500)
            dot = page.evaluate(CONSOLE_DOT, 0)
            check("no dot before any hook line", dot is not None and dot["hidden"] is True, f"got={dot!r}")
            check("the project dot is live (a session, no state)", page.evaluate(PROJECT_DOT) == "dot live", f"got={page.evaluate(PROJECT_DOT)!r}")

            # --- scenario 3: working -----------------------------------------------
            hook(daemon_dir, sid, "UserPromptSubmit")
            wait_console_dot(page, 0, "working")
            check("a UserPromptSubmit line paints the console dot working", True)
            check("…the project dot stays live", page.evaluate(PROJECT_DOT) == "dot live", f"got={page.evaluate(PROJECT_DOT)!r}")
            page.evaluate(f"() => {{ {SH}.windowMenu = true; {SH}.windowList = WBConsole.list(); }}")
            page.wait_for_timeout(200)
            states = page.evaluate(GOTO_STATES)
            check("the Go-to row carries the state", states == ["working"], f"got={states!r}")
            page.evaluate(f"() => {{ {SH}.windowMenu = false; }}")

            # --- scenario 4: waiting -----------------------------------------------
            hook(daemon_dir, sid, "PreToolUse", "AskUserQuestion", {"questions": [{"question": "which port?"}]})
            wait_console_dot(page, 0, "waiting")
            dot = page.evaluate(CONSOLE_DOT, 0)
            check("an AskUserQuestion line paints it waiting, with the question", dot["title"] == "agent waiting: AskUserQuestion: which port?", f"got={dot!r}")
            page.wait_for_function(f"() => ({PROJECT_DOT})() === 'dot waiting'", timeout=10000)
            check("the project dot turns waiting", True)
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))

            # --- scenario 5: a worktree row ----------------------------------------
            # A second console, born on the primary, cuts wt-a from its own
            # switcher and restarts inside it.
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 2", timeout=15000)
            check("the second child printed READY", wait_flat_contains(page, 1, READY))
            create_via_switcher(page, 1, "wt-a", f"claude · wt-a · {slug} · {ENV_LABEL}")
            page.wait_for_function("() => fetch('/api/sessions').then(r => r.json()).then(l => l.some(s => s.checkout === 'wt-a'))", timeout=15000)
            check("the second console restarted in wt-a", wait_flat_contains(page, 1, READY))
            sid2 = [s["id"] for s in sessions() if s.get("checkout") == "wt-a"][0]
            hook(daemon_dir, sid2, "PermissionRequest", "Bash", {"command": "rm -rf x"})
            wait_console_dot(page, 1, "waiting")
            open_picker(page, slug)
            page.wait_for_function(f"(n) => ({ROW_STATE})(n) === 'waiting'", arg="wt-a", timeout=10000)
            check("the wt-a row shows the agent waiting there", True)
            row_primary = page.evaluate(ROW_STATE, "primary")
            check("the primary row shows the first console's state", row_primary == "waiting", f"got={row_primary!r}")
            page.evaluate(f"() => {{ {SH}.branchOpen = false; }}")
            page.wait_for_function(f"() => {SH}.branchOpen === false", timeout=10000)

            # --- scenario 6: done ----------------------------------------------------
            hook(daemon_dir, sid, "Stop")
            hook(daemon_dir, sid2, "Stop")
            wait_console_dot(page, 0, "done")
            wait_console_dot(page, 1, "done")
            check("Stop lines paint both dots done", True)
            page.wait_for_function(f"() => ({PROJECT_DOT})() === 'dot live'", timeout=10000)
            check("the project dot is back to live", True)

            # --- scenario 7: the files go with the session -------------------------
            page.locator(".session-window").nth(1).locator(".session-close").click()
            page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent").click()
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            deadline = time.time() + 10
            while time.time() < deadline and os.path.exists(status_file(daemon_dir, sid2)):
                time.sleep(0.2)
            check("a closed session's status file is gone", not os.path.exists(status_file(daemon_dir, sid2)))
            check("…and its settings file", not os.path.exists(os.path.join(daemon_dir, "sessions", f"{sid2}.settings.json")))

            # --- scenario 8 ----------------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    check_floor = 21
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
