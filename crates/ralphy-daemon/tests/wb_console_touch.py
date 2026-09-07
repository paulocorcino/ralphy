"""Browser acceptance for the console's touch surface — the key bar, the terminal
font size, the virtual-keyboard inset, and the resume that brings a suspended
tablet's sockets back.

The pure rules are tabled in `ui-tests/wb-console.test.mjs` (keySequence,
applyCtrlLatch, keyBarVisible, stepFont, keyboardInset, resumeDecision). This
script proves the wiring those tables cannot reach: that a tapped button puts the
right BYTES on the real socket to a real child, that the bar appears on a touch
device and not on a desktop, that the size survives a reload in the one permitted
browser key, that the CSS var actually resizes a maximized console, and that a
resume opens a NEW socket without reviving a window whose session ended.

The key bytes are read off the socket rather than off the screen. A child that
echoes `GOT:<ESC>[A` hands the escape straight back to xterm, which acts on it
instead of printing it — so the terminal buffer is not an oracle for control
input. `ws.send` is wrapped instead, which is exactly what the operator's tap
produces, one layer above the wire.

Scenario 1   the bar is absent under a fine pointer and present once the operator
             turns it on, and the setting survives a reload
Scenario 2   with the setting left at its default, a touch device gets the bar and
             a desktop does not
Scenario 3   esc / tab / arrows put the right bytes on the socket, and the arrows
             follow the terminal into application cursor mode
Scenario 4   copy is disabled with no selection and enabled once there is one
Scenario 5   A+ / A− resize every console at once, persist, and survive a reload —
             with the browser still holding exactly one key
Scenario 6   `--kb-inset` shrinks a maximized console and the terminal refits
Scenario 7   a watcher's tap is refused and pulses the parked strip, exactly as a
             watcher's keystroke is
Scenario 8   the Ctrl latch folds the next character into a control code
Scenario 9   a resume opens a new socket when the caller says the link is stale,
             leaves a live one alone when it does not, and never revives a window
             whose session has ended
Scenario 10  ^C reaches the child as ETX

Run: python crates/ralphy-daemon/tests/wb_console_touch.py

The daemon is stopped by its own subprocess handle, NEVER by name — a stray
`taskkill /im ralphy.exe` would take the operator's own daemon with it.
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

PORT = 7450
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "console-touch-2026-09-07.png"

VIEW_KEY = "wb.view.v1"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


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
    empty = tempfile.mkdtemp(prefix="wbtouch_empty_")
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
    d = tempfile.mkdtemp(prefix="wbtouch_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n\nThe console touch fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbtouch@example.com"],
        ["git", "config", "user.name", "wbtouch"],
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
    # the previous build's console.
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


def wait_for(fn, timeout=10000, step=200):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if fn():
            return True
        time.sleep(step / 1000)
    return fn()


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


def open_console(page, slug):
    before = page.locator(".session-window").count()
    page.evaluate(f"() => window.WBConsole.open({{ repo: '{slug}', plain: true }})")
    page.wait_for_function(
        f"() => document.querySelectorAll('.session-window').length === {before + 1}", timeout=8000
    )
    page.locator(".session-window").nth(before).locator(".xterm").wait_for(timeout=15000)
    page.wait_for_timeout(600)
    return page.locator(".session-window").nth(before)


def wait_child_ready(page, i=0, timeout=15000):
    return wait_for(lambda: "READY" in screen(page, i), timeout)


def record_sends(page, i=0):
    """Wrap the window's live socket so every terminal frame it sends is captured.

    This is the oracle for control input: the child echoes an escape byte
    straight back into xterm, which ACTS on it rather than printing it, so the
    terminal buffer can never prove that `ESC [ A` was the thing sent.
    """
    page.evaluate(
        "(i) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const ws = w._term.ws;"
        "  window.__sent = [];"
        "  if (ws.__wrapped) return;"
        "  const orig = ws.send.bind(ws);"
        "  ws.send = (data) => {"
        "    const a = new Uint8Array(data);"
        # A terminal frame is [tag][8-byte session id][payload] (protocol.rs), so
        # the operator's bytes start at 9. Slicing at 1 reads the session id as
        # input, and every assertion below fails with eight leading NULs.
        "    if (a[0] === 0x01) window.__sent.push(new TextDecoder().decode(a.subarray(9)));"
        "    orig(data);"
        "  };"
        "  ws.__wrapped = true;"
        "}",
        i,
    )


def sent(page):
    """What the operator's taps put on the wire.

    xterm's own focus reports (`ESC [ I` / `ESC [ O`, sent when focus reporting
    is on) ride the same socket, and clicking a button focuses the terminal — so
    they are the terminal answering a question nobody asked, not input, and they
    are filtered out rather than asserted around.
    """
    return [d for d in page.evaluate("() => window.__sent || []") if d not in ("[I", "[O")]


def clear_sent(page):
    page.evaluate("() => { window.__sent = []; }")


def tap(page, i, name):
    """Click a key-bar button the way a finger does — through the real listener."""
    page.evaluate(
        "([i, name]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const b = w.querySelector(`.session-key[data-key='${name}']`);"
        "  if (!b) throw new Error('no key ' + name);"
        "  b.click();"
        "}",
        [i, name],
    )


def bar_shown(page, i=0):
    """Whether the bar is actually PAINTED, not merely present in the DOM.

    CONTEXT.md → Testing conventions: a geometry/visibility assertion has to
    prove the element was visible, or `display: none` reads as a pass.
    """
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " const b = w && w.querySelector('.session-keys');"
        " return !!b && b.offsetParent !== null && b.clientWidth > 0 && b.clientHeight > 0; }",
        i,
    )


def font_of(page, i=0):
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._term.term.options.fontSize", i
    )


def rows_of(page, i=0):
    return page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._term.term.rows", i)


def view_store(page):
    return page.evaluate(f"() => JSON.parse(localStorage.getItem('{VIEW_KEY}') || 'null')")


def desk_page(ctx, settle=6000):
    page = ctx.new_page()
    page.goto(BASE, wait_until="domcontentloaded")
    page.wait_for_timeout(settle)
    return page


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    if port_already_listening(PORT):
        check(
            f"port {PORT} free before launch",
            False,
            "a listener is ALREADY bound — aborting so this pass never measures someone else's daemon",
        )
        sys.exit(1)

    build()
    daemon_dir = tempfile.mkdtemp(prefix="wbtouch_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    info("fixture", f"{slug} at {fixture_dir}")

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check("daemon listening", False, BASE)
            sys.exit(1)
        check("daemon listening", True, BASE)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])

            # ---- a desktop profile: fine pointer, no touch screen -------------
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_timeout(1200)

            open_console(page, slug)
            check("the child booted", wait_child_ready(page), "READY on screen")

            # --- Scenario 1: the operator's switch, both ways -------------------
            check("1 no key bar on a desktop, where every key already exists", not bar_shown(page))
            page.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]'))"
                ".saveSetting('consoles.key_bar', 'on')"
            )
            page.wait_for_timeout(400)
            check("1 turning the setting on reveals the bar on the open console", bar_shown(page))
            check(
                "1 …and the choice is stored in the browser, not sent to the daemon",
                (view_store(page) or {}).get("keys") == "on",
                f"store={view_store(page)}",
            )
            page.reload(wait_until="domcontentloaded")
            page.wait_for_timeout(4000)
            check(
                "1 …and a reload brings the bar back with the restored console",
                page.locator(".session-window").count() == 1 and bar_shown(page),
                f"windows={page.locator('.session-window').count()}",
            )
            page.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]'))"
                ".saveSetting('consoles.key_bar', 'off')"
            )
            page.wait_for_timeout(400)
            check(
                "1 turning it off hides it again without a reload",
                not bar_shown(page),
                "the setting-change event reaches the console module",
            )

            # --- Scenario 2: the default, on each kind of machine ---------------
            page.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]'))"
                ".saveSetting('consoles.key_bar', 'unset')"
            )
            page.wait_for_timeout(400)
            check(
                "2 left at its default, a desktop still shows no bar",
                not bar_shown(page),
                f"store={view_store(page)}",
            )
            check(
                "2 …and 'unset' is stored as the ABSENCE of a preference",
                (view_store(page) or {}).get("keys") is None,
                f"store={view_store(page)}",
            )

            touch_ctx = browser.new_context(
                viewport={"width": 1024, "height": 768}, has_touch=True, is_mobile=False
            )
            touch_page = desk_page(touch_ctx)
            touch_page.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            touch_page.wait_for_timeout(800)
            check(
                "2 a touch device gets the bar with no setting at all",
                bar_shown(touch_page),
                f"store={view_store(touch_page)}",
            )
            touch_ctx.close()

            # --- Scenario 3: the bytes a tap puts on the wire -------------------
            page.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]'))"
                ".saveSetting('consoles.key_bar', 'on')"
            )
            page.wait_for_timeout(400)
            record_sends(page, 0)
            clear_sent(page)
            for name, expect in (("esc", "\x1b"), ("tab", "\t")):
                clear_sent(page)
                tap(page, 0, name)
                page.wait_for_timeout(200)
                check(f"3 {name} sends the byte a virtual keyboard has no key for", sent(page) == [expect],
                      f"got={sent(page)!r}")

            clear_sent(page)
            for name in ("up", "down", "right", "left"):
                tap(page, 0, name)
            page.wait_for_timeout(300)
            check(
                "3 the arrows send CSI while the terminal is in normal cursor mode",
                sent(page) == ["\x1b[A", "\x1b[B", "\x1b[C", "\x1b[D"],
                f"got={sent(page)!r}",
            )

            # The child does not switch modes, so drive the terminal's own parser
            # with the escape a full-screen program would emit (DECCKM set).
            page.evaluate(
                "() => document.querySelectorAll('.session-window')[0]._term.term.write('\\x1b[?1h')"
            )
            page.wait_for_timeout(300)
            check(
                "3 …and the terminal reports application cursor mode once a program asks",
                page.evaluate(
                    "() => !!document.querySelectorAll('.session-window')[0]"
                    "._term.term.modes.applicationCursorKeysMode"
                ),
            )
            clear_sent(page)
            tap(page, 0, "up")
            page.wait_for_timeout(200)
            check(
                "3 …so the arrow follows it to SS3 rather than scrolling nothing",
                sent(page) == ["\x1bOA"],
                f"got={sent(page)!r}",
            )
            page.evaluate(
                "() => document.querySelectorAll('.session-window')[0]._term.term.write('\\x1b[?1l')"
            )
            page.wait_for_timeout(200)

            # --- Scenario 4: copy, the one capability that needed a keyboard ----
            check(
                "4 copy is refused while there is nothing selected",
                page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0]"
                    ".querySelector(\".session-key[data-key='copy']\").disabled"
                ),
            )
            page.evaluate(
                "() => document.querySelectorAll('.session-window')[0]._term.term.selectAll()"
            )
            page.wait_for_timeout(400)
            check(
                "4 …and offered as soon as there is a selection to copy",
                page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0]"
                    ".querySelector(\".session-key[data-key='copy']\").disabled === false"
                ),
            )
            page.evaluate(
                "() => document.querySelectorAll('.session-window')[0]._term.term.clearSelection()"
            )

            # --- Scenario 5: the text size, across every console and a reload ---
            open_console(page, slug)
            check("5 a second console is open", page.locator(".session-window").count() == 2)
            before = font_of(page, 0)
            check("5 both consoles start at the same size", font_of(page, 1) == before, f"{before}")
            tap(page, 0, "font-up")
            tap(page, 0, "font-up")
            page.wait_for_timeout(500)
            check(
                "5 A+ resizes EVERY console, not the one that was tapped",
                font_of(page, 0) == before + 2 and font_of(page, 1) == before + 2,
                f"{font_of(page, 0)} / {font_of(page, 1)}",
            )
            tap(page, 0, "font-down")
            page.wait_for_timeout(400)
            check("5 …and A− steps back", font_of(page, 0) == before + 1, f"{font_of(page, 0)}")
            check(
                "5 …the size is stored per browser profile",
                (view_store(page) or {}).get("font") == before + 1,
                f"store={view_store(page)}",
            )
            keys_held = page.evaluate("() => Object.keys(localStorage)")
            check(
                "5 …and the browser still holds nothing but the one permitted view key",
                keys_held == [VIEW_KEY],
                f"got={keys_held}",
            )
            page.reload(wait_until="domcontentloaded")
            page.wait_for_timeout(5000)
            page.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            check(
                "5 …so a reload comes back at the chosen size, not xterm's default",
                font_of(page, 0) == before + 1,
                f"{font_of(page, 0)}",
            )

            # --- Scenario 6: the virtual keyboard's bite ------------------------
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " if (!w.classList.contains('maximized')) w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(700)
            tall = page.evaluate(
                "() => document.querySelectorAll('.session-window')[0].getBoundingClientRect().height"
            )
            tall_rows = rows_of(page, 0)
            check(
                "6 the maximized console is on screen before anything is measured",
                tall > 0
                and page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0].offsetParent !== null"
                ),
                f"height={tall}",
            )
            page.evaluate(
                "() => document.documentElement.style.setProperty('--kb-inset', '160px')"
            )
            page.wait_for_timeout(800)
            short = page.evaluate(
                "() => document.querySelectorAll('.session-window')[0].getBoundingClientRect().height"
            )
            check(
                "6 the keyboard inset shortens the console by exactly its own height",
                abs((tall - short) - 160) < 2,
                f"{tall} → {short}",
            )
            check(
                "6 …and the terminal refits, so the prompt is above the keyboard",
                rows_of(page, 0) < tall_rows,
                f"{tall_rows} → {rows_of(page, 0)} rows",
            )
            page.evaluate("() => document.documentElement.style.removeProperty('--kb-inset')")
            page.wait_for_timeout(500)
            check(
                "6 …and clearing it restores the full height",
                abs(
                    page.evaluate(
                        "() => document.querySelectorAll('.session-window')[0]"
                        ".getBoundingClientRect().height"
                    )
                    - tall
                )
                < 2,
            )
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " if (w.classList.contains('maximized')) w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(500)

            # --- Scenario 7: a watcher's tap, refused like a watcher's keystroke -
            ctx_b = browser.new_context(viewport={"width": 1280, "height": 860})
            page_b = desk_page(ctx_b, settle=7000)
            page_b.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            page_b.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]'))"
                ".saveSetting('consoles.key_bar', 'on')"
            )
            page_b.wait_for_timeout(600)
            parked_ok = wait_for(lambda: page_b.locator(".session-parked").count() > 0, 12000)
            check(
                "7 a second profile restoring the same desk parks as a watcher",
                parked_ok,
                f"parked={page_b.locator('.session-parked').count()}",
            )
            if parked_ok:
                idx = page_b.evaluate(
                    "() => [...document.querySelectorAll('.session-window')]"
                    ".findIndex((w) => w.querySelector('.session-parked'))"
                )
                record_sends(page_b, idx)
                clear_sent(page_b)
                tap(page_b, idx, "esc")
                page_b.wait_for_timeout(600)
                check(
                    "7 …and its tap reaches the child no more than its keystroke would",
                    sent(page_b) == [],
                    f"got={sent(page_b)!r}",
                )
                check(
                    "7 …with the refusal SEEN: the parked strip pulses",
                    page_b.evaluate(
                        "(i) => document.querySelectorAll('.session-window')[i]"
                        ".querySelector('.session-parked').classList.contains('is-nudged')",
                        idx,
                    ),
                )
            ctx_b.close()

            # --- Scenario 8: the Ctrl latch ------------------------------------
            second = open_console(page, slug)
            n = page.locator(".session-window").count() - 1
            check("8 a fresh console for the interrupt", wait_child_ready(page, n), "READY on screen")
            record_sends(page, n)
            clear_sent(page)
            tap(page, n, "ctrl")
            page.wait_for_timeout(200)
            check(
                "8 arming Ctrl sends nothing on its own — a modifier is not a key",
                sent(page) == [],
                f"got={sent(page)!r}",
            )
            check(
                "8 …and the button shows the chord is half-typed",
                page.evaluate(
                    "(i) => document.querySelectorAll('.session-window')[i]"
                    ".querySelector(\".session-key[data-key='ctrl']\")"
                    ".getAttribute('aria-pressed') === 'true'",
                    n,
                ),
            )
            page.evaluate(
                "(i) => document.querySelectorAll('.session-window')[i]._term.term.paste('c')", n
            )
            page.wait_for_timeout(400)
            check(
                "8 …so the next character is folded into a control code",
                sent(page) == ["\x03"],
                f"got={sent(page)!r}",
            )
            check(
                "8 …and the child was interrupted by it",
                wait_for(lambda: "INTERRUPTED:ETX" in screen(page, n), 8000),
                screen(page, n)[-120:],
            )
            check(
                "8 …with the latch disarmed again",
                page.evaluate(
                    "(i) => document.querySelectorAll('.session-window')[i]"
                    ".querySelector(\".session-key[data-key='ctrl']\")"
                    ".getAttribute('aria-pressed') === 'false'",
                    n,
                ),
            )

            # --- Scenario 9: coming back from a suspend -------------------------
            page.wait_for_timeout(1500)
            ended_count = page.evaluate(
                "() => [...document.querySelectorAll('.session-window')]"
                ".filter((w) => w._term && w._term.ws === null).length"
            )
            info("9 windows whose socket is already gone", str(ended_count))
            before_ws = page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " window.__ws0 = w._term.ws; return !!w._term.ws; }"
            )
            check("9 the first console holds a live socket to compare against", before_ws)
            woke = page.evaluate("() => window.WBConsole.resumeAll(false)")
            page.wait_for_timeout(600)
            check(
                "9 a resume with a healthy link leaves every socket alone",
                woke == 0
                and page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0]._term.ws === window.__ws0"
                ),
                f"woke={woke}",
            )
            woke = page.evaluate("() => window.WBConsole.resumeAll(true)")
            page.wait_for_timeout(1500)
            check(
                "9 …and a stale one replaces the socket rather than waiting out the backoff",
                woke >= 1
                and page.evaluate(
                    "() => document.querySelectorAll('.session-window')[0]._term.ws !== window.__ws0"
                ),
                f"woke={woke}",
            )
            check(
                "9 …bringing the session's scrollback back with it",
                wait_for(lambda: "READY" in screen(page, 0), 10000),
                screen(page, 0)[-80:],
            )
            # The window whose child exited in scenario 8 must not be dragged back
            # to life: its id is dead, so a reconnect would fail, give up, and
            # print a second "[session closed]" on every resume.
            closed_twice = screen(page, n).count("[session closed]")
            check(
                "9 …and a window whose session ended is not revived by it",
                closed_twice <= 1,
                f"'[session closed]' appears {closed_twice}×",
            )
            # The debounce, which is what stops `visibilitychange` and `online`
            # from tearing down on the second trigger the socket the first just
            # opened. Both calls in ONE evaluate: the window between them is the
            # millisecond the real pair arrives in, not a test's round trip.
            pair = page.evaluate(
                "() => [window.WBConsole.resumeAll(true), window.WBConsole.resumeAll(true)]"
            )
            check(
                "9 …with a second trigger in the same breath ignored",
                pair[1] == 0,
                f"woke={pair}",
            )

            # --- Scenario 10: ^C, the button that stops a runaway agent ---------
            open_console(page, slug)
            last = page.locator(".session-window").count() - 1
            check("10 a fresh console for the interrupt", wait_child_ready(page, last), "READY on screen")
            record_sends(page, last)
            clear_sent(page)
            tap(page, last, "ctrl-c")
            page.wait_for_timeout(400)
            check("10 ^C puts ETX on the wire", sent(page) == ["\x03"], f"got={sent(page)!r}")
            check(
                "10 …and the child reports the interrupt",
                wait_for(lambda: "INTERRUPTED:ETX" in screen(page, last), 8000),
                screen(page, last)[-120:],
            )

            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            info("screenshot", os.path.join(SHOT_DIR, SHOT))
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    ok = all(results)
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
