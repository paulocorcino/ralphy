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
Scenario 11  a one-finger drag scrolls the TERMINAL, not the canvas — the
             defect this file was extended for. Note the split oracle: a
             synthetic TouchEvent cannot drive NATIVE scrolling in any browser,
             so this half proves our handler moves the terminal, and the
             `touch-action` assertion proves the declaration that stops the
             browser panning the canvas is present. Neither alone is the claim.

Scenario 12  a window is DRAGGED by touch, and the titlebar is declared a drag
             handle rather than selectable text
Scenario 13  reloading while a console is maximized does not bury it under the
             consoles restored after it

Scenario 14  on a touch device the resize bands are a finger wide, the visible
             grip is covered by one, and a finger actually resizes the window

Scenario 15  the fullscreen button is built for an engine that can HOLD
             fullscreen and withheld from WebKit, where the first keystroke
             cancels it. Two contexts differing only in `navigator.vendor`, so
             the vendor string is the whole independent variable.

Scenario 16  the drag is the TRACKPAD, whoever owns the wheel: with the app
             tracking the mouse the finger's lines reach the socket as wheel
             reports and the viewport stays put; in the alternate buffer they
             arrive as arrow keys; back in the plain buffer they move the
             viewport again. This is the iPad "ghost text" defect — a finger
             that always moved the viewport scrolled Claude Code's stale
             frames instead of Claude Code.

Scenario 17  TWO fingers over a console pan the CANVAS — `touch-action: none`
             took that gesture from the browser, so the plane gets it back
             through the same scroll writes the mouse pan makes; the terminal
             does not scroll and nothing reaches the socket. Under `maxlock`
             there is nowhere to pan and the fingers do nothing.

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


def drag(page, i, dy, steps=10):
    """A one-finger drag down the middle of the i-th console.

    Real TouchEvents, not a wheel: the whole defect is that touch and wheel take
    different paths — xterm forwards `wheel` in JS, so the trackpad always
    worked while the finger panned the canvas instead.
    """
    page.evaluate(
        "([i, dy, steps]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const body = w.querySelector('.session-body');"
        "  const r = body.getBoundingClientRect();"
        "  const x = r.left + r.width / 2;"
        "  let y = r.top + r.height / 2;"
        "  const send = (type, cy) => {"
        "    const t = new Touch({ identifier: 1, target: body, clientX: x, clientY: cy });"
        "    body.dispatchEvent(new TouchEvent(type, {"
        "      touches: type === 'touchend' ? [] : [t],"
        "      targetTouches: type === 'touchend' ? [] : [t],"
        "      changedTouches: [t], bubbles: true, cancelable: true }));"
        "  };"
        "  send('touchstart', y);"
        "  for (let s = 0; s < steps; s++) { y += dy / steps; send('touchmove', y); }"
        "  send('touchend', y);"
        "}",
        [i, dy, steps],
    )


def drag2(page, i, dx, dy, steps=8):
    """A two-finger drag over the i-th console's body, fingers 80px apart."""
    page.evaluate(
        "([i, dx, dy, steps]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const body = w.querySelector('.session-body');"
        "  const r = body.getBoundingClientRect();"
        "  let x = r.left + r.width / 2, y = r.top + r.height / 2;"
        "  const send = (type, cx, cy) => {"
        "    const a = new Touch({ identifier: 1, target: body, clientX: cx - 40, clientY: cy });"
        "    const b = new Touch({ identifier: 2, target: body, clientX: cx + 40, clientY: cy });"
        "    const live = type === 'touchend' ? [] : [a, b];"
        "    body.dispatchEvent(new TouchEvent(type, {"
        "      touches: live, targetTouches: live, changedTouches: [a, b],"
        "      bubbles: true, cancelable: true }));"
        "  };"
        "  send('touchstart', x, y);"
        "  for (let s = 0; s < steps; s++) { x += dx / steps; y += dy / steps; send('touchmove', x, y); }"
        "  send('touchend', x, y);"
        "}",
        [i, dx, dy, steps],
    )


