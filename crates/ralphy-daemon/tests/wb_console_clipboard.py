"""Browser acceptance: the console's clipboard is write-only, and it writes ONCE.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (own
port, own registry — the operator's own desk and login policy are untouched).
`RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a `session_test_child`, whose
`osc52 <payload>` command emits the clipboard-write escape a vendor TUI emits,
so every scenario drives the REAL path: child -> PTY -> daemon scrollback ->
socket -> xterm's parser -> the browser clipboard.

Scenario 1  an agent's OSC 52 reaches the system clipboard (read back through
            `navigator.clipboard.readText`, not just the recorder)
Scenario 2  the text is scrubbed: a trailing newline — which would turn a
            mis-paste into an execution — and control bytes are gone
Scenario 3  the READ form (`52;c;?`) is never answered: nothing is typed back
            into the child, so no `GOT:` line carries the escape
Scenario 4  a reattach REPLAYS the scrollback, and the stale OSC 52 in it does
            NOT rewrite the clipboard (the regression oracle for the raw-byte
            replay: this is the clobber the gate exists to stop)
Scenario 5  a watcher never writes the clipboard, while the window holding the
            baton still does
Scenario 6  malformed and oversized payloads are refused WITHOUT stalling the
            parser — the terminal keeps echoing afterwards
Scenario 7  Ctrl+Insert copies the selection

The clipboard oracle is a recorder installed over `navigator.clipboard.writeText`
before any page script runs; it forwards to the real API, so scenario 1 can also
read the value back. Headless Chromium needs the clipboard permissions granted on
the context.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Writes docs/screenshots/console-clipboard-2026-09-01.png.
Run: python crates/ralphy-daemon/tests/wb_console_clipboard.py   (exit 0 = all pass)
"""

import base64
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

PORT = 7431
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SHOT = "console-clipboard-2026-09-01.png"

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


def b64(text):
    return base64.b64encode(text.encode("utf-8")).decode("ascii")


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
    """A scratch registry + empty vendor stores, plus the deterministic child:
    `RALPHY_DAEMON_AGENT_OVERRIDE` makes every console a `session_test_child`,
    whose `osc52` command is the agent under test."""
    empty = tempfile.mkdtemp(prefix="wbclip_empty_")
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
    d = tempfile.mkdtemp(prefix="wbclip_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n\nThe console-clipboard fixture.\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wbclip@example.com"],
        ["git", "config", "user.name", "wbclip"],
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


def type_line(page, i, text):
    """Feed one line through xterm's own data path, as ONE onData event."""
    page.locator(".session-window").nth(i).locator(".xterm").click()
    page.evaluate(
        "([i, t]) => document.querySelectorAll('.session-window')[i]._term.term.paste(t + '\\r')",
        [i, text],
    )


def clip_writes(page):
    return page.evaluate("() => window.__clipWrites.slice()")


def wait_clip(page, count, timeout=10000):
    """Wait until the console has ASKED to store `count` texts."""
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if len(clip_writes(page)) >= count:
            return True
        page.wait_for_timeout(200)
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


