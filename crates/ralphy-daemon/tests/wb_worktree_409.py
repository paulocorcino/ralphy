"""#409 browser acceptance: remove a worktree from the branch picker behind
every gate — live console, dirty, locked, branch kept — and the clean success.

One Playwright pass over a REAL daemon proving ADR-0063 §1/§2/§4 end to end:
a worktree created from the picker, selected, and a console opened in it
(the helper child via `RALPHY_DAEMON_AGENT_OVERRIDE`); the row's remove
action is then refused IN the daemon (`has a live console`) while that console
lives, by the CLI (`has uncommitted changes`, `is locked`) once it is closed,
and finally removes the directory — keeping the branch when it carries a
commit beyond its base (`branch 'wt-r' kept`), deleting it when clean. Every
refusal's message lands verbatim in the picker; the row and the selection
reset from the re-read listing, never from the reply's status.

Fixture: a repo on `main` with `README.md`, NO pre-made worktree (the picker
creates `wt-r` and later `wt-s`), registered through `ralphy daemon add`.

Scenario 1  the daemon is listening
Scenario 2  create `wt-r` from the picker → rows primary + wt-r, directory exists
Scenario 3  select `wt-r`
Scenario 4  `newConsole('claude')` titled `claude · wt-r · <slug> · <env>`,
            the child's `CWD:` inside `.ralphy/worktrees/wt-r`
Scenario 5  remove refused: `has a live console` — a notice with ONE button
            (OK), rows still 2, directory stays, one session row, selection
            kept; screenshot HERE
Scenario 6  close the console → `/api/sessions` empty
Scenario 7  dirty refusal: `has uncommitted changes`, rows 2, directory stays
Scenario 8  locked refusal: `is locked`
Scenario 9  a commit inside `wt-r`, then remove → `branch 'wt-r' kept`, row
            gone, directory gone, branch still exists, selection reset
Scenario 10 create `wt-s`, select, remove → no error, row gone, directory and
            branch gone, selection reset, chip reads `main`
Scenario 11 no page errors

Boots a Localhost daemon on 7456 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Portability knobs: `RALPHY_TEST_SKIP_BUILD=1` skips the cargo builds and
`RALPHY_TEST_TARGET_DIR=<dir>` (relative to the repo root) names where the
binaries are — the Linux leg builds in one container and runs here in another.

Writes docs/screenshots/409-worktree-remove-2026-09-15.png (`-linux` off Windows).
Run: python crates/ralphy-daemon/tests/wb_worktree_409.py   (exit 0 = all pass)
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

PORT = 7456
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_409.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, os.environ.get("RALPHY_TEST_TARGET_DIR", os.path.join("target", "debug")))
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "409-worktree-remove-2026-09-15" + ("" if os.name == "nt" else "-linux") + ".png"
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The local environment label, as `peer::environment_label` spells it.
_distro = os.environ.get("WSL_DISTRO_NAME")
ENV_LABEL = f"WSL: {_distro}" if _distro else ("Windows" if os.name == "nt" else "Linux")
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
    empty = tempfile.mkdtemp(prefix="wb409_empty_")
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
    (d / "README.md").write_bytes(f"# {name}\n\nThe #409 remove fixture repo.\n".encode())
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb409@example.com")
    git(d, "config", "user.name", "wb409")
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
WORKTREE_COUNT_IS = f"(n) => (({SH}.worktreeListings[{SH}.openSlug] || {{}}).worktrees || []).length === n"
CHIP_TEXT = (
    "() => ((document.querySelector('li.project.open .project-head .branch-chip-name') || {}).textContent || '')"
    "  .trim()"
)
# Every console window's title, trimmed.
TITLES = "() => [...document.querySelectorAll('.session-title')].map(e => e.textContent.trim())"
WINDOWS = "() => document.querySelectorAll('.session-window').length"
# The refusal as a one-button notice (`askNotice`): its body, and whether the
# foot has exactly ONE button reading OK.
NOTICE = ".wb-confirm .confirm-modal"
NOTICE_STATE = (
    "() => { const m = document.querySelector('.wb-confirm .confirm-modal'); if (!m) return null;"
    "  const btns = [...m.querySelectorAll('.modal-foot button')].map(b => b.textContent.trim());"
    "  return { title: m.querySelector('.modal-title').textContent.trim(),"
    "    body: m.querySelector('.confirm-body').textContent.trim(), buttons: btns }; }"
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


# --- the Files bar's checkout chip and its menu (ADR-0063 amendment 2026-09-16 b) ---
CK_CHIP = "li.project.open .files-sec .checkout-chip"
CK_VISIBLE = "() => { const c = document.querySelector('li.project.open .files-sec .checkout-chip'); return !!c && c.offsetParent !== null && c.clientWidth > 0; }"
CK_MENU_OPEN = "() => !!document.querySelector('.session-checkout-menu')"
CK_ITEM = (
    "(n) => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === n) || null"
)
CK_ROWS = (
    "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item:not(.create)')]"
    "  .map(e => ({ name: e.querySelector('.session-checkout-name').textContent.trim(),"
    "    branch: (e.querySelector('.session-checkout-branch') || {}).textContent || '',"
    "    dot: !!e.querySelector('.session-checkout-dirty'),"
    "    state: (() => { const s = e.querySelector('.session-checkout-state'); return s ? [...s.classList].find(c => c !== 'session-checkout-state') || null : null; })(),"
    "    current: e.classList.contains('current') }))"
)


def open_chip(page, slug):
    """Open the project and its checkout chip's menu; the chip exists once the
    listing answered with a worktree, so the wait is the listing's."""
    page.evaluate(f"(s) => {{ if ({SH}.openSlug !== s) {SH}.toggle(s); }}", arg=slug)
    page.wait_for_function(f"(s) => {SH}.openSlug === s", arg=slug, timeout=15000)
    page.wait_for_function(CK_VISIBLE, timeout=15000)
    if not page.evaluate(CK_MENU_OPEN):
        page.evaluate(f"() => document.querySelector('{CK_CHIP}').click()")
        page.wait_for_function(CK_MENU_OPEN, timeout=5000)