def viewport_y(page, i=0):
    """The terminal's scroll position.

    CONTEXT.md -> Testing conventions: `term.buffer.active.viewportY`, never
    `.xterm-viewport.scrollTop` — the latter reads 0 in both directions here and
    would pass whether the fix works or not.
    """
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._term.term.buffer.active.viewportY", i
    )


def touch_drag_titlebar(page, i, dx, dy, steps=8):
    """Drag the i-th console by its titlebar with a TOUCH pointer.

    `pointerType: 'touch'` is the point: the handler used to be bound to
    `mousedown`, which iOS synthesizes only after a tap resolves and never
    during a drag, so a finger on the titlebar ran the text selection instead.
    """
    return page.evaluate(
        "([i, dx, dy, steps]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const bar = w.querySelector('.session-titlebar');"
        "  const r = bar.getBoundingClientRect();"
        "  let x = r.left + 30, y = r.top + r.height / 2;"
        "  const ev = (type, target, cx, cy, buttons) => target.dispatchEvent("
        "    new PointerEvent(type, { pointerId: 7, pointerType: 'touch', isPrimary: true,"
        "      button: 0, buttons, clientX: cx, clientY: cy, bubbles: true, cancelable: true }));"
        "  ev('pointerdown', bar, x, y, 1);"
        "  for (let s = 0; s < steps; s++) {"
        "    x += dx / steps; y += dy / steps;"
        "    ev('pointermove', document, x, y, 1);"
        "  }"
        "  ev('pointerup', document, x, y, 0);"
        "  return { left: w.offsetLeft, top: w.offsetTop };"
        "}",
        [i, dx, dy, steps],
    )


def win_box(page, i):
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " return { left: w.offsetLeft, top: w.offsetTop }; }",
        i,
    )


