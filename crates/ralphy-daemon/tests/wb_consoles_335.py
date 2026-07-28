"""#335 browser acceptance: a busy session is a visible state, not a modal.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry — the operator's own desk and login policy are untouched).
Two browser contexts drive ONE live session: one holds the writer slot, the
other watches.

Scenario 1   ctx B parks on ctx A's live session; its `.session-parked` names
             what it watches (`watching console · ` + `driven in another
             window`)
Scenario 2   typing into parked B never reaches the child, the strip pulses
             `is-nudged` with a read-only hint, and the pulse is transient
Scenario 3   B's explicit take-over click parks A, replays B's scrollback
             exactly once, and B's next keystroke reaches the child
Scenario 4   parked A re-claims with one click and drives the child again,
             with NO `page.reload()` anywhere in the scenario
Scenario 5   `window.confirm` is never called across the whole run

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/335-watch-baton-2026-07-27.png.
Run: python crates/ralphy-daemon/tests/wb_consoles_335.py   (exit 0 = all pass)
"""

import json
import os
import socket
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
SHOT = "335-watch-baton-2026-07-27.png"
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


def port_already_listening(port, host="127.0.0.1"):
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.settimeout(0.5)
    try:
        return s.connect_ex((host, port)) == 0
    finally:
        s.close()


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
    empty = tempfile.mkdtemp(prefix="wb335_empty_")
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
    d = tempfile.mkdtemp(prefix="wb335_fixture_")
    p = Path(d)
    (p / "README.md").write_text("# fixture\n\nThe #335 watch-baton fixture repo.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb335@example.com"],
        ["git", "config", "user.name", "wb335"],
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


# Every context wraps `WebSocket` before any page script runs, and also counts
# outgoing terminal/resize frames and spies on `window.confirm` — the three
# oracles this issue's ACs turn on: no client-initiated storm, keystrokes that
# actually leave the browser, and no modal anywhere on the path.
WS_SPY = """
(() => {
  const Native = window.WebSocket;
  window.__wsCount = { created: 0, closed: 0 };
  window.__wsUrls = [];
  window.__wsSent = { term: 0, cmd: 0 };
  // Content, not just a count: xterm reports its OWN terminal-protocol frames
  // (cursor-position reports, DECSET 1004 focus in/out) over the identical
  // TAG_TERMINAL channel as a real keystroke, so a raw count bumps on a
  // DOM focus change alone. Match a keystroke by the text it carries.
  window.__wsSentTermText = [];
  window.__confirmCalls = [];
  window.confirm = (m) => { window.__confirmCalls.push(String(m)); return false; };
  function Spy(url, protocols) {
    const sock = protocols === undefined ? new Native(url) : new Native(url, protocols);
    if (String(url).includes('/ws/session')) {
      window.__wsCount.created += 1;
      window.__wsUrls.push(String(url));
      sock.addEventListener('close', () => { window.__wsCount.closed += 1; });
      const nsend = sock.send.bind(sock);
      sock.send = (d) => {
        try {
          const u = new Uint8Array(d);
          const t = u[0];
          if (t === 1) {
            window.__wsSent.term += 1;
            window.__wsSentTermText.push(new TextDecoder().decode(u.subarray(9)));
          } else if (t === 2) window.__wsSent.cmd += 1;
        } catch {}
        return nsend(d);
      };
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


def sent_counters(page):
    return page.evaluate("() => JSON.parse(JSON.stringify(window.__wsSent))")


def sent_term_texts(page):
    return page.evaluate("() => window.__wsSentTermText.slice()")


def confirm_calls(page):
    return page.evaluate("() => window.__confirmCalls")


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
    """Feed one line through xterm's own data path, as ONE onData event."""
    page.locator(".session-window").nth(i).locator(".xterm").click()
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def got_lines(page, i=0):
    """The child's `GOT:<line>` echoes, one per completed stdin line.

    Matched as "a GOT line CONTAINING the token", never `GOT:<token>` exactly:
    a reattach makes the client send a resize, and ConPTY repaints its
    cooked-mode edit buffer with the PREVIOUS line still in it (issue #334
    residue) — so the token reaching the child, not an exact line, is what
    this oracle is for.
    """
    return [l for l in screen(page, i).split("\n") if l.startswith("GOT:")]


def reached_child(page, i, token, timeout=15000):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if any(token in l for l in got_lines(page, i)):
            return True
        page.wait_for_timeout(250)
    return False


