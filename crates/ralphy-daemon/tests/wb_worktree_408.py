"""#408 browser acceptance: a console is born in the primary and only its own
switcher moves it; the claude --worktree experiment is retired.

One Playwright pass over a REAL daemon proving ADR-0063 §3 (as amended
2026-09-16 b) end to end: with a worktree selected for Files, `New console`
STILL opens the agent (the helper child, via `RALPHY_DAEMON_AGENT_OVERRIDE`)
in the primary — its `CWD:` line says so — and the title's switcher is what
moves it INSIDE the worktree: the title then reads `<agent> · <checkout> ·
<slug> · <env>`, the `/api/sessions` row carries the name, and neither the
title nor the row moves when the Files selection changes; a reload
re-announces it and a restart relaunches in the same tree. A console opened
under `primary` carries nothing. The retired `console_worktree` key in
`repos.toml` is logged exactly once for the daemon's whole life and dropped
by the next registry write.

Fixture: a repo on `main` with `README.md`, one worktree `wt-a` made by
`ralphy worktree add`, registered through `ralphy daemon add`, and
`console_worktree = true` appended to its `repos.toml` entry by hand.

Scenario 1  the daemon is listening
Scenario 2  `wt-a` selected for Files
Scenario 3  `newConsole('claude')` → one window titled
            `claude · primary · <slug> · Windows`, CWD the primary, a row
            with NO `checkout` — the selection is not where a console opens;
            then its switcher → `wt-a` (confirmed) → titled exactly
            `claude · wt-a · <slug> · Windows`
Scenario 4  the child's `CWD:` line ends with `.ralphy/worktrees/wt-a`
Scenario 5  `/api/sessions` has one row, `checkout == 'wt-a'`, `agent == 'claude'`
Scenario 6  selecting `primary` leaves the title and the row unchanged
Scenario 7  a reload restores the same title (the reattach re-announces it)
Scenario 8  `newConsole('codex')` under `primary` → `codex · primary · <slug> · Windows`
            (the `primary` segment is #412's switcher, present once a worktree exists)
            and a row with NO `checkout` key
Scenario 9  `quit` in the claude console, then its restart control →
            a fresh claude row with `checkout == 'wt-a'` and the same title
Scenario 10 screenshot (both windows, the wt-a title visible)
Scenario 11 no page errors
Scenario 12 the daemon's stderr carries exactly one `console_worktree` notice
Scenario 13 a second `ralphy daemon add` rewrites `repos.toml` without the key

Boots a Localhost daemon on 7455 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/408-console-worktree-2026-09-15.png.
Run: python crates/ralphy-daemon/tests/wb_worktree_408.py   (exit 0 = all pass)
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

PORT = 7455
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_worktree_408.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "408-console-worktree-2026-09-15.png"
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The local environment label: `WSL_DISTRO_NAME` is unset on this host.
ENV_LABEL = "Windows"

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
    empty = tempfile.mkdtemp(prefix="wb408_empty_")
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
    (d / ".gitignore").write_text(".ralphy/\n", encoding="utf-8")
    (d / "README.md").write_text(f"# {name}\n\nThe #408 console fixture repo.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb408@example.com")
    git(d, "config", "user.name", "wb408")
    git(d, "config", "core.autocrlf", "false")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return d


def add_worktree(fixture):
    """`ralphy worktree add wt-a` (the CLI, not git directly)."""
    subprocess.run(
        [EXE, "worktree", "add", "wt-a", "--repo", str(fixture)],
        check=True,
        capture_output=True,
        encoding="utf-8",
    )
    return fixture / ".ralphy" / "worktrees" / "wt-a"


def register_fixture(daemon_dir, fixture_dir):
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir)
    result = subprocess.run(
        [EXE, "daemon", "add", fixture_dir], env=env, check=True, capture_output=True, encoding="utf-8"
    )
    # stdout: "registered <slug> → <path>"; the arrow is U+2192, so decode utf-8.
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's panel.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


def launch(daemon_dir, log):
    # stderr to a FILE: the retired-key notice is a `tracing::warn!` line, and
    # "logged once" is a count over the daemon's whole life.
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=log,
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
# Every console window's title, trimmed: the titlebar's innerHTML starts with
# the icon's leading space.
TITLES = "() => [...document.querySelectorAll('.session-title')].map(e => e.textContent.trim())"
WINDOWS = "() => document.querySelectorAll('.session-window').length"


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
    page.evaluate("() => document.querySelector('li.project.open .files-sec .branch-chip').click()")
    page.wait_for_function(f"() => {SH}.branchOpen === true", timeout=10000)
    # `branch.list` arrives by round trip: gate on the real list having landed.
    page.wait_for_function(f"() => {SH}.branchModal.branches.length >= 1", timeout=15000)


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


def window_index_by_title(page, title):
    titles = page.evaluate(TITLES)
    return titles.index(title) if title in titles else -1


def wait_for_row(pred, timeout=15):
    """Poll `/api/sessions` until a row satisfies `pred`; returns it or None."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        for row in sessions():
            if pred(row):
                return row
        time.sleep(0.25)
    return None


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb408_reg_")
    fixture = seed("wb408_", "plain")
    add_worktree(fixture)
    slug = register_fixture(daemon_dir, str(fixture))
    expected = f"claude · wt-a · {slug} · {ENV_LABEL}"
    expected_primary = f"claude · primary · {slug} · {ENV_LABEL}"
    # With a worktree in the repo the title's segment exists and reads
    # `primary` for a console on the primary tree — it is the switcher (#412).
    expected_codex = f"codex · primary · {slug} · {ENV_LABEL}"

    # The retired key, hand-appended to the ONLY entry's table.
    registry = Path(daemon_dir, "repos.toml")
    registry.write_bytes(registry.read_bytes() + b"console_worktree = true\n")
    assert registry.read_text(encoding="utf-8").count("console_worktree") == 1

    log_path = Path(daemon_dir, "daemon.log")
    log = open(log_path, "wb")
    proc = launch(daemon_dir, log)
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

            # --- scenario 2: select wt-a ---------------------------------------
            click_row(page, "wt-a", slug)
            page.wait_for_function(f"(s) => {SH}.checkouts[s] === 'wt-a'", arg=slug, timeout=10000)
            check("wt-a is the selected checkout", True)

            # --- scenario 3: born in the primary; the switcher moves it ---------
            page.evaluate(f"() => {SH}.newConsole('claude')")
            page.wait_for_function(f"() => ({WINDOWS})() === 1", timeout=15000)
            # The title lands on `session-open`, not on window creation.
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=expected_primary, timeout=15000)
            check("the child printed READY on the primary", wait_flat_contains(page, 0, "READY"))
            buf = flat(page, 0).replace("\\", "/")
            born = sessions()
            check(
                "New console ignores the Files selection: CWD is the primary, the row has no checkout",
                ".ralphy/worktrees" not in buf and len(born) == 1 and "checkout" not in born[0],
                f"buffer={buf[:120]!r} rows={born!r}",
            )
            page.evaluate("() => document.querySelector('.session-window .session-checkout').click()")
            page.wait_for_function("() => !!document.querySelector('.session-checkout-menu')", timeout=5000)
            page.evaluate(
                "() => [...document.querySelectorAll('.session-checkout-menu .session-checkout-item')]"
                "  .find(e => e.querySelector('.session-checkout-name').textContent.trim() === 'wt-a').click()"
            )
            page.wait_for_selector(".wb-confirm", timeout=5000)
            page.locator(".wb-confirm .btn.accent").click()
            page.wait_for_function(f"(t) => ({TITLES})()[0] === t", arg=expected, timeout=15000)
            titles = page.evaluate(TITLES)
            check("after the switch the console title reads `claude · wt-a · <slug> · Windows` exactly", titles == [expected], f"got={titles!r}")

            # --- scenario 4: the child runs IN the worktree --------------------
            check("the child printed READY", wait_flat_contains(page, 0, "READY"))
            buf = flat(page, 0).replace("\\", "/")
            check("the child's CWD: line is the worktree", ".ralphy/worktrees/wt-a" in buf, f"buffer={buf[:200]!r}")

            # --- scenario 5: the sessions row carries the name -----------------
            rows = sessions()
            check(
                "/api/sessions has one claude row with checkout == 'wt-a'",
                len(rows) == 1 and rows[0].get("checkout") == "wt-a" and rows[0].get("agent") == "claude",
                f"rows={rows!r}",
            )
            first_id = rows[0]["id"] if rows else None

            # --- scenario 6: the selection changes, the console does not -------
            click_row(page, "primary", slug)
            page.wait_for_function(f"(s) => {SH}.checkoutOf(s) === null", arg=slug, timeout=10000)
            page.wait_for_timeout(500)
            titles = page.evaluate(TITLES)
            check("selecting primary leaves the live console's title unchanged", titles == [expected], f"got={titles!r}")
            rows = sessions()
            check("…and its /api/sessions row still says wt-a", len(rows) == 1 and rows[0].get("checkout") == "wt-a", f"rows={rows!r}")

            # --- scenario 7: a reload re-announces the checkout ----------------
            page.reload()
            wait_shell(page)
            page.wait_for_function(f"(t) => ({TITLES})().includes(t)", arg=expected, timeout=20000)
            check("after a reload the reattached console is titled with wt-a again", True)

            # --- scenario 8: a console under primary carries nothing -----------
            open_project(page, slug)
            page.evaluate(f"() => {SH}.newConsole('codex')")
            page.wait_for_function(f"() => ({WINDOWS})() === 2", timeout=15000)
            page.wait_for_function(f"(t) => ({TITLES})().includes(t)", arg=expected_codex, timeout=15000)
            check("a codex console opened under primary reads `codex · primary · <slug> · Windows`", True)
            codex = wait_for_row(lambda r: r.get("agent") == "codex")
            check("its /api/sessions row has NO checkout key", codex is not None and "checkout" not in codex, f"row={codex!r}")

            # --- scenario 9: a restart lands in the same tree ------------------
            i = window_index_by_title(page, expected)
            check("the claude window is still on the desk", i >= 0, f"titles={page.evaluate(TITLES)!r}")
            page.evaluate(
                "([i]) => document.querySelectorAll('.session-window')[i]._term.term.paste('quit\\r')",
                [i],
            )
            page.wait_for_function(
                "([i]) => { const w = document.querySelectorAll('.session-window')[i];"
                " return !!w && w.classList.contains('ended'); }",
                arg=[i],
                timeout=20000,
            )
            page.evaluate("([i]) => document.querySelectorAll('.session-window')[i].querySelector('.session-restart').click()", [i])
            page.wait_for_selector(".wb-confirm", timeout=5000)
            page.locator(".wb-confirm .btn.accent").click()
            fresh = wait_for_row(lambda r: r.get("agent") == "claude" and r.get("id") != first_id)
            check("the restart spawned a fresh claude session", fresh is not None, f"row={fresh!r}")
            check("…whose row says wt-a again", fresh is not None and fresh.get("checkout") == "wt-a", f"row={fresh!r}")
            page.wait_for_function(f"(t) => ({TITLES})().filter(x => x === t).length === 1", arg=expected, timeout=15000)
            check("…and whose window is titled with wt-a again", True)
            check("the restarted child runs in the worktree", wait_flat_contains(page, window_index_by_title(page, expected), "READY")
                  and ".ralphy/worktrees/wt-a" in flat(page, window_index_by_title(page, expected)).replace("\\", "/"))

            # --- scenario 10 ----------------------------------------------------
            # The restarted window carries the original's rect, which the codex
            # window shares; nudge codex aside so BOTH titles are in the frame.
            page.evaluate(
                "([t]) => { const w = [...document.querySelectorAll('.session-window')]"
                "  .find(w => w.querySelector('.session-title').textContent.trim() === t);"
                "  if (w) { w.style.left = '640px'; w.style.top = '420px'; } }",
                [expected_codex],
            )
            page.wait_for_timeout(300)
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            check("screenshot written", os.path.exists(os.path.join(SHOT_DIR, SHOT)))

            # --- scenario 11 ----------------------------------------------------
            check("no page errors were thrown", not thrown, f"got={thrown}")
            browser.close()
    finally:
        stop(proc)
        log.close()

    # --- scenario 12: the retired key is logged exactly once --------------------
    text = log_path.read_text(encoding="utf-8", errors="replace")
    check(
        "the retired console_worktree key is logged exactly once for the daemon's life",
        text.count("console_worktree") == 1,
        f"count={text.count('console_worktree')}",
    )

    # --- scenario 13: the next registry write drops the key ---------------------
    subprocess.run(
        [EXE, "daemon", "add", str(fixture)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
    )
    rewritten = registry.read_text(encoding="utf-8")
    check(
        "a re-`daemon add` rewrites repos.toml without console_worktree",
        rewritten.count("console_worktree") == 0 and "path = " in rewritten,
        f"file={rewritten!r}",
    )

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # A deleted scenario must not silently shrink the suite (#339 trap).
    check_floor = 22
    if len(results) != check_floor:
        print(f"[FAIL] the suite ran {len(results)} checks, expected {check_floor}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
