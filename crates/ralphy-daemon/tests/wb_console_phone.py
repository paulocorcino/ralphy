"""Browser acceptance: the console on a PHONE — full bleed, paste, line selection.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry — the operator's own desk and login policy are untouched).
`RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a `session_test_child`, which
echoes each line it receives as `GOT:<line>` — the oracle for what a paste
really put on the wire, and the text a selection has to pick up.

The pure rules (`pasteOffered`, `phoneBleed`, `selectionRow`) are tabled in
`ui-tests/wb-console.test.mjs`; this script proves the WIRING those tables
cannot reach: the body class, the media query, the key-bar buttons, the touch
listeners, and xterm's `selectLines`/`paste` on the real terminal.

Scenario 1  the copy key wears `bi-copy` and the paste key wears
            `bi-clipboard` — the clipboard glyph means paste, not copy
Scenario 2  a tap on `paste` puts the clipboard's text on the wire WITHOUT
            executing it: the child sees nothing until Enter, then `GOT:` it
Scenario 3  a watcher's paste is refused — the child never sees it
Scenario 4  on a phone viewport, maximizing folds the rail, the sidebar and the
            tab strip away; the console spans the viewport and gains columns;
            restore brings every one of them back
Scenario 5  the same maximize on a desktop viewport keeps the rail — the gate
            is the width, not the class
Scenario 6  `sel` arms one drag: dragging across two rows selects those two
            lines (not the third), the terminal does not scroll, the arming
            drops at finger-up, and `copy` then writes exactly those lines
Scenario 7  with `sel` off, the same drag is a scroll and selects nothing

Headless Chromium proves the wiring only; Safari's paste permission bubble and
the on-screen keyboard surviving a non-passive `touchstart` are the operator's
to confirm on hardware.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/console-phone-2026-09-19.png.
Run: python crates/ralphy-daemon/tests/wb_console_phone.py   (exit 0 = all pass)
"""

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

PORT = 7452
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "console-phone-2026-09-19.png"

PHONE = {"width": 390, "height": 844}
DESKTOP = {"width": 1280, "height": 800}

# Installed before any page script: wraps the clipboard write so a scenario can
# assert what the console ASKED to store, and still forwards to the real API.
RECORDER = """
window.__clipWrites = [];
const api = navigator.clipboard;
if (api && api.writeText) {
  const real = api.writeText.bind(api);
  api.writeText = (t) => { window.__clipWrites.push(t); return real(t); };
}
"""

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
    empty = tempfile.mkdtemp(prefix="wbphone_empty_")
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
    d = tempfile.mkdtemp(prefix="wbphone_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n\nThe console phone fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbphone@example.com"],
        ["git", "config", "user.name", "wbphone"],
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


def got_lines(page, i=0):
    return [l for l in screen(page, i).split("\n") if l.startswith("GOT:")]


def type_line(page, i, text):
    """Feed one line through xterm's own data path, as ONE onData event."""
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def clip_writes(page):
    return page.evaluate("() => window.__clipWrites.slice()")


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


def tap(page, i, name):
    """Click a key-bar button the way a finger does — through the real listener."""
    page.evaluate(
        "([i, name]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const b = w.querySelector(`.session-key[data-key='${name}']`);"
        "  if (!b) throw new Error('no key ' + name);"
        "  if (b.disabled) throw new Error('key disabled: ' + name);"
        "  b.click();"
        "}",
        [i, name],
    )


def key_icon(page, i, name):
    return page.evaluate(
        "([i, name]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const b = w.querySelector(`.session-key[data-key='${name}'] i`);"
        "  return b ? b.className : null; }",
        [i, name],
    )


def key_attr(page, i, name, attr):
    return page.evaluate(
        "([i, name, attr]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const b = w.querySelector(`.session-key[data-key='${name}']`);"
        "  if (!b) return null;"
        "  return attr === 'disabled' ? b.disabled : b.getAttribute(attr); }",
        [i, name, attr],
    )