def close_chip(page):
    if page.evaluate(CK_MENU_OPEN):
        page.keyboard.press("Escape")
        page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)


def chip_rows(page, slug):
    open_chip(page, slug)
    rows = page.evaluate(CK_ROWS)
    close_chip(page)
    return rows


def click_row(page, name, slug=None):
    """Pick a checkout from the chip's menu (the menu closes on the pick)."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CK_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CK_ITEM})(n).click()", arg=name)
    page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)


def click_remove(page, name, slug=None):
    """The trash on a row of the chip's menu; waits for the re-read to land."""
    open_chip(page, slug or page.evaluate(f"() => {SH}.openSlug"))
    page.wait_for_function(f"(n) => !!({CK_ITEM})(n)", arg=name, timeout=15000)
    page.evaluate(f"(n) => ({CK_ITEM})(n).querySelector('.session-checkout-remove').click()", arg=name)
    page.wait_for_function(f"() => !({CK_MENU_OPEN})()", timeout=5000)
    page.wait_for_function(f"() => Object.keys({SH}.worktreeRemoving).length === 0", timeout=20000)






def wait_settled(page):
    """`worktreeRemoving` is cleared only after the listing re-read landed, so
    this is the value to gate on before anyone counts rows (#405 trap)."""
    page.wait_for_function(f"() => Object.keys({SH}.worktreeRemoving).length === 0", timeout=15000)


def refusal(page):
    """Wait for the notice, read it, dismiss it with its one OK, and let the
    re-read settle. `err` is the body; `buttons` what the foot offered."""
    page.wait_for_function(f"() => !!({NOTICE_STATE})()", timeout=15000)
    st = page.evaluate(NOTICE_STATE)
    page.locator(NOTICE + " .modal-foot button").click()
    page.wait_for_function(f"() => !({NOTICE_STATE})()", timeout=5000)
    wait_settled(page)
    return {"err": st["body"], "title": st["title"], "buttons": st["buttons"]}


