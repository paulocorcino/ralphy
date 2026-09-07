"""ADR-0055 browser acceptance: a pasted IMAGE becomes a clipboard drop, and the
drop's PATH is what gets pasted into the console.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry — the operator's own desk and login policy are untouched).
`RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a `session_test_child`, which
echoes each line it receives as `GOT:<line>` — so "the path was typed" and "no
newline was typed with it" are both read off the child, not off the widget.

The paste is synthetic: a `ClipboardEvent("paste")` carrying a `DataTransfer`
with a real PNG `File`, dispatched on xterm's own textarea — the exact node and
event the browser fires for Ctrl+V — so every scenario drives the real path:
paste listener -> `image.write` over /ws/command -> daemon sniff + confined
write -> reply path -> `term.paste` -> PTY -> child.

Scenario 1  a PNG paste lands under `<repo>/.ralphy-clipboard/` with the PNG
            magic intact, and the path is typed into the console WITHOUT a
            newline: no `GOT:` line until Enter is pressed, then exactly one,
            carrying the path
Scenario 2  a text-only paste falls through to xterm's own paste, unchanged
Scenario 3  HTML bytes labelled `image/png` are refused as `not an image`,
            nothing lands, and the refusal is printed in the console
Scenario 4  a watcher's image paste is refused client-side: no verb is sent,
            nothing lands, nothing is typed
Scenario 5  an image past the cap is refused client-side as `too large`, and
            no verb is sent
Scenario 6  the detached-fence popup carries the verb bridge (`window.WBDaemon`)
            the paste listener needs — the consequence ADR-0055 names, checked
            on the popup document itself

No image fixture is checked in: the PNG is hand-encoded (zlib + CRC32) so the
daemon's magic check is exercised against real bytes.

Requires `playwright` (`pip install playwright`, then `playwright install chromium`).

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/console-image-paste-2026-09-07.png.
Run: python crates/ralphy-daemon/tests/wb_console_image_paste.py   (exit 0 = all pass)
"""

import base64
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import zlib
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7449
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "console-image-paste-2026-09-07.png"

DROP_DIR = ".ralphy-clipboard"
CAP = 4 * 1024 * 1024

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def info(name, detail):
    print(f"[INFO] {name} {detail}", flush=True)


def png_bytes(width, height, rgb=(232, 217, 168)):
    """A real, decodable RGB PNG — signature, IHDR, IDAT, IEND with live CRCs."""

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(rgb) * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


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
    empty = tempfile.mkdtemp(prefix="wbpaste_empty_")
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
    d = tempfile.mkdtemp(prefix="wbpaste_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n\nThe console image-paste fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbpaste@example.com"],
        ["git", "config", "user.name", "wbpaste"],
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


def press_enter(page, i=0):
    page.evaluate(
        "(i) => document.querySelectorAll('.session-window')[i]._term.term.paste('\\r')",
        i,
    )


def paste_event(page, i, mime, b64, name="shot.png"):
    """Dispatch a synthetic `paste` on xterm's textarea carrying ONE item of
    `mime` — a File for `image/*`, a string for text. Returns whether the
    listener claimed it (`defaultPrevented`)."""
    return page.evaluate(
        "([i, mime, b64, name]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const ta = w._term.term.textarea;"
        "  const dt = new DataTransfer();"
        "  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));"
        "  if (mime.startsWith('image/')) {"
        "    dt.items.add(new File([bytes], name, { type: mime }));"
        "  } else {"
        "    dt.setData(mime, new TextDecoder().decode(bytes));"
        "  }"
        "  const ev = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true });"
        "  ta.dispatchEvent(ev);"
        "  return ev.defaultPrevented;"
        "}",
        [i, mime, b64, name],
    )


def paste_big_image(page, i, size):
    """Dispatch an image paste whose File is `size` bytes (built in-page, so the
    harness never base64s megabytes across the bridge)."""
    return page.evaluate(
        "([i, size]) => {"
        "  const w = document.querySelectorAll('.session-window')[i];"
        "  const ta = w._term.term.textarea;"
        "  const bytes = new Uint8Array(size);"
        "  bytes.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);"
        "  const dt = new DataTransfer();"
        "  dt.items.add(new File([bytes], 'huge.png', { type: 'image/png' }));"
        "  const ev = new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true });"
        "  ta.dispatchEvent(ev);"
        "  return ev.defaultPrevented;"
        "}",
        [i, size],
    )


def drops(fixture_dir):
    d = Path(fixture_dir) / DROP_DIR
    return sorted(p.name for p in d.iterdir()) if d.is_dir() else []