def painted(page, selector):
    """Whether the element is actually PAINTED, not merely present in the DOM.

    CONTEXT.md → Testing conventions: a visibility assertion has to prove the
    element was visible, or `display: none` reads as a pass.
    """
    return page.evaluate(
        "(sel) => { const el = document.querySelector(sel);"
        "  return !!el && el.offsetParent !== null && el.clientWidth > 0; }",
        selector,
    )


def viewport_y(page, i=0):
    """The terminal's scroll position — `term.buffer.active.viewportY`, never
    `.xterm-viewport.scrollTop` (CONTEXT.md → Testing conventions)."""
    return page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._term.term.buffer.active.viewportY", i
    )


def cols(page, i=0):
    return page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._term.term.cols", i)


def row_center_y(page, i, needle):
    """The client Y of the middle of the on-screen row whose text contains
    `needle`, or None if that line is not on screen."""
    return page.evaluate(
        "([i, needle]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const term = w._term.term; const b = term.buffer.active;"
        "  let hit = -1;"
        "  for (let y = 0; y < b.length; y++) {"
        "    const l = b.getLine(y); if (l && l.translateToString(true).includes(needle)) { hit = y; break; }"
        "  }"
        "  if (hit < 0) return null;"
        "  const row = hit - b.viewportY;"
        "  if (row < 0 || row >= term.rows) return null;"
        "  const screen = w.querySelector('.xterm-screen');"
        "  const r = screen.getBoundingClientRect();"
        "  const cell = term.element.clientHeight / term.rows;"
        "  return r.top + (row + 0.5) * cell; }",
        [i, needle],
    )


def touch_drag_y(page, i, y0, y1, steps=6):
    """One finger over the i-th console's body from client y0 to y1."""
    page.evaluate(
        "([i, y0, y1, steps]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const body = w.querySelector('.session-body');"
        "  const r = body.getBoundingClientRect();"
        "  const x = r.left + r.width / 2;"
        "  let y = y0;"
        "  const send = (type, cy) => {"
        "    const t = new Touch({ identifier: 1, target: body, clientX: x, clientY: cy });"
        "    body.dispatchEvent(new TouchEvent(type, {"
        "      touches: type === 'touchend' ? [] : [t],"
        "      targetTouches: type === 'touchend' ? [] : [t],"
        "      changedTouches: [t], bubbles: true, cancelable: true }));"
        "  };"
        "  send('touchstart', y);"
        "  for (let s = 0; s < steps; s++) { y += (y1 - y0) / steps; send('touchmove', y); }"
        "  send('touchend', y);"
        "}",
        [i, y0, y1, steps],
    )


def selection(page, i=0):
    return page.evaluate(
        "(i) => { const t = document.querySelectorAll('.session-window')[i]._term.term;"
        " return { has: t.hasSelection(), text: t.getSelection() }; }",
        i,
    )


def maximize(page, i=0):
    page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " if (!w.classList.contains('maximized')) w.querySelector('.session-max').click(); }",
        i,
    )


def restore(page, i=0):
    page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " if (w.classList.contains('maximized')) w.querySelector('.session-max').click(); }",
        i,
    )


