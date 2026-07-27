"""#334 browser acceptance: the client stops stealing sessions.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. Two browser contexts drive
ONE live session: one holds the writer slot, the other watches.

Scenario 1   `WBConsole.reconnectDecision` in isolation: a 16-row table over
             close-event shapes (announced/not, opened/not, clean/not, id
             known/unknown, both cap boundaries)
Scenario 2   ctx A opens a console and its keystroke round-trips
Scenario 3   ctx B (fresh profile) restores the desk, lands as a WATCHER, sees
             the same output, and neither context storms the socket over 12 s
Scenario 4   an explicit take-over click moves the baton: exactly one
             `takeover=1` URL ever, and A parks instead of stealing it back
Scenario 5   a real dropped link (offline/online) still recovers by reconnecting
Scenario 6   F5 reattaches without evicting anything
Scenario 7   a genuine end (`POST /api/sessions/close`) states "closed" in both

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/334-console-pairing-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_consoles_334.py   (exit 0 = all pass)
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

PORT = 7423
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "334-console-pairing-2026-07-27.png"
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
    """A scratch registry + empty vendor stores, plus the deterministic echo
    child: `RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a
    `session_test_child`, whose `GOT:<line>` reply is a machine-readable oracle
    for "this keystroke reached the PTY"."""
    empty = tempfile.mkdtemp(prefix="wb334_empty_")
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
    d = tempfile.mkdtemp(prefix="wb334_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #334 console-pairing fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb334@example.com"],
        ["git", "config", "user.name", "wb334"],
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
    return result.stdout.strip().split("registered ", 1)[1].split(" →")[0].strip()


def build():
    # The UI assets are `include_dir!`-embedded: without this the browser loads
    # the previous build's console. The echo child is the session under test.
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


def http(method, path, body=None):
    data = None
    headers = {}
    if body is not None:
        import json as _json

        data = _json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status, r.read().decode()


# --- scenario 1: the pure reconnect rule, evaluated in the page --------------
# One row per rule, plus both cap boundaries. `announced` is the daemon's
# eviction reason (null when the close carried no announcement); `opened` is
# whether THIS socket opened, `everOpened` whether any socket of this window did.
def row(**kw):
    base = {
        "code": 1006,
        "wasClean": False,
        "opened": False,
        "everOpened": False,
        "announced": None,
        "idKnown": True,
        "failedReopens": 0,
    }
    base.update(kw)
    return base


DECISION_ROWS = [
    (
        "taken-over on an opened socket parks as a watcher",
        row(announced="taken-over", opened=True, everOpened=True, code=1005),
        "park-as-watcher",
    ),
    (
        "taken-over before the socket opened still parks",
        row(announced="taken-over", code=1006),
        "park-as-watcher",
    ),
    (
        "an unknown id gives up even when taken over (R1 precedes R2)",
        row(announced="taken-over", opened=True, everOpened=True, code=1005, idKnown=False),
        "give-up",
    ),
    (
        "child-exited gives up",
        row(announced="child-exited", opened=True, everOpened=True, code=1005),
        "give-up",
    ),
    (
        "daemon-shutdown gives up",
        row(announced="daemon-shutdown", opened=True, everOpened=True, code=1005),
        "give-up",
    ),
    (
        "child-exited on a clean 1000 gives up",
        row(announced="child-exited", opened=True, everOpened=True, code=1000, wasClean=True),
        "give-up",
    ),
    (
        "no announcement + a clean 1000 gives up",
        row(opened=True, everOpened=True, code=1000, wasClean=True),
        "give-up",
    ),
    (
        "no announcement + a dirty 1001 (going away) gives up",
        row(opened=True, everOpened=True, code=1001),
        "give-up",
    ),
    (
        "no announcement + a dirty 1006 reconnects",
        row(opened=True, everOpened=True, code=1006),
        "reconnect",
    ),
    (
        "no announcement + the measured 1005 eviction shape reconnects",
        row(opened=True, everOpened=True, code=1005),
        "reconnect",
    ),
    (
        "no announcement + an unknown id gives up",
        row(opened=True, everOpened=True, code=1006, idKnown=False),
        "give-up",
    ),
    (
        "a never-opened socket retries as a would-be writer",
        row(code=1006, failedReopens=1),
        "reconnect",
    ),
    (
        "…and settles for watching at WATCH_AFTER=3",
        row(code=1006, failedReopens=3),
        "park-as-watcher",
    ),
    (
        "a window that HAS held the session keeps reconnecting past WATCH_AFTER",
        row(code=1006, everOpened=True, failedReopens=5),
        "reconnect",
    ),
    (
        "failedReopens at the cap (10) still reconnects",
        row(opened=True, everOpened=True, code=1005, failedReopens=10),
        "reconnect",
    ),
    (
        "failedReopens past the cap (11) gives up",
        row(opened=True, everOpened=True, code=1005, failedReopens=11),
        "give-up",
    ),
]


def decision_table(page):
    outs = page.evaluate(
        "(rows) => rows.map((r) => { try {"
        " return window.WBConsole.reconnectDecision(r);"
        " } catch (e) { return String(e); } })",
        [r for (_, r, _) in DECISION_ROWS],
    )
    for (label, inp, want), got in zip(DECISION_ROWS, outs):
        check(f"reconnectDecision: {label}", got == want, f"got={got!r} want={want!r} in={inp}")


# Every context wraps `WebSocket` before any page script runs, so a reconnect
# storm is COUNTED, not eyeballed: `__wsCount` tallies `/ws/session`
# constructions and closes, `__wsUrls` keeps every URL (which is how "no
# client-initiated reconnect carries takeover" is asserted rather than assumed).
WS_SPY = """
(() => {
  const Native = window.WebSocket;
  window.__wsCount = { created: 0, closed: 0 };
  window.__wsUrls = [];
  function Spy(url, protocols) {
    const sock = protocols === undefined ? new Native(url) : new Native(url, protocols);
    if (String(url).includes('/ws/session')) {
      window.__wsCount.created += 1;
      window.__wsUrls.push(String(url));
      sock.addEventListener('close', () => { window.__wsCount.closed += 1; });
    }
    return sock;
  }
  Spy.prototype = Native.prototype;
  // The client reads `WebSocket.OPEN`; without these the readyState guards in
  // wb-console.js compare against `undefined` and every send is dropped.
  Spy.CONNECTING = Native.CONNECTING;
  Spy.OPEN = Native.OPEN;
  Spy.CLOSING = Native.CLOSING;
  Spy.CLOSED = Native.CLOSED;
  window.WebSocket = Spy;
})();
"""


def new_context(browser):
    ctx = browser.new_context(viewport={"width": 1400, "height": 900})
    ctx.add_init_script(WS_SPY)
    return ctx


def probe_page(ctx):
    """A bare page on the Consoles tab — enough for the pure-function table."""
    page = ctx.new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(600)
    return page


def desk_page(ctx, settle=2500):
    """A page on the Consoles tab with the daemon's desk already restored."""
    page = ctx.new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
    page.wait_for_timeout(settle)
    return page


def counters(page):
    return page.evaluate("() => JSON.parse(JSON.stringify(window.__wsCount))")


def urls(page):
    return page.evaluate("() => window.__wsUrls.slice()")


def takeover_urls(page):
    return [u for u in urls(page) if "takeover=1" in u]


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


def type_line(page, i, text):
    """Feed one line through xterm's own data path. `paste` rather than
    `keyboard.type` + Enter: measured, a reattached window echoed the typed
    characters but the trailing Enter never completed a line at the child, so
    `GOT:<line>` never came back. `paste` emits the whole line as ONE onData
    event over the same socket, which is the routing this pass is about."""
    page.locator(".session-window").nth(i).locator(".xterm").click()
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def flat(page, i=0):
    """The buffer with its line breaks removed. xterm hard-wraps at the terminal
    width, so a UI marker like `[connection lost — reconnecting…]` is split
    mid-word in the buffer and a literal search over `screen()` would miss it."""
    return screen(page, i).replace("\n", "")


def wait_for_screen(page, i, needle, timeout=15000):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if needle in flat(page, i):
            return True
        page.wait_for_timeout(250)
    return False


def open_console(page, slug):
    before = page.locator(".session-window").count()
    page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000
    )
    page.locator(".session-window").nth(before).locator(".xterm").wait_for(timeout=15000)
    page.wait_for_timeout(600)
    return page.locator(".session-window").nth(before)


def parked(page, i=0):
    return page.locator(".session-window").nth(i).locator(".session-parked").count()


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb334_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # --- scenario 1: the pure reconnect rule -------------------------
            ctx_probe = new_context(browser)
            page = probe_page(ctx_probe)
            check(
                "reconnectDecision is exported like resizeRect/reconcileDesk",
                page.evaluate("() => typeof window.WBConsole.reconnectDecision") == "function",
            )
            decision_table(page)
            ctx_probe.close()

            # --- scenario 2: ctx A drives a live console ---------------------
            ctx_a = new_context(browser)
            page_a = desk_page(ctx_a, settle=1200)
            open_console(page_a, slug)
            type_line(page_a, 0, "hello-334")
            check(
                "ctx A's keystroke reaches the child",
                wait_for_screen(page_a, 0, "GOT:hello-334"),
                screen(page_a, 0)[-120:],
            )
            check(
                "…over exactly one socket",
                counters(page_a)["created"] == 1,
                f"got={counters(page_a)}",
            )
            check("…and A is not parked", parked(page_a) == 0)

            sid = page_a.evaluate(
                "() => document.querySelector('.session-window')._term.sessionId"
            )
            check("…with a known session id", sid is not None, f"got={sid}")

            # --- scenario 3: ctx B restores the same desk as a WATCHER -------
            ctx_b = new_context(browser)
            page_b = desk_page(ctx_b, settle=6000)
            check(
                "a second browser profile restores the same window",
                page_b.locator(".session-window").count() == 1,
                f"got={page_b.locator('.session-window').count()}",
            )
            check(
                "…and sees the session's output without claiming it",
                wait_for_screen(page_b, 0, "GOT:hello-334"),
                screen(page_b, 0)[-120:],
            )
            check("…B's window is parked", parked(page_b) == 1)
            parked_text = page_b.locator(".session-parked").inner_text()
            check(
                "…the strip says the session is driven elsewhere",
                "driven in another window" in parked_text,
                f"got={parked_text!r}",
            )
            check("…A is still NOT parked", parked(page_a) == 0)
            check(
                "…B's terminal says so too",
                "[watching — driven in another window]" in flat(page_b, 0),
                screen(page_b, 0)[-160:],
            )

            count_a = counters(page_a)
            count_b = counters(page_b)
            check("A opened exactly 1 session socket", count_a["created"] == 1, f"got={count_a}")
            check(
                "B opened exactly 4 (one refused, two bounded retries, one watch)",
                count_b["created"] == 4,
                f"got={count_b}",
            )
            check(
                "…the last of them being the watch attach",
                urls(page_b)[-1].endswith("watch=1"),
                f"got={urls(page_b)[-1]}",
            )
            check(
                "NO client-initiated reconnect carried takeover, in either context",
                takeover_urls(page_a) == [] and takeover_urls(page_b) == [],
                f"A={takeover_urls(page_a)} B={takeover_urls(page_b)}",
            )

            # The storm assertion: counts, not the absence of visible flicker.
            page_a.wait_for_timeout(12000)
            check(
                "over a sustained 12s window A opens/closes nothing more",
                counters(page_a) == count_a,
                f"before={count_a} after={counters(page_a)}",
            )
            check(
                "…and neither does B",
                counters(page_b) == count_b,
                f"before={count_b} after={counters(page_b)}",
            )
            check(
                "…still with no takeover=1 URL anywhere",
                takeover_urls(page_a) == [] and takeover_urls(page_b) == [],
            )

            # A watcher's keystrokes are dropped by the daemon, not echoed back.
            type_line(page_b, 0, "watcher-must-not-type")
            page_b.wait_for_timeout(1500)
            check(
                "a watcher's keystroke never reaches the child (B)",
                "GOT:watcher-must-not-type" not in flat(page_b, 0),
            )
            check(
                "…nor shows up in the writer's terminal (A)",
                "GOT:watcher-must-not-type" not in flat(page_a, 0),
            )

            # --- scenario 4: the baton moves on an EXPLICIT click ------------
            page_b.locator('[data-act="take-over"]').click()
            page_b.wait_for_timeout(2500)
            check(
                "B's take-over click is the FIRST takeover=1 URL of the run",
                len(takeover_urls(page_b)) == 1,
                f"got={takeover_urls(page_b)}",
            )
            check("…and B is no longer parked", parked(page_b) == 0)
            check(
                "the evicted A keeps exactly one window — it does not vanish or duplicate",
                page_a.locator(".session-window").count() == 1,
                f"got={page_a.locator('.session-window').count()}",
            )
            check("…and A is now parked", parked(page_a) == 1)
            check(
                "…A says the session is driven elsewhere",
                "[watching — driven in another window]" in flat(page_a, 0),
                flat(page_a, 0)[-160:],
            )
            check(
                "…A NEVER sent a takeover of its own (the storm's root cause)",
                takeover_urls(page_a) == [],
                f"got={takeover_urls(page_a)}",
            )
            after_take_a = counters(page_a)
            check(
                "…A opened exactly ONE more socket (the watch reattach)",
                after_take_a["created"] == count_a["created"] + 1,
                f"before={count_a} after={after_take_a}",
            )
            page_a.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            page_a.wait_for_timeout(8000)
            check(
                "…and stays parked, opening nothing further over 8s",
                counters(page_a) == after_take_a and parked(page_a) == 1,
                f"before={after_take_a} after={counters(page_a)}",
            )

            type_line(page_b, 0, "after-takeover")
            check(
                "the taker drives the child",
                wait_for_screen(page_b, 0, "GOT:after-takeover"),
                flat(page_b, 0)[-120:],
            )
            check(
                "…and the parked watcher sees it too",
                wait_for_screen(page_a, 0, "GOT:after-takeover"),
                flat(page_a, 0)[-120:],
            )

            # Hand the baton BACK, so the drop/reload scenarios below exercise a
            # WRITER's recovery. A second explicit click — still operator-driven.
            page_a.locator('[data-act="take-over"]').click()
            page_a.wait_for_timeout(2500)
            check(
                "A reclaims the baton only by its own explicit click",
                len(takeover_urls(page_a)) == 1 and parked(page_a) == 0,
                f"urls={takeover_urls(page_a)} parked={parked(page_a)}",
            )
            check("…and B parks in turn", parked(page_b) == 1)
            base_b = counters(page_b)
            base_a = counters(page_a)

            # --- scenario 5: a real dropped link still recovers --------------
            # The outage is real — offline blocks every reconnect attempt for its
            # duration. The DROP itself is delivered in-page: measured on this
            # host, Chromium's offline emulation leaves an already-established
            # WebSocket up (no close event for >10s), so the socket is closed and
            # the client's own handler invoked with the `1006 / wasClean=false`
            # shape a dropped link produces. Nothing about the client is stubbed:
            # this is the event the browser itself would deliver.
            ctx_a.set_offline(True)
            page_a.evaluate(
                "() => { const t = document.querySelectorAll('.session-window')[0]._term;"
                " const s = t.ws; const onclose = s.onclose; s.onclose = null; s.close();"
                " setTimeout(() => onclose({ code: 1006, wasClean: false }), 50); }"
            )
            check(
                "an offline A says the link dropped, not that the session ended",
                wait_for_screen(page_a, 0, "[connection lost — reconnecting…]", timeout=10000),
                flat(page_a, 0)[-160:],
            )
            check(
                "…and does NOT claim the session ended",
                "[session closed]" not in flat(page_a, 0),
            )
            ctx_a.set_offline(False)
            type_ok = False
            deadline = time.time() + 20
            while time.time() < deadline and not type_ok:
                page_a.wait_for_timeout(1000)
                if parked(page_a) == 0:
                    type_line(page_a, 0, "after-drop")
                    type_ok = wait_for_screen(page_a, 0, "GOT:after-drop", timeout=4000)
            check("a recovered A drives the child again", type_ok, flat(page_a, 0)[-160:])
            check("…without parking", parked(page_a) == 0)
            check(
                "…and without ever sending a second takeover",
                len(takeover_urls(page_a)) == 1,
                f"got={takeover_urls(page_a)}",
            )
            check(
                "…while B's socket count is untouched by A's outage",
                counters(page_b) == base_b,
                f"before={base_b} after={counters(page_b)}",
            )
            check(
                "…and B sees the recovered writer's output",
                wait_for_screen(page_b, 0, "GOT:after-drop"),
                flat(page_b, 0)[-120:],
            )

            # --- scenario 6: F5 reattaches without evicting anything ---------
            page_a.reload()
            page_a.wait_for_selector("[x-data]", timeout=8000)
            page_a.evaluate(f"() => {{ {SH}.active = 'consoles'; }}")
            page_a.wait_for_timeout(6000)
            check(
                "a reloaded A comes back with exactly one window",
                page_a.locator(".session-window").count() == 1,
                f"got={page_a.locator('.session-window').count()}",
            )
            check("…not parked — the reload reclaimed its own slot", parked(page_a) == 0)
            check(
                "…having sent no takeover on the way back",
                takeover_urls(page_a) == [],
                f"got={takeover_urls(page_a)}",
            )
            type_line(page_a, 0, "after-reload")
            check(
                "…and drives the child again",
                wait_for_screen(page_a, 0, "GOT:after-reload"),
                flat(page_a, 0)[-120:],
            )
            check(
                "…while B, never evicted, sees the same line",
                wait_for_screen(page_b, 0, "GOT:after-reload"),
                flat(page_b, 0)[-120:],
            )
            check(
                "…with B's socket count unchanged across the whole reload",
                counters(page_b) == base_b,
                f"before={base_b} after={counters(page_b)}",
            )

            # --- scenario 7: a genuine end says "closed", in both contexts ---
            lost_a = flat(page_a, 0).count("[connection lost — reconnecting…]")
            lost_b = flat(page_b, 0).count("[connection lost — reconnecting…]")
            base_a = counters(page_a)
            http("POST", f"api/sessions/close?id={sid}")
            check(
                "a closed session states so in the writer's terminal",
                wait_for_screen(page_a, 0, "[session closed]"),
                flat(page_a, 0)[-160:],
            )
            check(
                "…and in the watcher's",
                wait_for_screen(page_b, 0, "[session closed]"),
                flat(page_b, 0)[-160:],
            )
            check(
                "…without either reading it as a flaky link",
                flat(page_a, 0).count("[connection lost — reconnecting…]") == lost_a
                and flat(page_b, 0).count("[connection lost — reconnecting…]") == lost_b,
            )
            page_a.wait_for_timeout(6000)
            # `created` is the storm oracle here, not the whole dict: the close
            # legitimately bumps `closed` in both contexts.
            check(
                "…and neither retries afterwards (A)",
                counters(page_a)["created"] == base_a["created"],
                f"before={base_a} after={counters(page_a)}",
            )
            check(
                "…nor B",
                counters(page_b)["created"] == base_b["created"],
                f"before={base_b} after={counters(page_b)}",
            )

            ctx_b.close()
            ctx_a.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 40
    print(f"\n{sum(1 for r in results if r)}/{len(results)} checks passed", flush=True)
    print("NO TAKEOVER, NO STORM" if ok else "FAILED", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
