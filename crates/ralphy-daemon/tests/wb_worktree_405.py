"""#405 browser acceptance: creating a worktree from a console's own switcher.

One Playwright pass over a REAL daemon proving the create path end to end
(ADR-0063 §2, amendment 2026-09-16 b): an agent console's title switcher
exists before any worktree does and ends with `new worktree…`; the prompt
takes a name and the branch to cut from (default: the primary's); "Create &
restart" runs `worktree.add` and restarts THAT console inside the new tree
(the helper child's `CWD:` line is the oracle); the worktree exists on disk
with its base recorded; a refusal the daemon sends comes back into the prompt
verbatim; a name the daemon would not take never leaves the prompt.

Scenario 1  the daemon is listening
Scenario 2  a console on a fixture with NO worktrees: switcher reads
            `primary`, menu = `primary` (current) + `new worktree…`
Scenario 3  `new worktree…` opens the prompt: base select defaults to `main`
            and lists `taken`; the note names the carry-over keys
Scenario 4  name `wt-new` + "Create & restart" → the console restarts with
            title `claude · wt-new · …` and CWD inside wt-new; ONE relaunch
            socket carrying `checkout=wt-new`; the record reads wt-new;
            `<fixture>/.ralphy/worktrees/wt-new` exists;
            `git config branch.wt-new.base` is `main`; the shell's listing
            shows the new worktree; the Files selection stays unset
Scenario 5  name `taken` → the daemon refuses (`already exists`); the prompt
            re-opens with that message and the name kept; Escape cancels;
            no directory
Scenario 6  name `a/b` → the prompt's own gate refuses; no `worktree.add`
            reached the daemon; no directory
Scenario 7  name `wt-off-taken` with base `taken` → created from `taken`
            (`branch.wt-off-taken.base = taken`), the console restarts there
Scenario 8  no page errors

Boots a Localhost daemon on 7452 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Portability knobs: `RALPHY_TEST_SKIP_BUILD=1` skips the cargo builds and
`RALPHY_TEST_TARGET_DIR=<dir>` (relative to the repo root) names where the
binaries are.

Writes docs/screenshots/405-worktree-create-2026-09-15.png (`-linux` off Windows).
Run: python crates/ralphy-daemon/tests/wb_worktree_405.py   (exit 0 = all pass)
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

PORT = 7452
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_409.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, os.environ.get("RALPHY_TEST_TARGET_DIR", os.path.join("target", "debug")))
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "405-worktree-create-2026-09-15" + ("" if os.name == "nt" else "-linux") + ".png"
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
    empty = tempfile.mkdtemp(prefix="wb405_empty_")
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
    (d / "README.md").write_bytes(f"# {name}\n\nThe #405 create fixture repo.\n".encode())
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb405@example.com")
    git(d, "config", "user.name", "wb405")
    git(d, "config", "core.autocrlf", "false")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    # A branch `taken` that is NOT checked out anywhere: the `already exists`
    # refusal, distinct from `is checked out at`; also a base to cut from.
    git(d, "branch", "taken")
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


CHIP_TEXT = (
    "() => ((document.querySelector('li.project.open .project-head .branch-chip-name') || {}).textContent || '')"
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
        "() => { const c = document.querySelector('li.project.open .project-head .branch-chip');"
        "  return !!c && c.offsetParent !== null && c.clientWidth > 0; }",
        timeout=15000,
    )


def open_picker(page, slug):
    open_project(page, slug)
    if not page.evaluate(f"() => {SH}.branchOpen === true"):
        page.evaluate("() => document.querySelector('li.project.open .project-head .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # The listing arrives by round trip: gate on the reply having landed, not
    # on the modal being open (an open modal samples an empty section).
    page.wait_for_function(f"() => {SH}.branchModal.checkouts !== null", timeout=15000)




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


def desk_records(page):
    """The saved desk, read from the DAEMON. Settles past the shell's 250 ms
    upload debounce first."""
    page.wait_for_timeout(400)
    return page.request.get(BASE + "api/desk").json()["windows"]


SWITCHER = "() => document.querySelector('.session-window .session-checkout')"
SWITCHER_TEXT = "() => (document.querySelector('.session-window .session-checkout') || {}).textContent"
MENU_ROWS = (
    "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .map(e => ({ name: e.querySelector('.session-checkout-name').textContent.trim(),"
    "    branch: (e.querySelector('.session-checkout-branch') || {}).textContent || '',"
    "    current: e.classList.contains('current') }))"
)
MENU_OPEN = "() => !!document.querySelector('.session-checkout-menu')"
CREATE_ITEM = "() => (document.querySelector('.session-checkout-menu .session-checkout-item.create') || {}).textContent"
CONFIRM = ".wb-confirm .btn.accent, .wb-confirm .btn.danger"
CONFIRM_OPEN = "() => !!document.querySelector('.wb-confirm')"


def click_menu_row(page, name):
    page.evaluate(
        "(n) => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item')]"
        "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === n).click()",
        arg=name,
    )



CREATE_ITEM = "() => document.querySelector('.session-checkout-menu .session-checkout-item.create')"
PROMPT = ".wb-worktree"
PROMPT_OPEN = "() => !!document.querySelector('.wb-worktree')"
PROMPT_STATE = (
    "() => { const p = document.querySelector('.wb-worktree'); if (!p) return null;"
    "  const base = p.querySelector('select.prompt-input, input.prompt-input:nth-of-type(2)');"
    "  return { name: p.querySelector('input.prompt-input').value,"
    "    base: base ? base.value : null,"
    "    options: base && base.tagName === 'SELECT' ? [...base.options].map(o => o.value) : [],"
    "    note: (p.querySelector('.wb-worktree-note') || {}).textContent || '',"
    "    error: (() => { const e = p.querySelector('.prompt-error'); return e && !e.hidden ? e.textContent.trim() : ''; })() }; }"
)
CARRY_NOTE = "Ignored files come along only via worktree.copy / worktree.share in settings.json."


def open_switcher_menu(page):
    page.wait_for_function(f"() => !!({SWITCHER})()", timeout=15000)
    page.evaluate(f"() => ({SWITCHER})().click()")
    page.wait_for_function(MENU_OPEN, timeout=5000)


def open_prompt(page):
    open_switcher_menu(page)
    page.evaluate(f"() => ({CREATE_ITEM})().click()")
    page.wait_for_function(PROMPT_OPEN, timeout=10000)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb405_reg_")
    fixture = seed("wb405_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))
    wt_new = fixture / ".ralphy" / "worktrees" / "wt-new"
    title_of = lambda name: f"claude · {name} · {slug} · {ENV_LABEL}"

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
            commands = []
            page.on("websocket", lambda ws: commands.append(ws) if ws.url.endswith("/ws/command") else None)
            page.goto(BASE)
            wait_shell(page)

            # --- scenario 2: the switcher before any worktree --------------------
            open_project(page, slug)
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=title_of("primary"), timeout=15000)
            check("the child printed READY", wait_flat_contains(page, 0, READY))
            open_switcher_menu(page)
            rows = page.evaluate(MENU_ROWS)
            create = page.evaluate(f"() => (({CREATE_ITEM})() || {{}}).textContent")
            check(
                "with no worktrees the menu is primary (current) + `new worktree…`",
                rows == [{"name": "primary", "branch": "", "current": True}] and "new worktree" in (create or ""),
                f"rows={rows!r} create={create!r}",
            )

            # --- scenario 3: the prompt -------------------------------------------
            page.evaluate(f"() => ({CREATE_ITEM})().click()")
            page.wait_for_function(PROMPT_OPEN, timeout=10000)
            st = page.evaluate(PROMPT_STATE)
            check(
                "the prompt's base defaults to main and lists taken",
                st and st["base"] == "main" and "taken" in st["options"],
                f"got={st!r}",
            )
            check("the note names the carry-over keys and the restart", st and CARRY_NOTE in st["note"] and "restarts" in st["note"], f"got={st!r}")

            # --- scenario 4: create wt-new → the console restarts inside -----------
            launches.clear()
            page.fill(PROMPT + " input.prompt-input", "wt-new")
            page.click(PROMPT + " .btn.accent")
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=title_of("wt-new"), timeout=30000)
            check("the console restarted with the wt-new title", True)
            check("…the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("…and its CWD: line is the new worktree", ".ralphy/worktrees/wt-new" in buf, f"buffer={buf[:200]!r}")
            check("one relaunch socket, carrying checkout=wt-new", len(launches) == 1 and "checkout=wt-new" in launches[0], f"got={launches!r}")
            recs = desk_records(page)
            check("the record reads checkout wt-new", bool(recs) and recs[0].get("checkout") == "wt-new", f"got={recs!r}")
            check("the worktree directory exists on disk", wt_new.is_dir(), str(wt_new))
            base = git(fixture, "config", "branch.wt-new.base") if wt_new.is_dir() else None
            check("branch.wt-new.base records main", base == "main", f"got={base!r}")
            page.wait_for_function(
                f"() => {{ const l = {SH}.worktreeListings; const k = Object.keys(l)[0]; return !!k && (l[k]?.worktrees || []).some(w => w.name === 'wt-new'); }}",
                timeout=15000,
            )
            check("the shell's listing shows the new worktree", True)
            check("the Files selection stays unset", page.evaluate(f"(s) => {SH}.checkoutOf(s) === null", arg=slug))
            open_switcher_menu(page)
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => !({MENU_OPEN})()", timeout=5000)

            # --- scenario 5: a taken branch is refused by the daemon, verbatim -----
            open_prompt(page)
            page.fill(PROMPT + " input.prompt-input", "taken")
            page.click(PROMPT + " .btn.accent")
            page.wait_for_function(f"() => {{ const s = ({PROMPT_STATE})(); return !!s && s.error !== ''; }}", timeout=20000)
            st = page.evaluate(PROMPT_STATE)
            check("the refusal comes back into the prompt (`already exists`)", "already exists" in (st["error"] or ""), f"got={st!r}")
            check("…with the name kept", st["name"] == "taken", f"got={st!r}")
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => !({PROMPT_OPEN})()", timeout=5000)
            check("Escape cancels the prompt", True)
            check("nothing was created for the refused name", not (fixture / ".ralphy" / "worktrees" / "taken").exists())
            check("the console is still in wt-new", page.evaluate(TITLES) == [title_of("wt-new")], f"got={page.evaluate(TITLES)!r}")

            # --- scenario 6: the prompt's own gate ---------------------------------
            open_prompt(page)
            # Counted AFTER the prompt opened: opening it reads `branch.list`.
            before = len(commands)
            page.fill(PROMPT + " input.prompt-input", "a/b")
            page.click(PROMPT + " .btn.accent")
            page.wait_for_timeout(300)
            st = page.evaluate(PROMPT_STATE)
            check("a name with a separator never leaves the prompt", st is not None and st["error"] != "" and st["name"] == "a/b", f"got={st!r}")
            check("…and no command socket was opened for it", len(commands) == before, f"opened={len(commands) - before}")
            page.keyboard.press("Escape")
            page.wait_for_function(f"() => !({PROMPT_OPEN})()", timeout=5000)
            check("nothing was created for the separator name", not (fixture / ".ralphy" / "worktrees" / "a").exists())

            # --- scenario 7: cut from another branch --------------------------------
            open_prompt(page)
            page.fill(PROMPT + " input.prompt-input", "wt-off-taken")
            page.select_option(PROMPT + " select.prompt-input", "taken")
            page.click(PROMPT + " .btn.accent")
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=title_of("wt-off-taken"), timeout=30000)
            base = git(fixture, "config", "branch.wt-off-taken.base")
            check("the worktree is cut from the picked branch", base == "taken", f"got={base!r}")
            check("…and the console restarted in it", wait_flat_contains(page, 0, READY) and ".ralphy/worktrees/wt-off-taken" in flat(page, 0).replace("\\", "/"))

            # --- scenario 8 --------------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 26
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