def wait_for(fn, timeout=10000, step=200):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if fn():
            return True
        time.sleep(step / 1000)
    return fn()


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
    daemon_dir = tempfile.mkdtemp(prefix="wbpaste_reg_")
    fixture_dir = make_fixture_repo()
    slug = register_fixture(daemon_dir, fixture_dir)
    info("fixture", f"{slug} at {fixture_dir}")

    png = png_bytes(24, 16)
    png_b64 = base64.b64encode(png).decode("ascii")

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE):
            check("daemon listening", False, BASE)
            sys.exit(1)
        check("daemon listening", True, BASE)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            page = ctx.new_page()
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_timeout(1200)

            open_console(page, slug)
            check("the child booted", wait_child_ready(page), "READY on screen")
            check("no drop before any paste", drops(fixture_dir) == [], f"{drops(fixture_dir)}")

            # --- Scenario 1: a PNG paste lands, and the PATH is typed sans newline
            claimed = paste_event(page, 0, "image/png", png_b64)
            landed = wait_for(lambda: len(drops(fixture_dir)) == 1)
            names = drops(fixture_dir)
            check("1 the paste listener claimed the image", claimed)
            check("1 one drop landed under .ralphy-clipboard/", landed and len(names) == 1, f"{names}")
            on_disk = (Path(fixture_dir) / DROP_DIR / names[0]).read_bytes() if names else b""
            check(
                "1 the drop is the pasted PNG, byte for byte",
                on_disk == png and names[0].startswith("paste-") and names[0].endswith(".png"),
                f"{names[0] if names else None} {len(on_disk)}B",
            )
            page.wait_for_timeout(1200)
            before_enter = got_lines(page)
            check(
                "1 nothing was executed: no GOT: line before Enter",
                not any(DROP_DIR in l for l in before_enter),
                f"{before_enter}",
            )
            press_enter(page)
            expected = f"{DROP_DIR}/{names[0]}" if names else "<none>"
            typed = wait_for(lambda: any(expected in l for l in got_lines(page)), 15000)
            after = [l for l in got_lines(page) if DROP_DIR in l]
            check(
                "1 Enter reveals exactly one GOT: line carrying the drop's path",
                typed and len(after) == 1 and after[0] == f"GOT:{expected}",
                f"{after}",
            )

            # --- Scenario 2: text paste falls through to xterm ------------------
            claimed = paste_event(page, 0, "text/plain", base64.b64encode(b"plain-text-paste\r").decode())
            arrived = wait_for(lambda: any("plain-text-paste" in l for l in got_lines(page)), 15000)
            check(
                "2 a text paste is xterm's own, unchanged",
                (not claimed) and arrived and len(drops(fixture_dir)) == 1,
                f"claimed={claimed} drops={drops(fixture_dir)}",
            )

            # --- Scenario 3: HTML dressed as an image is refused -----------------
            html_b64 = base64.b64encode(b"<html><script>x</script></html>").decode()
            claimed = paste_event(page, 0, "image/png", html_b64, name="evil.png")
            refused = wait_for(lambda: "[paste refused — not an image]" in screen(page), 10000)
            check(
                "3 HTML labelled image/png is refused as not an image, nothing lands",
                claimed and refused and len(drops(fixture_dir)) == 1,
                f"drops={drops(fixture_dir)}",
            )

            # --- Scenario 4: a watcher's paste is refused client-side -------------
            ctx_b = browser.new_context(viewport={"width": 1280, "height": 800})
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
                "4 the second context really is a watcher",
                page_b.evaluate("() => document.querySelectorAll('.session-window')[0]._term.watching"),
            )
            claimed = paste_event(page_b, 0, "image/png", png_b64)
            nudged = wait_for(
                lambda: page_b.evaluate(
                    "() => !!document.querySelector('.session-window .session-parked.is-nudged')"
                ),
                5000,
            )
            page_b.wait_for_timeout(1000)
            check(
                "4 the watcher's image paste is claimed and refused VISIBLY (the parked strip nudges)",
                claimed and nudged,
                f"claimed={claimed} nudged={nudged}",
            )
            check(
                "4 …and nothing lands, nothing is typed",
                len(drops(fixture_dir)) == 1 and "[paste refused" not in screen(page_b),
                f"drops={drops(fixture_dir)}",
            )
            ctx_b.close()

            # --- Scenario 5: past the cap, refused before any verb ---------------
            claimed = paste_big_image(page, 0, CAP + 1)
            refused = wait_for(lambda: "[paste refused — too large]" in screen(page), 10000)
            check(
                "5 an image past the cap is refused as too large, nothing lands",
                claimed and refused and len(drops(fixture_dir)) == 1,
                f"drops={drops(fixture_dir)}",
            )

            # --- Scenario 6: the detached-fence popup carries the verb bridge -----
            popup = ctx.new_page()
            popup.goto(BASE + "detached-fence.html", wait_until="load")
            popup.wait_for_timeout(500)
            bridged = popup.evaluate(
                "() => !!window.WBDaemon && typeof window.WBDaemon.write === 'function'"
                " && !!window.WBConsole && typeof window.WBConsole.pasteDecision === 'function'"
            )
            check("6 the detached-fence popup loads WBDaemon beside the consoles module", bridged)
            popup.close()

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