def win_width(page, i=0):
    return page.evaluate(
        "(i) => { const w = document.querySelectorAll('.session-window')[i];"
        " return [w.getBoundingClientRect().width, window.innerWidth]; }",
        i,
    )


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
    daemon_dir = tempfile.mkdtemp(prefix="wbphone_reg_")
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

            # ---- the phone: narrow, touch, the key bar shows by default --------
            ctx = browser.new_context(viewport=PHONE, has_touch=True, is_mobile=True)
            ctx.grant_permissions(["clipboard-read", "clipboard-write"], origin=BASE.rstrip("/"))
            ctx.add_init_script(RECORDER)
            page = ctx.new_page()
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_timeout(1200)

            open_console(page, slug)
            check("the child booted", wait_child_ready(page), "READY on screen")
            check(
                "the key bar is up on the phone (the premise of every tap below)",
                painted(page, ".session-window .session-keys"),
            )

            # --- Scenario 1: honest icons --------------------------------------
            copy_icon = key_icon(page, 0, "copy") or ""
            paste_icon = key_icon(page, 0, "paste") or ""
            check("1 copy wears bi-copy", "bi-copy" in copy_icon.split(), copy_icon)
            check("1 paste wears bi-clipboard", "bi-clipboard" in paste_icon.split(), paste_icon)
            check(
                "1 paste is enabled on a secure origin (loopback)",
                key_attr(page, 0, "paste", "disabled") is False,
            )

            # --- Scenario 2: paste puts the text on the wire, does not execute --
            page.evaluate("() => navigator.clipboard.writeText('hello phone')")
            before = len(got_lines(page))
            tap(page, 0, "paste")
            page.wait_for_timeout(1200)
            check(
                "2 the paste alone executes nothing (no trailing newline)",
                len(got_lines(page)) == before,
                f"{got_lines(page)[before:]}",
            )
            type_line(page, 0, "")  # Enter, on its own
            ok = wait_for(lambda: any("hello phone" in l for l in got_lines(page)[before:]), 8000)
            check("2 …and Enter makes the child echo what was pasted", ok, f"{got_lines(page)[before:]}")

            # --- Scenario 4: the phone bleed -----------------------------------
            check("4 before maximize the rail is painted", painted(page, ".rail"))
            check("4 before maximize the tab strip is painted", painted(page, ".tabbar"))
            cols_before = cols(page)
            maximize(page)
            page.wait_for_function("() => document.body.classList.contains('console-max')", timeout=5000)
            page.wait_for_timeout(700)  # the grid transition + the refit
            w, vw = win_width(page)
            check("4 body carries console-max", True)
            check("4 the rail folds away", not painted(page, ".rail"))
            check("4 the sidebar folds away", not painted(page, ".side"))
            check("4 the tab strip folds away", not painted(page, ".tabbar"))
            check("4 the console spans the whole viewport", abs(w - vw) < 2, f"window={w} viewport={vw}")
            cols_max = cols(page)
            check("4 the terminal gained columns", cols_max > cols_before, f"{cols_before} → {cols_max}")
            # The screenshot: the bleed, with the key bar (browser-driven
            # verification is what a screenshot is for).
            page.screenshot(path=os.path.join(SHOT_DIR, SHOT))
            restore(page)
            page.wait_for_function("() => !document.body.classList.contains('console-max')", timeout=5000)
            page.wait_for_timeout(700)
            check("4 restore brings the rail back", painted(page, ".rail"))
            check("4 restore brings the tab strip back", painted(page, ".tabbar"))
            check("4 …and the columns come back", cols(page) == cols_before, f"{cols(page)} vs {cols_before}")

            # --- Scenario 6: line selection by one drag -------------------------
            for word in ("alpha", "bravo", "charlie"):
                type_line(page, 0, word)
            ok = wait_for(lambda: any("GOT:charlie" in l for l in got_lines(page)), 8000)
            check("6 three known lines are on screen", ok, f"{got_lines(page)[-3:]}")
            tap(page, 0, "select")
            check("6 the sel key reads pressed once armed", key_attr(page, 0, "select", "aria-pressed") == "true")
            # Measured AFTER arming: the tap focuses the terminal, and focusing
            # the textarea scrolls both the workspace (the rows move) and the
            # buffer (xterm rides to the bottom). A finger reads the screen it
            # sees, so the test must too.
            page.wait_for_timeout(200)
            vy0 = viewport_y(page)
            y_a = row_center_y(page, 0, "GOT:alpha")
            y_b = row_center_y(page, 0, "GOT:bravo")
            check("6 both target rows are on screen", y_a is not None and y_b is not None, f"{y_a} {y_b}")
            touch_drag_y(page, 0, y_a, y_b)
            page.wait_for_timeout(300)
            sel = selection(page)
            check("6 the drag made a selection", sel["has"], repr(sel["text"]))
            check(
                "6 the selection is the two dragged lines, whole",
                "GOT:alpha" in sel["text"] and "GOT:bravo" in sel["text"] and "GOT:charlie" not in sel["text"],
                repr(sel["text"]),
            )
            check("6 the terminal did not scroll under the finger", viewport_y(page) == vy0, f"{vy0} → {viewport_y(page)}")
            check("6 finger-up disarms the mode", key_attr(page, 0, "select", "aria-pressed") == "false")
            check("6 copy lights up with the selection", key_attr(page, 0, "copy", "disabled") is False)
            n = len(clip_writes(page))
            tap(page, 0, "copy")
            ok = wait_for(lambda: len(clip_writes(page)) > n, 5000)
            copied = clip_writes(page)[-1] if ok else None
            check(
                "6 copy writes exactly those lines",
                ok and "GOT:alpha" in copied and "GOT:bravo" in copied and "GOT:charlie" not in copied,
                repr(copied),
            )

            # --- Scenario 7: with sel off, the drag is a scroll -----------------
            page.evaluate("(i) => document.querySelectorAll('.session-window')[i]._term.term.clearSelection()", 0)
            # Fill the scrollback so there is something to scroll.
            for k in range(40):
                type_line(page, 0, f"fill{k}")
            wait_for(lambda: any("GOT:fill39" in l for l in got_lines(page)), 10000)
            page.wait_for_timeout(300)
            vy_bottom = viewport_y(page)
            body_box = page.locator(".session-window").first.locator(".session-body").bounding_box()
            y_mid = body_box["y"] + body_box["height"] / 2
            touch_drag_y(page, 0, y_mid, y_mid + 120)
            page.wait_for_timeout(300)
            check("7 sel off: the drag scrolls", viewport_y(page) < vy_bottom, f"{vy_bottom} → {viewport_y(page)}")
            check("7 sel off: nothing is selected", not selection(page)["has"])

            # ---- a desktop context on the same desk: watcher + width control ---
            ctx_b = browser.new_context(viewport=DESKTOP)
            ctx_b.grant_permissions(["clipboard-read", "clipboard-write"], origin=BASE.rstrip("/"))
            ctx_b.add_init_script(RECORDER)
            page_b = ctx_b.new_page()
            page_b.goto(BASE, wait_until="domcontentloaded")
            page_b.wait_for_timeout(1500)
            page_b.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            page_b.wait_for_function(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " return !!w && !!w._term && w._term.watching === true; }",
                timeout=15000,
            )
            check(
                "the desktop context is a watcher, the phone still holds the baton",
                page_b.evaluate("() => document.querySelectorAll('.session-window')[0]._term.watching")
                and not page.evaluate("() => document.querySelectorAll('.session-window')[0]._term.watching"),
            )

            # --- Scenario 3: a watcher's paste is refused -----------------------
            # The desktop has no key bar by default; turn it on for the tap.
            page_b.evaluate(
                "() => Alpine.$data(document.querySelector('[x-data]')).saveSetting('consoles.key_bar', 'on')"
            )
            page_b.wait_for_timeout(400)
            page_b.evaluate("() => navigator.clipboard.writeText('from the watcher')")
            before = len(got_lines(page))
            tap(page_b, 0, "paste")
            page_b.wait_for_timeout(800)
            type_line(page, 0, "")  # Enter from the baton holder: flushes anything that DID arrive
            page.wait_for_timeout(1200)
            check(
                "3 the watcher's paste never reaches the child",
                not any("from the watcher" in l for l in got_lines(page)[before:]),
                f"{got_lines(page)[before:]}",
            )

            # --- Scenario 5: a desktop maximize keeps the chrome ---------------
            maximize(page_b)
            page_b.wait_for_function("() => document.body.classList.contains('console-max')", timeout=5000)
            page_b.wait_for_timeout(500)
            check("5 desktop: body carries console-max too (the class is unconditional)", True)
            check("5 desktop: the rail stays painted", painted(page_b, ".rail"))
            check("5 desktop: the tab strip stays painted", painted(page_b, ".tabbar"))
            restore(page_b)
            page_b.wait_for_timeout(300)
            ctx_b.close()
            ctx.close()
            browser.close()
    finally:
        stop(proc)

    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed", flush=True)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