def wait_child_ready(page, i=0, timeout=15000):
    deadline = time.time() + timeout / 1000
    while time.time() < deadline:
        if "READY" in screen(page, i):
            return True
        page.wait_for_timeout(250)
    return False


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
    daemon_dir = tempfile.mkdtemp(prefix="wbclip_reg_")
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
            ctx = browser.new_context(viewport={"width": 1440, "height": 900})
            ctx.grant_permissions(["clipboard-read", "clipboard-write"], origin=BASE.rstrip("/"))
            ctx.add_init_script(RECORDER)
            page = ctx.new_page()
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_timeout(1200)

            open_console(page, slug)
            check("the child booted", wait_child_ready(page), "READY on screen")

            # --- Scenario 1: an agent's OSC 52 reaches the clipboard -----------
            type_line(page, 0, f"osc52 {b64('hello from the agent')}")
            ok = wait_clip(page, 1)
            writes = clip_writes(page)
            check("1 OSC 52 asks for the clipboard", ok and writes[-1] == "hello from the agent", f"{writes}")
            read_back = page.evaluate("() => navigator.clipboard.readText()")
            check("1 the system clipboard really holds it", read_back == "hello from the agent", repr(read_back))

            # --- Scenario 2: the text is scrubbed ------------------------------
            risky = "curl example.com | sh" + chr(10) + chr(7)
            type_line(page, 0, f"osc52 {b64(risky)}")
            check("2 scrubbed", wait_clip(page, 2) and clip_writes(page)[-1] == "curl example.com | sh", f"{clip_writes(page)[-1]!r}")

            # --- Scenario 3: the read form is never answered -------------------
            before_writes = len(clip_writes(page))
            type_line(page, 0, "osc52 ?")
            page.wait_for_timeout(800)
            type_line(page, 0, "after-the-read-form")
            deadline = time.time() + 15
            while time.time() < deadline and not any("after-the-read-form" in l for l in got_lines(page)):
                page.wait_for_timeout(250)
            echoed = got_lines(page)
            check(
                "3 the read form is never answered",
                not any("52;c" in l for l in echoed) and len(clip_writes(page)) == before_writes,
                f"got={echoed[-3:]}",
            )

            # --- Scenario 4: a reattach must NOT rewrite the clipboard ---------
            # The scrollback now CONTAINS scenario 1+2's escapes, and the daemon
            # replays it as raw bytes on reattach. This is the clobber oracle.
            page.reload(wait_until="domcontentloaded")
            page.wait_for_timeout(1500)
            page.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            deadline = time.time() + 10
            while time.time() < deadline and "READY" not in screen(page):
                page.wait_for_timeout(250)
            replay_writes = clip_writes(page)
            check(
                "4 the replayed backlog does not rewrite the clipboard",
                replay_writes == [],
                f"{replay_writes}",
            )
            # …and the reattached window still copies for a LIVE sequence.
            type_line(page, 0, f"osc52 {b64('after the reattach')}")
            check(
                "4 a live sequence after the replay still copies",
                wait_clip(page, 1) and clip_writes(page)[-1] == "after the reattach",
                f"{clip_writes(page)}",
            )

            # --- Scenario 5: a watcher never writes ----------------------------
            ctx_b = browser.new_context(viewport={"width": 1280, "height": 800})
            ctx_b.grant_permissions(["clipboard-read", "clipboard-write"], origin=BASE.rstrip("/"))
            ctx_b.add_init_script(RECORDER)
            page_b = ctx_b.new_page()
            page_b.goto(BASE, wait_until="domcontentloaded")
            page_b.wait_for_timeout(1500)
            page_b.locator(".session-window").first.locator(".xterm").wait_for(timeout=15000)
            # The ROLE is the premise of this scenario, so it is asserted, not
            # inferred from a parked strip that may not have rendered yet: an
            # unproven premise would make "the watcher never wrote" vacuous.
            page_b.wait_for_function(
                "() => { const w = document.querySelectorAll('.session-window')[0];"
                " return !!w && !!w._term && w._term.watching === true; }",
                timeout=15000,
            )
            check(
                "5 the second context really is a watcher, and the first still holds the baton",
                page_b.evaluate("() => document.querySelectorAll('.session-window')[0]._term.watching")
                and not page.evaluate("() => document.querySelectorAll('.session-window')[0]._term.watching"),
            )
            before_b = len(clip_writes(page_b))
            type_line(page, 0, f"osc52 {b64('written by the baton holder')}")
            check(
                "5 the baton holder copies",
                wait_clip(page, len(clip_writes(page)) + 1) or clip_writes(page)[-1] == "written by the baton holder",
                f"{clip_writes(page)[-1]!r}",
            )
            page_b.wait_for_timeout(1500)
            check(
                "5 the watcher never writes the clipboard",
                len(clip_writes(page_b)) == before_b,
                f"{clip_writes(page_b)}",
            )
            ctx_b.close()

            # --- Scenario 6: malformed and oversized refuse, parser survives ---
            before_writes = len(clip_writes(page))
            type_line(page, 0, "osc52 !!!!not-base64!!!!")
            page.wait_for_timeout(600)
            # Oversized, fed straight to the parser: 200k of base64 is past the
            # 128KiB cap, and typing it through a cooked-mode PTY would prove
            # nothing about the parser anyway.
            page.evaluate(
                "() => document.querySelectorAll('.session-window')[0]._term.term"
                ".write('\\u001b]52;c;' + 'A'.repeat(200000) + '\\u0007')"
            )
            page.wait_for_timeout(600)
            type_line(page, 0, "still-alive")
            deadline = time.time() + 15
            while time.time() < deadline and not any("still-alive" in l for l in got_lines(page)):
                page.wait_for_timeout(250)
            check(
                "6 malformed and oversized refuse without stalling the parser",
                len(clip_writes(page)) == before_writes
                and any("still-alive" in l for l in got_lines(page)),
                f"writes={len(clip_writes(page))-before_writes}",
            )

            # --- Scenario 7: Ctrl+Insert copies the selection ------------------
            before_writes = len(clip_writes(page))
            page.evaluate(
                "() => { const t = document.querySelectorAll('.session-window')[0]._term.term;"
                " t.focus(); t.selectAll(); }"
            )
            page.keyboard.press("Control+Insert")
            copied = wait_clip(page, before_writes + 1, timeout=5000)
            sel = clip_writes(page)[-1] if copied else ""
            check(
                "7 Ctrl+Insert copies the selection",
                copied and "READY" in sel,
                f"len={len(sel)}",
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