def add_worktree(page, fixture, slug, name):
    """Cut a worktree the way the console's prompt does (`ralphy worktree
    add`) and re-read the shell's listing, so the chip shows it."""
    subprocess.run([EXE, "worktree", "add", name], cwd=str(fixture), check=True, capture_output=True)
    page.evaluate(f"(s) => {SH}.ensureWorktreeListing(s, true)", arg=slug)
    page.wait_for_function(
        f"(n) => (({SH}.worktreeListings[{SH}.openSlug] || {{}}).worktrees || []).some(w => w.name === n)",
        arg=name, timeout=15000,
    )


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


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb409_reg_")
    fixture = seed("wb409_", "plain")
    slug = register_fixture(daemon_dir, str(fixture))
    wt = fixture / ".ralphy" / "worktrees" / "wt-r"
    expected_title = f"claude · wt-r · {slug} · {ENV_LABEL}"

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
            page.goto(BASE)
            wait_shell(page)

            # --- scenario 2: cut wt-r; the chip's menu lists it -------------------
            open_project(page, slug)
            add_worktree(page, fixture, slug, "wt-r")
            rows = [{"name": r["name"], "branch": r["branch"]} for r in chip_rows(page, slug)]
            check(
                "the chip's menu reads primary + wt-r · wt-r",
                rows == [{"name": "primary", "branch": "main"}, {"name": "wt-r", "branch": "wt-r"}],
                f"got={rows!r}",
            )
            check("the wt-r directory exists", wt.is_dir(), str(wt))

            # --- scenario 3: select wt-r -----------------------------------------
            click_row(page, "wt-r", slug)
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-r'", arg=slug, timeout=10000)
            check("wt-r is the selected checkout", True)

            # --- scenario 4: a console moved into wt-r by its switcher ----------
            # (a console is born in the primary; the Files selection is not
            # where it opens — ADR-0063 amendment 2026-09-16 b)
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            check("the child printed READY on the primary", wait_flat_contains(page, 0, READY))
            page.evaluate("() => document.querySelector('.session-window .session-checkout').click()")
            page.wait_for_function("() => !!document.querySelector('.session-checkout-menu')", timeout=5000)
            page.evaluate(
                "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item')]"
                "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === 'wt-r').click()"
            )
            page.wait_for_selector(".wb-confirm", timeout=5000)
            page.locator(".wb-confirm .btn.accent").click()
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=expected_title, timeout=15000)
            titles = page.evaluate(TITLES)
            check("the console title reads `claude · wt-r · <slug> · <env>` exactly", titles == [expected_title], f"got={titles!r}")
            check("the child printed READY", wait_flat_contains(page, 0, READY))
            buf = flat(page, 0).replace("\\", "/")
            check("the child's CWD: line is the worktree", ".ralphy/worktrees/wt-r" in buf, f"buffer={buf[:200]!r}")

            # --- scenario 5: refused while the console lives ---------------------
            click_remove(page, "wt-r", slug)
            # The notice, on screen, is the screenshot: the refusal as the
            # operator meets it.
            page.wait_for_function(f"() => !!({NOTICE_STATE})()", timeout=15000)
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            r = refusal(page)
            check("remove is refused with `has a live console`", "has a live console" in (r["err"] or ""), f"got={r['err']!r}")
            check("the refusal is a notice with ONE button, OK, titled for the worktree", r["buttons"] == ["OK"] and r["title"] == "Could not delete worktree wt-r", f"got={r!r}")
            rows = chip_rows(page, slug)
            check("the row stays (primary + wt-r)", [x["name"] for x in rows] == ["primary", "wt-r"], f"got={rows!r}")
            check("the directory stays", wt.is_dir())
            check("/api/sessions still has one row", len(sessions()) == 1, f"rows={sessions()!r}")
            check("a refusal keeps the selection", page.evaluate(f"(s) => {SH}.checkoutOf(s) === 'wt-r'", arg=slug))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))

            # --- scenario 6: close the console -----------------------------------
            page.locator(".session-window .session-close").click()
            # The console plane asks first (#334): answer it.
            page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent").click()
            page.wait_for_function(f"() => ({WINDOWS})() === 0", timeout=15000)
            check("after the close /api/sessions is empty", wait_sessions_empty(), f"rows={sessions()!r}")

            # --- scenario 7: dirty refusal ---------------------------------------
            scratch = wt / "scratch.txt"
            scratch.write_bytes(b"dirty\n")
            click_remove(page, "wt-r", slug)
            r = refusal(page)
            check("a dirty worktree is refused with `has uncommitted changes`", "has uncommitted changes" in (r["err"] or ""), f"got={r['err']!r}")
            rows = chip_rows(page, slug)
            check("…the row stays", [x["name"] for x in rows] == ["primary", "wt-r"], f"got={rows!r}")
            check("…the directory stays", wt.is_dir())

            # --- scenario 8: locked refusal --------------------------------------
            os.remove(scratch)
            git(fixture, "worktree", "lock", "--reason", "held", ".ralphy/worktrees/wt-r")
            click_remove(page, "wt-r", slug)
            r = refusal(page)
            check("a locked worktree is refused with `is locked`", "is locked" in (r["err"] or ""), f"got={r['err']!r}")
            check("…the directory stays", wt.is_dir())
            git(fixture, "worktree", "unlock", ".ralphy/worktrees/wt-r")

            # --- scenario 9: a commit beyond the base — directory goes, branch kept
            (wt / "feature.txt").write_bytes(b"x\n")
            git(wt, "add", "-A")
            git(wt, "commit", "-m", "beyond")
            click_remove(page, "wt-r", slug)
            r = refusal(page)
            check("the reply says `branch 'wt-r' kept`", "branch 'wt-r' kept" in (r["err"] or ""), f"got={r['err']!r}")
            page.wait_for_function(WORKTREE_COUNT_IS, arg=0, timeout=15000)
            check("the re-read listing has no worktree; the chip is gone", not page.evaluate(CK_VISIBLE))
            check("the directory is gone", not wt.exists())
            check("the branch wt-r still exists", git(fixture, "branch", "--list", "wt-r") != "")
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            check("the selection reset to primary", True)

            # --- scenario 10: the clean success ----------------------------------
            wt_s = fixture / ".ralphy" / "worktrees" / "wt-s"
            add_worktree(page, fixture, slug, "wt-s")
            check("wt-s is cut from main", git(fixture, "config", "branch.wt-s.base") == "main")
            click_row(page, "wt-s", slug)
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-s'", arg=slug, timeout=10000)
            click_remove(page, "wt-s", slug)
            page.wait_for_function(WORKTREE_COUNT_IS, arg=0, timeout=15000)
            wait_settled(page)
            check("a clean remove opens no notice", page.evaluate(f"() => !({NOTICE_STATE})()"))
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            check("the selection reset to primary", True)
            check("the wt-s directory is gone", not wt_s.exists())
            check("the branch wt-s is gone", git(fixture, "branch", "--list", "wt-s") == "")
            page.wait_for_function(f"() => ({CHIP_TEXT})() === 'main'", timeout=10000)
            check("the chip reads `main`", True)

            # --- scenario 11 ----------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 33
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