def never_reached_child(page, i, token):
    return not any(token in l for l in got_lines(page, i))


def flat(page, i=0):
    """The buffer with its line breaks removed. xterm hard-wraps at the terminal
    width, so a UI marker is split mid-word in the buffer and a literal search
    over `screen()` would miss it."""
    return screen(page, i).replace("\n", "")


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
    if port_already_listening(PORT):
        check(f"port {PORT} free before launch", False, "a listener is ALREADY bound — aborting so this pass never measures someone else's daemon")
        sys.exit(1)

    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb335_reg_")
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

            # --- scenario 1: ctx B parks and names what it watches -----------
            ctx_a = new_context(browser)
            page_a = desk_page(ctx_a, settle=1200)
            open_console(page_a, slug)
            type_line(page_a, 0, "hello-335")
            check(
                "ctx A's keystroke reaches the child",
                reached_child(page_a, 0, "hello-335"),
                screen(page_a, 0)[-120:],
            )
            check("…and A is not parked", parked(page_a) == 0)

            ctx_b = new_context(browser)
            page_b = desk_page(ctx_b, settle=6000)
            check(
                "a second browser profile restores the same window",
                page_b.locator(".session-window").count() == 1,
                f"got={page_b.locator('.session-window').count()}",
            )
            check("…B's window is parked", parked(page_b) == 1)
            parked_text = page_b.locator(".session-parked").inner_text()
            check(
                "…the strip names what it watches",
                "watching console · " in parked_text,
                f"got={parked_text!r}",
            )
            check(
                "…and says the session is driven elsewhere",
                "driven in another window" in parked_text,
                f"got={parked_text!r}",
            )

            # --- scenario 2: the watcher gate + its nudge --------------------
            # Matched by CONTENT, not a raw frame count: xterm reports its OWN
            # terminal-protocol frames (cursor-position reports, DECSET 1004
            # focus in/out) over the identical TAG_TERMINAL channel as a real
            # keystroke, so a DOM focus change alone can legitimately bump the
            # count (measured live around the take-over reconnect below).
            terms_before = sent_term_texts(page_b)
            color_before = page_b.evaluate(
                "() => getComputedStyle(document.querySelector('.session-parked')).color"
            )
            hint_color_before = page_b.evaluate(
                "() => getComputedStyle(document.querySelector('.session-parked-hint')).color"
            )
            type_line(page_b, 0, "watcher-must-not-type")
            new_terms = sent_term_texts(page_b)[len(terms_before):]
            check(
                "a watcher's keystroke never leaves the browser",
                not any("watcher-must-not-type" in t for t in new_terms),
                f"new_terms={new_terms}",
            )
            check(
                "…the strip pulses is-nudged",
                page_b.locator(".session-parked.is-nudged").count() == 1,
                f"got={page_b.locator('.session-parked.is-nudged').count()}",
            )
            hint_text = page_b.locator(".session-parked-hint").inner_text()
            check(
                "…with the read-only hint",
                hint_text == "input is read-only — take over to type",
                f"got={hint_text!r}",
            )
            color_after = page_b.evaluate(
                "() => getComputedStyle(document.querySelector('.session-parked')).color"
            )
            check(
                "…and the strip's computed color visibly changes",
                color_after != color_before,
                f"before={color_before} after={color_after}",
            )
            hint_color_after = page_b.evaluate(
                "() => getComputedStyle(document.querySelector('.session-parked-hint')).color"
            )
            check(
                "…and the hint text's OWN computed color changes too (not just the strip's)",
                hint_color_after != hint_color_before,
                f"before={hint_color_before} after={hint_color_after}",
            )

            # Ordering barrier instead of a fixed sleep: a writer sentinel that
            # must appear first proves the watcher's line never arrived, rather
            # than merely not having arrived YET.
            type_line(page_a, 0, "writer-sentinel")
            check(
                "the writer's sentinel reaches the child",
                reached_child(page_a, 0, "writer-sentinel"),
                screen(page_a, 0)[-120:],
            )
            check(
                "…and the watcher's keystroke NEVER reached the child (A)",
                never_reached_child(page_a, 0, "watcher-must-not-type"),
                f"got={got_lines(page_a, 0)}",
            )
            check(
                "…nor in the watcher's own buffer (B)",
                never_reached_child(page_b, 0, "watcher-must-not-type"),
                f"got={got_lines(page_b, 0)}",
            )

            page_b.wait_for_timeout(3000)
            check(
                "the nudge is transient — is-nudged clears within 3s",
                page_b.locator(".session-parked.is-nudged").count() == 0,
                f"got={page_b.locator('.session-parked.is-nudged').count()}",
            )
            hint_after = page_b.evaluate(
                "() => document.querySelector('.session-parked-hint').textContent"
            )
            check(
                "…and the hint text clears too",
                hint_after == "",
                f"got={hint_after!r}",
            )

            # --- scenario 3: the baton moves on an EXPLICIT click ------------
            take_over_count = page_b.locator('[data-act="take-over"]').count()
            check(
                "exactly one take-over control exists in the parked window",
                take_over_count == 1,
                f"got={take_over_count}",
            )
            check("…and B has sent no takeover yet", takeover_urls(page_b) == [])

            type_line(page_a, 0, "replay-once")
            check(
                "the watcher sees the writer's output before taking over",
                reached_child(page_b, 0, "replay-once"),
                flat(page_b, 0)[-160:],
            )

            page_b.locator('[data-act="take-over"]').click()
            page_b.wait_for_timeout(2500)
            check(
                "B's take-over click is the FIRST takeover=1 URL",
                len(takeover_urls(page_b)) == 1,
                f"got={takeover_urls(page_b)}",
            )
            check("…A never sent a takeover of its own", takeover_urls(page_a) == [])
            check("…B is no longer parked", parked(page_b) == 0)
            check("…and A is now parked", parked(page_a) == 1)
            check(
                "the evicted A keeps exactly one window",
                page_a.locator(".session-window").count() == 1,
                f"got={page_a.locator('.session-window').count()}",
            )

            # Evidence PNG while A is parked — BEFORE the re-claim click below.
            page_a.screenshot(path=os.path.join(SHOT_DIR, SHOT))

            # BEFORE typing anything else into B: the scrollback replay must be
            # exactly once. With `term.reset()` removed from the reconnecting
            # `onopen` this count becomes 2 — the inversion this line detects.
            replay_count = sum(1 for l in got_lines(page_b, 0) if "replay-once" in l)
            check(
                "B's scrollback shows the replayed line exactly once",
                replay_count == 1,
                f"got={got_lines(page_b, 0)}",
            )

            # Matched by CONTENT, not "+1": the take-over reconnect legitimately
            # emits its own cursor-position/focus-tracking frames on the same
            # TAG_TERMINAL channel (see the scenario 2 comment above), so a raw
            # delta is not a stable oracle across this reconnect boundary.
            terms_before_baton = sent_term_texts(page_b)
            type_line(page_b, 0, "after-baton")
            new_terms_b = sent_term_texts(page_b)[len(terms_before_baton):]
            check(
                "B's next keystroke is sent over the wire, as the writer now",
                any("after-baton" in t for t in new_terms_b),
                f"new_terms={new_terms_b}",
            )
            check(
                "…and reaches the child",
                reached_child(page_b, 0, "after-baton"),
                flat(page_b, 0)[-120:],
            )
            check(
                "…while the parked watcher (A) still sees the output",
                reached_child(page_a, 0, "after-baton"),
                flat(page_a, 0)[-120:],
            )

            # --- scenario 4: parked A re-claims with one click ---------------
            page_a.locator('[data-act="take-over"]').click()
            page_a.wait_for_timeout(2500)
            check("A is no longer parked after re-claiming", parked(page_a) == 0)
            check(
                "…via exactly one takeover=1 URL",
                len(takeover_urls(page_a)) == 1,
                f"got={takeover_urls(page_a)}",
            )
            check("…and B parks in turn", parked(page_b) == 1)

            # No `page.reload()` anywhere in this scenario — the re-claim is the
            # AC this issue ships: a parked client gets back WITHOUT reloading.
            type_line(page_a, 0, "reclaimed")
            check(
                "the re-claimed A drives the child again",
                reached_child(page_a, 0, "reclaimed"),
                flat(page_a, 0)[-120:],
            )

            # --- scenario 5: no modal anywhere on the path --------------------
            check("no confirm() dialog fired in A", confirm_calls(page_a) == [], f"got={confirm_calls(page_a)}")
            check("…nor in B", confirm_calls(page_b) == [], f"got={confirm_calls(page_b)}")

            ctx_b.close()
            ctx_a.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results) and len(results) >= 25
    print(f"\n{sum(1 for r in results if r)}/{len(results)} checks passed", flush=True)
    print("THE BATON IS VISIBLE" if ok else "FAILED", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