def touch_drag_el(page, i, selector, dx, dy, steps=8):
    """Drag an element inside the i-th window with a TOUCH pointer."""
    return page.evaluate(
        "([i, sel, dx, dy, steps]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const h = w.querySelector(sel);"
        "  const r = h.getBoundingClientRect();"
        "  let x = r.left + r.width / 2, y = r.top + r.height / 2;"
        "  const ev = (type, target, cx, cy, buttons) => target.dispatchEvent("
        "    new PointerEvent(type, { pointerId: 9, pointerType: 'touch', isPrimary: true,"
        "      button: 0, buttons, clientX: cx, clientY: cy, bubbles: true, cancelable: true }));"
        "  ev('pointerdown', h, x, y, 1);"
        "  for (let s = 0; s < steps; s++) {"
        "    x += dx / steps; y += dy / steps;"
        "    ev('pointermove', document, x, y, 1);"
        "  }"
        "  ev('pointerup', document, x, y, 0);"
        "  return { w: w.offsetWidth, h: w.offsetHeight };"
        "}",
        [i, selector, dx, dy, steps],
    )


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


            # --- Scenario 11: the drag belongs to the terminal ------------------
            open_console(page, slug)
            t_i = page.locator(".session-window").count() - 1
            check("11 a fresh console for the gesture", wait_child_ready(page, t_i), "READY on screen")
            page.evaluate(
                "(i) => { const t = document.querySelectorAll('.session-window')[i]._term.term;"
                " const nl = String.fromCharCode(13, 10);"
                " let s = ''; for (let n = 0; n < 400; n++) s += 'scrollback ' + n + nl;"
                " t.write(s); }",
                t_i,
            )
            page.wait_for_timeout(1500)
            ws_before = page.evaluate(
                "() => { const w = document.getElementById('workspace');"
                " return { x: w.scrollLeft, y: w.scrollTop,"
                "          room: w.scrollWidth > w.clientWidth + 1 || w.scrollHeight > w.clientHeight + 1 }; }"
            )
            check(
                "11 the canvas HAS room to pan, so leaving it still is not vacuous",
                ws_before["room"],
                f"{ws_before}",
            )
            vy_before = viewport_y(page, t_i)
            check(
                "11 …and the terminal has scrollback to move through",
                vy_before > 0,
                f"viewportY={vy_before}",
            )
            drag(page, t_i, 240)
            page.wait_for_timeout(800)
            vy_after = viewport_y(page, t_i)
            ws_after = page.evaluate(
                "() => { const w = document.getElementById('workspace');"
                " return { x: w.scrollLeft, y: w.scrollTop }; }"
            )
            check(
                "11 dragging DOWN scrolls the terminal back through its scrollback",
                vy_after < vy_before,
                f"viewportY {vy_before} -> {vy_after}",
            )
            check(
                "11 …and the canvas underneath does not move a pixel",
                ws_after == {"x": ws_before["x"], "y": ws_before["y"]},
                f"{ws_before} -> {ws_after}",
            )
            drag(page, t_i, -240)
            page.wait_for_timeout(800)
            vy_back = viewport_y(page, t_i)
            check(
                "11 …and dragging UP comes back down again",
                vy_back > vy_after,
                f"viewportY {vy_after} -> {vy_back}",
            )
            # The canvas sits at 0/0, so only an UPWARD drag could move it into
            # positive scroll — the direction in which "it did not move" is a
            # real claim rather than a clamp.
            check(
                "11 …with the canvas still untouched after a drag it COULD have panned",
                page.evaluate(
                    "() => { const w = document.getElementById('workspace');"
                    " return w.scrollLeft === 0 && w.scrollTop === 0; }"
                ),
            )
            check(
                "11 the terminal declares the gesture its own in the stylesheet",
                page.evaluate(
                    "(i) => getComputedStyle(document.querySelectorAll('.session-window')[i]"
                    ".querySelector('.session-body')).touchAction",
                    t_i,
                )
                == "none",
                "WebKit honours only auto/none/manipulation — `pinch-zoom` there reads as `auto`",
            )


            # --- Scenario 12: a finger moves a window ---------------------------
            # Un-maximize everything first: a maximized window refuses to drag by
            # design, so measuring one would pass for the wrong reason.
            page.evaluate(
                "() => { for (const w of document.querySelectorAll('.session-window.maximized'))"
                " w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(500)
            d_i = 0
            before_box = win_box(page, d_i)
            touch_drag_titlebar(page, d_i, 120, 60)
            page.wait_for_timeout(400)
            after_box = win_box(page, d_i)
            check(
                "12 a touch drag on the titlebar moves the window",
                after_box != before_box,
                f"{before_box} -> {after_box}",
            )
            check(
                "12 …by roughly the distance the finger travelled",
                abs((after_box["left"] - before_box["left"]) - 120) < 12
                and abs((after_box["top"] - before_box["top"]) - 60) < 12,
                f"{before_box} -> {after_box}",
            )
            bar_css = page.evaluate(
                "(i) => { const cs = getComputedStyle(document.querySelectorAll('.session-window')[i]"
                ".querySelector('.session-titlebar'));"
                " return { touchAction: cs.touchAction, webkitUserSelect: cs.webkitUserSelect }; }",
                d_i,
            )
            check(
                "12 the titlebar claims the gesture, or the browser cancels it as a scroll",
                bar_css["touchAction"] == "none",
                f"{bar_css}",
            )
            check(
                "12 …and refuses to be selected, which is what a long press did instead",
                bar_css["webkitUserSelect"] == "none",
                f"{bar_css}",
            )
            # The drop is persisted, not just painted.
            page.reload(wait_until="domcontentloaded")
            page.wait_for_timeout(5000)
            page.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            check(
                "12 …and the move survives a reload, so the desk recorded it",
                win_box(page, d_i) == after_box,
                f"{after_box} -> {win_box(page, d_i)}",
            )

            # --- Scenario 13: a maximized console is not buried by a reload -----
            n_wins = page.locator(".session-window").count()
            check("13 more than one console is on the plane", n_wins > 1, f"windows={n_wins}")
            page.evaluate(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " if (!w.classList.contains('maximized')) w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(600)
            page.reload(wait_until="domcontentloaded")
            page.wait_for_timeout(6000)
            page.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            page.wait_for_timeout(1000)
            stack = page.evaluate(
                "() => [...document.querySelectorAll('.session-window')].map((w) => ({"
                " max: w.classList.contains('maximized'),"
                " z: parseInt(w.style.zIndex, 10) || 0 }))"
            )
            maxed = [w for w in stack if w["max"]]
            others = [w for w in stack if not w["max"]]
            check(
                "13 the maximized console came back maximized",
                len(maxed) == 1,
                f"{stack}",
            )
            check(
                "13 …and nothing restored after it is painted on top",
                bool(maxed) and bool(others) and maxed[0]["z"] > max(w["z"] for w in others),
                f"{stack}",
            )
            page.evaluate(
                "() => { const w = document.querySelector('.session-window.maximized');"
                " if (w) w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(500)


            # --- Scenario 14: a finger can resize, not just barely -------------
            # A fresh TOUCH profile, because the band sizes are a media query and
            # the desktop context above will never match it.
            grip_ctx = browser.new_context(
                viewport={"width": 1024, "height": 768}, has_touch=True, is_mobile=False
            )
            grip = desk_page(grip_ctx, settle=7000)
            grip.locator(".session-window .xterm").first.wait_for(timeout=15000)
            grip.wait_for_timeout(1200)
            grip.evaluate(
                "() => { for (const w of document.querySelectorAll('.session-window.maximized'))"
                " w.querySelector('.session-max').click(); }"
            )
            grip.wait_for_timeout(600)
            bands = grip.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " const se = w.querySelector('.session-handle.h-se').getBoundingClientRect();"
                " const e = w.querySelector('.session-handle.h-e').getBoundingClientRect();"
                " const vis = w.querySelector('.session-resize').getBoundingClientRect();"
                " return { seW: se.width, seH: se.height, eW: e.width,"
                "          coversGrip: se.left <= vis.left + 1 && se.top <= vis.top + 1 }; }"
            )
            check(
                "14 the corner band is a fingertip, not a cursor's pixel",
                bands["seW"] >= 24 and bands["seH"] >= 24,
                f"{bands}",
            )
            check(
                "14 …the edge bands grew with it",
                bands["eW"] >= 12,
                f"{bands}",
            )
            check(
                "14 …and the band covers the visible grip the operator aims at",
                bands["coversGrip"],
                f"{bands}",
            )
            # The key bar must keep its own pixels: the handles overlap the bottom
            # edge, and grown to a finger's size they would otherwise swallow the
            # lower half of every button in the row. Asked BEFORE the resize
            # below, while the window is still known to be inside the viewport —
            # `elementFromPoint` answers null off-screen, which is not a verdict
            # (CONTEXT.md -> Testing conventions: prove the element was visible).
            keys_win = grip.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " const b = w.querySelector('.session-key[data-key=\"font-up\"]');"
                " if (!b) return { ok: false, why: 'no key bar' };"
                " const r = b.getBoundingClientRect();"
                " if (b.offsetParent === null || r.width <= 0) return { ok: false, why: 'not painted' };"
                " const x = r.left + r.width / 2, y = r.bottom - 4;"
                " if (x < 0 || y < 0 || x > innerWidth || y > innerHeight)"
                "   return { ok: false, why: 'off-screen' };"
                " const hit = document.elementFromPoint(x, y);"
                " return { ok: !!hit && hit.closest('.session-key') === b,"
                "          hit: hit ? String(hit.className || hit.tagName) : null }; }"
            )
            check(
                "14 the key bar owns its buttons where a resize handle overlaps them",
                bool(keys_win and keys_win.get("ok")),
                f"elementFromPoint over the last button = {keys_win}",
            )
            before_size = grip.evaluate(
                "() => { const w = document.querySelector('.session-window');"
                " return { w: w.offsetWidth, h: w.offsetHeight }; }"
            )
            after_size = touch_drag_el(grip, 0, ".session-handle.h-se", 90, 70)
            check(
                "14 a finger on the corner actually resizes the window",
                after_size["w"] > before_size["w"] and after_size["h"] > before_size["h"],
                f"{before_size} -> {after_size}",
            )
            grip_ctx.close()

            # --- Scenario 15: fullscreen is offered per ENGINE ------------------
            # `navigator.vendor` is spoofed rather than launching WebKit: the
            # decision under test is a string comparison, and running the whole
            # daemon under a second engine would change a dozen variables to
            # measure one. What this proves is the WIRING — that the pure rule
            # tabled in wb-console.test.mjs is the thing the button reads.
            check(
                "15 an engine that can hold fullscreen gets the button",
                page.locator(".session-window").first.locator(".session-full").is_visible(),
            )
            wk_ctx = browser.new_context(viewport={"width": 1024, "height": 768})
            wk_ctx.add_init_script(
                "Object.defineProperty(navigator, 'vendor', { get: () => 'Apple Computer, Inc.' });"
            )
            wk = desk_page(wk_ctx, settle=7000)
            check(
                "15 the spoof took",
                wk.evaluate("navigator.vendor") == "Apple Computer, Inc.",
            )
            open_console(wk, slug)
            check(
                "15 WebKit is not offered a fullscreen the keyboard would cancel",
                wk.locator(".session-window").first.locator(".session-full").is_hidden(),
            )
            check(
                "15 …and maximize, the honest control there, is still built",
                wk.locator(".session-window").first.locator(".session-max").is_visible(),
            )
            wk_ctx.close()

            # --- Scenario 16: the finger is the trackpad, whoever owns the wheel --
            open_console(page, slug)
            a_i = page.locator(".session-window").count() - 1
            check("16 a console for the app-driven gesture", wait_child_ready(page, a_i), "READY on screen")
            page.evaluate(
                "(i) => { const t = document.querySelectorAll('.session-window')[i]._term.term;"
                " const nl = String.fromCharCode(13, 10);"
                " let s = ''; for (let n = 0; n < 200; n++) s += 'history ' + n + nl;"
                " t.write(s); }",
                a_i,
            )
            page.wait_for_timeout(1200)
            record_sends(page, a_i)
            esc = "String.fromCharCode(27)"
            # The APP asks for the mouse (DECSET 1000 + SGR 1006), as a TUI does.
            page.evaluate(
                f"(i) => document.querySelectorAll('.session-window')[i]._term.term.write({esc} + '[?1000h' + {esc} + '[?1006h')",
                a_i,
            )
            page.wait_for_timeout(300)
            check(
                "16 the terminal is tracking the mouse for the app",
                page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._term.term.modes.mouseTrackingMode", a_i)
                != "none",
            )
            clear_sent(page)
            vy0 = viewport_y(page, a_i)
            drag(page, a_i, 120)
            page.wait_for_timeout(900)
            reports = [d for d in sent(page) if d.startswith("[<64;") or d.startswith("[<65;")]
            check(
                "16 a drag under a tracking app puts WHEEL REPORTS on the socket",
                len(reports) >= 3,
                f"{len(reports)} reports, first {reports[:1]!r}",
            )
            check(
                "16 …dragging DOWN is wheel UP (button 64), as on the trackpad",
                reports and all(r.startswith("[<64;") for r in reports),
                f"{reports[:2]!r}",
            )
            check(
                "16 …and the viewport did NOT move through the history",
                viewport_y(page, a_i) == vy0,
                f"viewportY {vy0} -> {viewport_y(page, a_i)}",
            )
            # The app lets the mouse go and enters the ALTERNATE buffer.
            page.evaluate(
                f"(i) => document.querySelectorAll('.session-window')[i]._term.term.write({esc} + '[?1006l' + {esc} + '[?1000l' + {esc} + '[?1049h')",
                a_i,
            )
            page.wait_for_timeout(300)
            check(
                "16 the alternate buffer is active",
                page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._term.term.buffer.active.type", a_i)
                == "alternate",
            )
            clear_sent(page)
            drag(page, a_i, 120)
            page.wait_for_timeout(900)
            arrows = [d for d in sent(page) if d in ("[A", "OA")]
            check(
                "16 a drag in the alternate buffer arrives as ARROW KEYS",
                len(arrows) >= 3,
                f"{len(arrows)} arrows of {len(sent(page))} frames",
            )
            # Back to the plain buffer: the viewport is the owner again.
            page.evaluate(
                f"(i) => document.querySelectorAll('.session-window')[i]._term.term.write({esc} + '[?1049l')",
                a_i,
            )
            page.wait_for_timeout(300)
            clear_sent(page)
            vy1 = viewport_y(page, a_i)
            drag(page, a_i, 120)
            page.wait_for_timeout(900)
            check(
                "16 …and in the plain buffer the same drag moves the viewport and sends nothing",
                viewport_y(page, a_i) < vy1 and not sent(page),
                f"viewportY {vy1} -> {viewport_y(page, a_i)}, sent={sent(page)[:2]!r}",
            )

            # --- Scenario 17: two fingers pan the canvas ------------------------
            # Leave the alternate buffer behind (scenario 16 restored the plain
            # one) and make sure nothing is maximized, or there is nowhere to pan.
            page.evaluate(
                "() => { for (const w of document.querySelectorAll('.session-window.maximized'))"
                " w.querySelector('.session-max').click(); }"
            )
            page.wait_for_timeout(400)
            # Start from a scrolled plane, so a pan in EITHER direction is a real
            # claim and not a clamp at 0.
            page.evaluate("() => { const w = document.getElementById('workspace'); w.scrollLeft = 200; w.scrollTop = 150; }")
            page.wait_for_timeout(200)
            ws0 = page.evaluate("() => { const w = document.getElementById('workspace'); return { x: w.scrollLeft, y: w.scrollTop }; }")
            check("17 the plane starts scrolled, with room both ways", ws0["x"] > 0 and ws0["y"] > 0, f"{ws0}")
            clear_sent(page)
            vy_p = viewport_y(page, a_i)
            drag2(page, a_i, 120, 80)
            page.wait_for_timeout(500)
            ws1 = page.evaluate("() => { const w = document.getElementById('workspace'); return { x: w.scrollLeft, y: w.scrollTop }; }")
            check(
                "17 two fingers dragged right and down pan the canvas left and up by that much",
                abs((ws0["x"] - ws1["x"]) - 120) <= 2 and abs((ws0["y"] - ws1["y"]) - 80) <= 2,
                f"{ws0} -> {ws1}",
            )
            check(
                "17 …and the terminal under the fingers did not scroll",
                viewport_y(page, a_i) == vy_p,
                f"viewportY {vy_p} -> {viewport_y(page, a_i)}",
            )
            check("17 …and nothing reached the socket", not sent(page), f"{sent(page)[:2]!r}")
            check(
                "17 …and the plane's grab state was released with the fingers",
                not page.evaluate("() => document.getElementById('stage').classList.contains('panning')"),
            )
            # Under maxlock there is nowhere to go.
            page.locator(".session-window").nth(a_i).locator(".session-max").click()
            page.wait_for_timeout(500)
            check(
                "17 a maximized console locks the plane",
                page.evaluate("() => document.getElementById('workspace').classList.contains('maxlock')"),
            )
            wsm = page.evaluate("() => { const w = document.getElementById('workspace'); return { x: w.scrollLeft, y: w.scrollTop }; }")
            drag2(page, a_i, 120, 80)
            page.wait_for_timeout(500)
            check(
                "17 …and two fingers there move nothing",
                page.evaluate("() => { const w = document.getElementById('workspace'); return { x: w.scrollLeft, y: w.scrollTop }; }") == wsm,
                f"{wsm}",
            )
            page.locator(".session-window").nth(a_i).locator(".session-max").click()
            page.wait_for_timeout(300)

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
