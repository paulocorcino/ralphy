"""The page says why it closes a console socket (the `detach` command) browser
acceptance.

The daemon logs one `console socket traffic` line when a console socket
closes. Its `end` field tells a network drop from a close the page chose, so
the replay bytes of a reattach can be put on the right cause. This pass reads
those lines from the daemon's own stderr.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7465. The desk holds
two plain consoles 6000 px apart. The view lands on the newest one, so the
other one is off the viewport and sleeps.

D1  the console off the viewport sleeps, and the `dormant` line names its session
D2  a resume with a stale verdict reopens the awake console: `reconnect`
D3  closing the page closes the last socket with a bare Close: `client-closed`
D4  no line of this pass says `dropped`

A second browser loads the same desk, now with a session in each record. It
has no stored view, so the view lands on the middle of the two windows and
neither is visible:
B1  both consoles start asleep, with no terminal
B2  they open no socket: after the grace period no line names their sessions
B3  brought into view, a console wakes and attaches

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_detach.py   (exit 0 = all pass)
"""

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7465
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
VIEW = {"width": 1600, "height": 900}
# `DORMANT_AFTER_MS` in wb-console.ts, plus the time to close and log.
SLEEP_WAIT_S = 25
FLOOR = 11  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    out = ""
    for ts, (wid, left) in enumerate([("w-near", 40), ("w-far", 6000)], start=100):
        rect = {"left": left, "top": 40, "width": 600, "height": 360}
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f'consoleName = "{wid}"\n'
            f"ts = {ts}\n"
            f"{T.rect_toml(rect)}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def launch(daemon_dir, log_path):
    env = dict(T.empty_env(daemon_dir), RUST_LOG="info")
    log = open(log_path, "wb")
    return subprocess.Popen(
        [T.EXE, "daemon", "--port", str(PORT)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=log,
    )


def traffic_lines(log_path):
    text = Path(log_path).read_text(encoding="utf-8", errors="replace")
    return [line for line in text.splitlines() if "console socket traffic" in line]


def ends(log_path):
    out = []
    for line in traffic_lines(log_path):
        for word in line.split():
            if word.startswith("end="):
                out.append(word[len("end=") :].strip('"'))
    return out


def session_of(log_path, end):
    for line in traffic_lines(log_path):
        if f'end="{end}"' in line:
            for word in line.split():
                if word.startswith("session="):
                    return int(word[len("session=") :])
    return None


STATE = """() => [...document.querySelectorAll('#stage .session-window')].map((w) => ({
  id: w._deskId,
  dormant: w.classList.contains('dormant'),
  session: w._term?.sessionId ?? w._dormantSession ?? null,
}))"""


OLD_SOCKETS = """() => { window.__oldSockets = [...document.querySelectorAll('#stage .session-window')]
  .map((w) => w._term?.ws).filter(Boolean); }"""

NEW_SOCKET_OPEN = """() => [...document.querySelectorAll('#stage .session-window')]
  .filter((w) => !w.classList.contains('dormant'))
  .every((w) => w._term?.ws && !window.__oldSockets.includes(w._term.ws) && w._term.ws.readyState === 1)"""


def sessions_since(log_path, n):
    out = []
    for line in traffic_lines(log_path)[n:]:
        for word in line.split():
            if word.startswith("session="):
                out.append(int(word[len("session=") :]))
    return out


def wait_for_end(log_path, end, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if end in ends(log_path):
            return True
        time.sleep(0.5)
    return False


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbdetach_daemon_")
    log_path = os.path.join(daemon_dir, "daemon-stderr.log")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    write_desk(daemon_dir, slug)
    proc = launch(daemon_dir, log_path)
    errors = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page, want=2)
            page.wait_for_function(
                "() => [...document.querySelectorAll('#stage .session-window')]"
                ".every((w) => w._term?.term?.cols > 0)",
                timeout=30000,
            )

            # D1 ---------------------------------------------------------------
            slept = wait_for_end(log_path, "dormant", SLEEP_WAIT_S)
            check("D1 a console off the viewport sleeps and says dormant", slept, str(ends(log_path)))
            state = page.evaluate(STATE)
            asleep = [w for w in state if w["dormant"]]
            awake = [w for w in state if not w["dormant"]]
            check("D1 one window is dormant on the page", len(asleep) == 1, str(state))
            check(
                "D1 the dormant line names that window's session",
                bool(asleep) and session_of(log_path, "dormant") == asleep[0]["session"],
                f"{session_of(log_path, 'dormant')} {state}",
            )

            # D2 ---------------------------------------------------------------
            page.evaluate(OLD_SOCKETS)
            page.evaluate("() => window.WBConsole.resumeAll(true)")
            check(
                "D2 a stale resume reopens the awake console and says reconnect",
                wait_for_end(log_path, "reconnect", 10),
                str(ends(log_path)),
            )
            check(
                "D2 the reconnect line names the awake window's session",
                bool(awake) and session_of(log_path, "reconnect") == awake[0]["session"],
                f"{session_of(log_path, 'reconnect')} {state}",
            )
            check("no page errors", not errors, str(errors[:3]))

            # D3 ---------------------------------------------------------------
            # The resume opens a NEW socket. Closed while it still connects, it
            # ends `dropped` (no Close frame), and closed before it exists, it
            # logs nothing: measured, 7 passes in 14 runs without this wait.
            page.wait_for_function(NEW_SOCKET_OPEN, timeout=10000)
            browser.close()
        check(
            "D3 closing the page says client-closed",
            wait_for_end(log_path, "client-closed", 10),
            str(ends(log_path)),
        )
        check("D4 no line says dropped", "dropped" not in ends(log_path), str(ends(log_path)))

        # B1-B3: the desk records now name the two live sessions -------------
        seen = len(traffic_lines(log_path))
        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_context(viewport=dict(VIEW)).new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page, want=2)
            time.sleep(2)
            state = page.evaluate(STATE)
            asleep = [w for w in state if w["dormant"] and w["session"] is not None]
            no_term = page.evaluate(
                "() => [...document.querySelectorAll('#stage .session-window.dormant')]"
                ".every((w) => !w._term)"
            )
            check("B1 both consoles start asleep", len(asleep) == 2 and no_term, str(state))
            time.sleep(SLEEP_WAIT_S)
            named = sessions_since(log_path, seen)
            check(
                "B2 they opened no socket",
                bool(asleep) and not any(w["session"] in named for w in asleep),
                f"{asleep} {named}",
            )
            if asleep:
                page.evaluate(
                    "(id) => [...document.querySelectorAll('#stage .session-window')]"
                    ".find((w) => w._deskId === id).scrollIntoView({ block: 'center', inline: 'center' })",
                    asleep[0]["id"],
                )
            try:
                page.wait_for_function(
                    "(id) => { const w = [...document.querySelectorAll('#stage .session-window')]"
                    ".find((w) => w._deskId === id);"
                    " return !w.classList.contains('dormant') && w._term?.term?.cols > 0; }",
                    arg=asleep[0]["id"] if asleep else "",
                    timeout=10000,
                )
                woke = True
            except Exception:
                woke = False
            check("B3 brought into view, it wakes and attaches", woke, str(page.evaluate(STATE)))
            browser.close()
        check("no page errors on the second page", not errors, str(errors[:3]))
    finally:
        T.stop(proc)
    for line in traffic_lines(log_path):
        print("  " + line)
    check(f"floor: at least {FLOOR} checks ran", len(results) >= FLOOR, str(len(results)))


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"\n{passed}/{len(results)} passed")
    sys.exit(0 if results and all(results) else 1)
