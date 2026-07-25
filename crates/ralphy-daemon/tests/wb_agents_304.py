"""#304 browser acceptance: the console menu serves the daemon's adapter roster with live sessions.

One Playwright pass over a REAL daemon: real `/api/agents`, real sessions, real
PTYs. Nothing is stubbed — the fixtures are a throwaway git repo registered with
the daemon and `RALPHY_DAEMON_AGENT_OVERRIDE` pointing every "vendor" launch at
the session test-child bin, so an agent console spawns the helper instead of a
vendor CLI (no quota, no install requirement).

Scenario 1   `GET /api/agents` from the page: 7 rows, claude→"1" … gemini→"7"
Scenario 2   with NO repo selected the menu renders the 7 roster labels + console
             last, agent rows `disabled` with the verbatim "select a repo first…"
Scenario 3   selecting the fixture repo enables them
Scenario 4   a live claude console makes its row read "1 live"; clicking the row
             REACHES it — no new session, and the only socket opened carries `id=`
Scenario 5   the row's "+" launches a second session — 2 sessions, row reads "2 live"
Scenario 6   `Alt+Shift+Digit2` still opens a `/ws/session?…agent=codex` console
Scenario 7   `Alt+Shift+Digit1` on a LIVE row REACHES it — no duplicate session
Scenario 8   a disabled row's accelerator is inert (no repo selected)
Scenario 9   a 500 from `/api/agents` in DAEMON mode leaves the roster empty —
             the demo seed is never shown to a daemon that cannot answer
Scenario 10  the plain console row never advertises `attach`

Boots a Localhost daemon on 7399 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host). Every session is closed before the daemon stops, so
no helper child is left behind.

Writes docs/screenshots/304-agent-menu-2026-07-25.png.
Run: python crates/ralphy-daemon/tests/wb_agents_304.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

# The Windows console's default codepage (cp1252 here) cannot encode the glyphs
# this script prints in its detail strings; force utf-8 stdout so a PASSING
# assertion never dies on its own detail.
sys.stdout.reconfigure(encoding="utf-8")

PORT = 7399
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_agents_304.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
WIN = os.name == "nt"
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if WIN else "ralphy")
CHILD = os.path.join(REPO_ROOT, "target", "debug", "session_test_child.exe" if WIN else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
# The account dropdown reuses `.dropdown-item`, so every menu query is scoped.
MENU = ".console-menu"

# The daemon's roster, as this issue pins it (roster.rs::accelerators_are_unique_and_stable).
EXPECTED = [
    ("claude", "1"),
    ("codex", "2"),
    ("opencode", "3"),
    ("kimi", "4"),
    ("copilot", "5"),
    ("cursor", "6"),
    ("gemini", "7"),
]
NEEDS_REPO = "select a repo first — an agent needs one to work in"

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
    """A scratch registry + empty vendor stores, and every agent launch pointed at
    the session test child: the operator's daemon dir is never touched and no
    vendor CLI is required to prove the launch path."""
    empty = tempfile.mkdtemp(prefix="wb304_empty_")
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


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb304_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #304 agent-menu fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb304@example.com"],
        ["git", "config", "user.name", "wb304"],
        ["git", "add", "-A"],
        ["git", "commit", "-m", "fixture"],
    ):
        subprocess.run(args, cwd=d, check=True, capture_output=True)
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
    # after any assets/ui edit or the browser loads yesterday's console. The
    # helper child is the stand-in every agent launch resolves to.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bins"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def open_menu(page):
    """Open the New console dropdown and return its rendered rows."""
    if not page.evaluate(f"() => {SH}.agentMenu"):
        page.evaluate(f"() => {{ {SH}.agentMenu = true; }}")
    page.wait_for_timeout(250)
    return page.locator(f"{MENU} .dropdown-item")


def close_menu(page):
    page.evaluate(f"() => {{ {SH}.agentMenu = false; }}")
    page.wait_for_timeout(150)


def sessions_of(page):
    return page.request.get(BASE + "api/sessions").json()


def row_by_label(page, label):
    return page.locator(f"{MENU} .dropdown-item", has=page.locator(f"span:text-is('{label}')")).first


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb304_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

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
            sockets = []
            page.on("websocket", lambda ws: sockets.append(ws.url))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)

            # --- scenario 1: the endpoint IS the roster -----------------------
            served = page.evaluate("async () => (await fetch('/api/agents')).json()")
            check(
                "GET /api/agents serves one row per launchable adapter",
                len(served) == len(EXPECTED),
                f"got={served}",
            )
            check(
                "…with the id/label/accelerator of every adapter, in digit order",
                [(r["id"], r["accelerator"]) for r in served] == EXPECTED,
                f"got={[(r['id'], r['accelerator']) for r in served]}",
            )
            check(
                "…and no capability/availability field (the roster says what the daemon CAN launch)",
                all(sorted(r.keys()) == ["accelerator", "id", "label"] for r in served),
                f"keys={sorted(served[0].keys())}",
            )

            # --- scenario 2: the menu renders from it, disabled with no repo --
            page.evaluate(f"() => {{ {SH}.openSlug = null; }}")
            page.wait_for_timeout(200)
            rows = open_menu(page)
            labels = rows.locator("span:not(.row-live):not(.row-new)").all_inner_texts()
            check(
                "the menu renders the served roster, plain console LAST",
                labels == [r["id"] for r in served] + ["console"],
                f"got={labels}",
            )
            digits = rows.locator("kbd").all_inner_texts()
            check(
                "…each row carrying its accelerator from the daemon, console on 0",
                digits == [f"Alt+Shift+{d}" for (_, d) in EXPECTED] + ["Alt+Shift+0"],
                f"got={digits}",
            )
            disabled = page.evaluate(
                f"() => [...document.querySelectorAll('{MENU} .dropdown-item')]"
                ".map((b) => [b.disabled, b.getAttribute('title')])"
            )
            check(
                "with no repo selected every AGENT row is disabled",
                all(d[0] is True for d in disabled[:-1]),
                f"got={[d[0] for d in disabled]}",
            )
            check(
                "…and says why, verbatim",
                all(d[1] == NEEDS_REPO for d in disabled[:-1]),
                f"got={disabled[0][1]!r}",
            )
            check(
                "…while the plain console stays enabled (it falls back to the home dir)",
                disabled[-1][0] is False and (disabled[-1][1] or "") == "",
                f"got={disabled[-1]}",
            )

            # --- scenario 3: selecting a repo enables them --------------------
            close_menu(page)
            page.evaluate(f"() => {{ {SH}.openSlug = '{slug}'; {SH}.active = 'agents'; }}")
            page.wait_for_timeout(300)
            open_menu(page)
            enabled = page.evaluate(
                f"() => [...document.querySelectorAll('{MENU} .dropdown-item')]"
                ".map((b) => [b.disabled, b.getAttribute('title')])"
            )
            check(
                "selecting a repo enables every agent row",
                all(e[0] is False for e in enabled),
                f"got={[e[0] for e in enabled]}",
            )
            check(
                "…and clears the explanation",
                all((e[1] or "") == "" for e in enabled),
                f"got={[e[1] for e in enabled]}",
            )
            check(
                "no row reports a live session yet",
                page.locator(f"{MENU} .dropdown-item .row-live:visible").count() == 0,
                "",
            )

            # --- scenario 4: a live row REACHES its session -------------------
            row_by_label(page, "claude").click()
            page.wait_for_timeout(300)
            page.locator(".session-window .xterm").first.wait_for(timeout=15000)
            page.wait_for_timeout(800)
            after_launch = sessions_of(page)
            check(
                "clicking an agent row with no live session launches one",
                len(after_launch) == 1 and after_launch[0]["agent"] == "claude",
                f"got={after_launch}",
            )
            # The presence tick feeds the fold; ask for it now rather than waiting.
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(400)
            rows = open_menu(page)
            claude_row = row_by_label(page, "claude")
            check(
                "the claude row now reports its live session, and how many",
                claude_row.locator(".row-live").inner_text() == "1 live",
                f"got={claude_row.locator('.row-live').inner_text()!r}",
            )
            check(
                "…and it is the ONLY row reporting one",
                page.locator(f"{MENU} .dropdown-item .row-live:visible").count() == 1,
                f"got={page.locator('.dropdown-item .row-live:visible').count()}",
            )
            check(
                "…offering to reach it, not to launch a duplicate",
                page.evaluate(
                    f"() => {SH}.consoleItems().find((r) => r.kind === 'claude').action"
                )
                == "attach",
                "",
            )

            # `/ws` is the daemon's control channel, always opened; only
            # `/ws/session` sockets launch or attach a PTY (#303).
            mark = len([u for u in sockets if "/ws/session" in u])
            windows_before = page.locator(".session-window").count()
            claude_row.click()
            page.wait_for_timeout(1500)
            reached = sessions_of(page)
            check(
                "clicking the live row opens NO second session",
                len(reached) == 1,
                f"got={reached}",
            )
            opened = [u for u in sockets if "/ws/session" in u][mark:]
            # `all()` over an empty list would pass vacuously, and the focus
            # branch legitimately opens NOTHING — so assert the launch shape is
            # absent outright, then the two observable consequences.
            check(
                "…opening no launch socket at all",
                not any("agent=" in u for u in opened),
                f"new sockets={opened}",
            )
            check(
                "…and any socket it did open attaches by id",
                all("id=" in u for u in opened),
                f"new sockets={opened}",
            )
            check(
                "…reusing the window already holding that session, and focusing it",
                page.locator(".session-window").count() == windows_before
                and page.evaluate(
                    "() => { const ws = [...document.querySelectorAll('.session-window')];"
                    " const top = ws.reduce((a, b) =>"
                    "   (+b.style.zIndex || 0) >= (+a.style.zIndex || 0) ? b : a);"
                    " return top._term?.sessionId; }"
                )
                == 1,
                f"{windows_before} -> {page.locator('.session-window').count()}",
            )

            # --- scenario 5: the "+" escape hatch still launches --------------
            rows = open_menu(page)
            page.screenshot(path=os.path.join(SHOT_DIR, "304-agent-menu-2026-07-25.png"))
            row_by_label(page, "claude").locator(".row-new").click()
            page.wait_for_timeout(300)
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window .xterm').length === 2",
                timeout=15000,
            )
            page.wait_for_timeout(800)
            two = sessions_of(page)
            check(
                "the row's + launches a second console anyway",
                len(two) == 2 and all(s["agent"] == "claude" for s in two),
                f"got={two}",
            )
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(400)
            open_menu(page)
            check(
                "…and the row counts both",
                row_by_label(page, "claude").locator(".row-live").inner_text() == "2 live",
                f"got={row_by_label(page, 'claude').locator('.row-live').inner_text()!r}",
            )

            # --- scenario 6: the accelerators keep their digits ---------------
            close_menu(page)
            mark = len([u for u in sockets if "/ws/session" in u])
            page.keyboard.press("Alt+Shift+Digit2")
            page.wait_for_timeout(300)
            page.wait_for_function(
                "() => document.querySelectorAll('.session-window .xterm').length === 3",
                timeout=15000,
            )
            page.wait_for_timeout(800)
            opened = [u for u in sockets if "/ws/session" in u][mark:]
            check(
                "Alt+Shift+2 still opens a codex console",
                len(opened) == 1 and "agent=codex" in opened[0] and f"repo={slug}" in opened[0],
                f"new sockets={opened}",
            )
            check(
                "…and the daemon really started it",
                any(s["agent"] == "codex" for s in sessions_of(page)),
                f"got={sessions_of(page)}",
            )

            # --- scenario 7: the accelerator takes its ROW's action ------------
            # The keyboard is the path that could reintroduce the duplicate the
            # menu refuses, so assert it on a row that is already live.
            mark = len([u for u in sockets if "/ws/session" in u])
            windows_before = page.locator(".session-window").count()
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(400)
            page.keyboard.press("Alt+Shift+Digit1")
            page.wait_for_timeout(1500)
            check(
                "Alt+Shift+1 on a LIVE claude row reaches it — no fourth session",
                len(sessions_of(page)) == 3,
                f"got={sessions_of(page)}",
            )
            opened = [u for u in sockets if "/ws/session" in u][mark:]
            check(
                "…opening no launch socket, and no new window",
                not any("agent=" in u for u in opened)
                and page.locator(".session-window").count() == windows_before,
                f"new sockets={opened}",
            )

            # --- scenario 8: a DISABLED row's accelerator is inert -------------
            page.evaluate(f"() => {{ {SH}.openSlug = null; }}")
            page.wait_for_timeout(300)
            mark = len([u for u in sockets if "/ws/session" in u])
            windows_before = page.locator(".session-window").count()
            page.keyboard.press("Alt+Shift+Digit3")
            page.wait_for_timeout(1200)
            check(
                "with no repo selected an agent accelerator launches nothing",
                len(sessions_of(page)) == 3
                and [u for u in sockets if "/ws/session" in u][mark:] == []
                and page.locator(".session-window").count() == windows_before,
                f"sessions={len(sessions_of(page))}",
            )
            page.evaluate(f"() => {{ {SH}.openSlug = '{slug}'; }}")
            page.wait_for_timeout(200)

            # --- scenario 9: a FAILED /api/agents in DAEMON mode shows nothing -
            # The demo seed is for `file://` only; a daemon that cannot answer
            # must not have adapters invented for it.
            page.route("**/api/agents", lambda route: route.fulfill(status=500, body="nope"))
            page.evaluate(f"async () => {SH}.loadAgents()")
            page.wait_for_timeout(400)
            check(
                "a 500 from /api/agents leaves the roster EMPTY in daemon mode",
                page.evaluate(f"() => [{SH}.roster.length, {SH}.agents.length]") == [0, 0],
                f"got={page.evaluate(f'() => {SH}.roster')}",
            )
            open_menu(page)
            check(
                "…so the menu offers the plain console alone, never the demo seed",
                page.locator(f"{MENU} .dropdown-item").count() == 1
                and page.locator(f"{MENU} .dropdown-item span").first.inner_text() == "console",
                f"rows={page.locator(f'{MENU} .dropdown-item').all_inner_texts()}",
            )
            close_menu(page)
            page.unroute("**/api/agents")
            page.evaluate(f"async () => {SH}.loadAgents()")
            page.wait_for_timeout(400)

            # --- scenario 10: the plain console row never claims to reach ------
            # It counts its live shells but always launches; a row that said
            # "attach" while the click launched would lie about its own click.
            page.evaluate(f"async () => {SH}.refreshLive()")
            page.wait_for_timeout(300)
            plain = page.evaluate(
                f"() => {SH}.consoleItems().find((r) => r.plain)"
            )
            check(
                "the plain console row always launches, and offers no session to reach",
                plain["action"] == "launch" and plain["sessionId"] is None,
                f"got={plain}",
            )

            # --- teardown: leave no helper child behind -----------------------
            for s in sessions_of(page):
                page.request.post(BASE + f"api/sessions/close?id={s['id']}")
            page.wait_for_timeout(600)
            check("every session closed before the daemon stops", sessions_of(page) == [], "")

            ctx.close()
            browser.close()
    finally:
        stop(proc)

    # The count floor is load-bearing: an early `sys.exit` or a scenario that
    # never ran must not report success on a handful of passing checks.
    ok = all(results) and len(results) >= 31
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    if ok:
        print("AGENT ROSTER")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
