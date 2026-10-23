"""#598 / #689 browser check: a late socket of a tab evicts nobody.

One REAL daemon on a scratch `RALPHY_DAEMON_DIR` (the operator's own desk and
login policy are untouched), with `RALPHY_DAEMON_AGENT_OVERRIDE` set to
`session_test_child`. The desk holds one console. Every `/ws/session` socket of
page A is routed through Playwright, and the route can hold a socket (no
upgrade reaches the daemon) or stop forwarding one (the daemon side stays
open: a half-open socket).

Late socket (the #598 acceptance)
1  The window reconnects; the route HOLDS that first socket, so its upgrade
   never reaches the daemon. The window reconnects again, and the second
   socket attaches with a higher attempt.
2  Only then does the first socket's upgrade reach the daemon: the check
   opens its exact url (same holder, its lower attempt) from the page. A
   socket the page opens itself is used because a socket the page already
   closed cannot be connected by the route afterwards. The daemon answers
   it with `session-end` reason `superseded`.
3  The writer socket is not ended (no `taken-over`), the window shows no
   take-over prompt, and a typed line reaches the child (`GOT:`).
4  The daemon log has the refusal of the late socket.

Half-open reclaim
5  The route stops forwarding the writer socket without closing it on the
   daemon side; the window reconnects. The daemon ends the half-open socket
   as `taken-over`, the window shows no prompt, and a typed line reaches
   the child.

Lost heartbeat (#751)
7  The route stops forwarding both the writer socket and the presence
   socket `/ws`, and keeps both open on the daemon side. The page stays
   visible and the check sends no resume. Once the heartbeat is stale, the
   page resumes by itself: a new console socket opens, the window shows no
   prompt, and a typed line reaches the child.

Real take over
6  A second browser context (another tab: another holder) shows the
   take-over prompt; its "Take over" button gives it the console: a line it
   types reaches the child, and page A shows the prompt.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_late_socket_598.py   (exit 0 = all pass)
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright
from playwright._impl import _network

# Playwright 1.60 reads `event["code"]` of a page close, and a close with no
# code (the workbench's own `close()`) has none: the close handler then raises
# a `KeyError` instead of running.
_close_page = _network.WebSocketRoute._channel_close_page


def _close_page_with_no_code(self, event):
    _close_page(self, {"code": None, "reason": None, **event})


_network.WebSocketRoute._channel_close_page = _close_page_with_no_code

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7533
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")

TAG_COMMAND = 0x02
# `resume` ignores a second call within 1.5 s (wb-resume.ts).
DEBOUNCE_MS = 1700

BUFFER = """(sel) => { const w = document.querySelector(sel); const b = w?._term?.term?.buffer.active; if (!b) return '';
  let out = ''; for (let y = 0; y < b.length; y++) out += (b.getLine(y)?.translateToString(true) ?? '') + '\\n';
  return out; }"""
WIN = "#stage .session-window"

results = []
# Every routed socket of page A, in order.
routed = []
# What the route does with the next socket: "forward" or "hold".
mode = {"next": "forward"}
# Every presence socket `/ws` of page A, in order.
presence = []


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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild or the browser loads an old bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True)


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb598_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb598@example.com"],
        ["git", "config", "user.name", "wb598"],
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


def write_desk(daemon_dir, slug):
    Path(daemon_dir, "desk.toml").write_bytes(
        (
            "[[windows]]\n"
            'id = "w-1"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            'consoleName = "w-1"\n'
            "ts = 100\n"
            "rect = { left = 40, top = 40, width = 680, height = 500 }\n"
        ).encode("utf-8")
    )


def launch(daemon_dir, log_path):
    empty = tempfile.mkdtemp(prefix="wb598_empty_")
    env = dict(
        os.environ,
        RUST_LOG="info",
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_DAEMON_AGENT_OVERRIDE=CHILD,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=open(log_path, "wb")
    )


ANSI = re.compile(r"\x1b\[[0-9;]*m")


def log_lines(log_path, needle):
    text = ANSI.sub("", Path(log_path).read_text(encoding="utf-8", errors="replace"))
    return [line for line in text.splitlines() if needle in line]


def attempt_of(url):
    values = urllib.parse.parse_qs(urllib.parse.urlparse(url).query).get("attempt")
    return int(values[0]) if values else None


def route(ws):
    """Forward the socket, or hold it so that its upgrade never reaches the daemon.

    A forwarded socket records its `session-open` and the reason of its
    `session-end`, also after `forward` is turned off: then nothing passes in
    either direction. A close handler is set only to make a socket half-open
    (`half_open`): a handler that is set turns off the close forwarding."""
    rec = {"url": ws.url, "held": mode["next"] == "hold", "open": None, "end": None, "forward": True, "ws": ws}
    routed.append(rec)
    if rec["held"]:
        # No `connect_to_server`: the page's socket stays CONNECTING, and the
        # daemon never sees its upgrade.
        return
    server = ws.connect_to_server()

    def from_server(message):
        data = message if isinstance(message, (bytes, bytearray)) else message.encode("utf-8")
        if data and data[0] == TAG_COMMAND:
            try:
                msg = json.loads(bytes(data[1:]).decode("utf-8"))
            except ValueError:
                msg = {}
            if msg.get("verb") == "session-open":
                rec["open"] = msg.get("payload") or {}
            elif msg.get("verb") == "session-end":
                rec["end"] = (msg.get("payload") or {}).get("reason")
        if rec["forward"]:
            ws.send(message)

    def from_page(message):
        if rec["forward"]:
            server.send(message)

    server.on_message(from_server)
    ws.on_message(from_page)


def route_presence(ws):
    """Forward the presence socket until `forward` is turned off."""
    rec = {"url": ws.url, "forward": True, "ws": ws}
    presence.append(rec)
    server = ws.connect_to_server()
    server.on_message(lambda m: rec["forward"] and ws.send(m))
    ws.on_message(lambda m: rec["forward"] and server.send(m))


def half_open(rec):
    """Stop passing anything, and keep the daemon side open when the page closes."""
    rec["forward"] = False
    rec["ws"].on_close(lambda code, reason: None)


def pump(page, until, timeout_s=15):
    """Playwright delivers socket events only while the page is being waited on."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if until():
            return True
        page.wait_for_timeout(50)
    return until()


def screen(page):
    return page.evaluate(BUFFER, WIN)


def type_line(page, text):
    page.evaluate("(t) => document.querySelector('#stage .session-window')._term.term.paste(t + '\\r')", text)


def reached_child(page, token, timeout_s=15):
    return pump(page, lambda: any(token in l for l in screen(page).split("\n") if l.startswith("GOT:")), timeout_s)


def parked(page):
    return page.locator(f"{WIN} .session-parked").count()


def resume(page):
    page.wait_for_timeout(DEBOUNCE_MS)
    return page.evaluate("() => document.querySelector('#stage .session-window')._term.resume(true)")


def open_page(ctx):
    page = ctx.new_page()
    page.goto(BASE)
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate("() => Alpine.store('tabs').activate('consoles')")
    page.wait_for_function(f"() => !!document.querySelector('{WIN}')", timeout=20000)
    return page


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb598_daemon_")
    log_path = os.path.join(daemon_dir, "daemon-stderr.log")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    write_desk(daemon_dir, slug)
    proc = launch(daemon_dir, log_path)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            ctx_a = browser.new_context(viewport={"width": 1400, "height": 900})
            ctx_a.route_web_socket(re.compile(r"/ws/session"), route)
            ctx_a.route_web_socket(re.compile(r"/ws(\?|$)"), route_presence)
            page = open_page(ctx_a)
            page.wait_for_function(
                f"() => document.querySelector('{WIN}')._term?.sessionId != null", timeout=20000
            )
            type_line(page, "HELLO-598")
            check("the child echoed the first line", reached_child(page, "HELLO-598"))

            # --- late socket ------------------------------------------------
            # The upgrade of the first socket stalls. Playwright opens a socket it
            # does not connect, so the window sees an open socket that never
            # answers; the next resume finds it stale and opens a second one.
            mode["next"] = "hold"
            known = len(routed)
            if not resume(page):
                check("the window reconnects", False)
                sys.exit(1)
            pump(page, lambda: len(routed) > known)
            first = routed[known] if len(routed) > known else None
            mode["next"] = "forward"
            if not resume(page):
                check("the window replaces the silent socket", False)
                sys.exit(1)
            pump(page, lambda: len(routed) > known + 1 and routed[known + 1]["open"] is not None)
            second = routed[known + 1] if len(routed) > known + 1 else None
            a1 = attempt_of(first["url"]) if first else None
            a2 = attempt_of(second["url"]) if second else None
            check(
                "1 the first socket was held, and the second attached with a higher attempt",
                first and first["held"] and second and second["open"] is not None and a1 and a2 and a2 > a1,
                f"first={first and first['url']} second={second and second['url']}",
            )

            late_index = len(routed)
            page.evaluate("(u) => { window.__late = new WebSocket(u); window.__late.binaryType = 'arraybuffer'; }", first["url"])
            pump(page, lambda: len(routed) > late_index and routed[late_index]["end"] is not None)
            late = routed[late_index] if len(routed) > late_index else None
            check(
                "2 the late upgrade is refused as superseded",
                late and late["url"] == first["url"] and late["end"] == "superseded",
                f"late={late and {'url': late['url'], 'end': late['end'], 'open': late['open']}}",
            )
            page.wait_for_timeout(800)
            type_line(page, "LATE-598")
            check(
                "3 the writer is not taken over, no prompt, and it still writes",
                second["end"] is None and parked(page) == 0 and reached_child(page, "LATE-598"),
                f"end={second['end']} parked={parked(page)}",
            )
            refused = [l for l in log_lines(log_path, "console socket refused") if "superseded" in l]
            check("4 the daemon logged the late socket as refused", len(refused) == 1, f"lines={refused}")

            # --- half-open reclaim -------------------------------------------
            half_open(second)
            known = len(routed)
            if not resume(page):
                check("the window reconnects after a half-open drop", False)
                sys.exit(1)
            pump(page, lambda: len(routed) > known and routed[known]["open"] is not None and second["end"] is not None)
            third = routed[known] if len(routed) > known else None
            page.wait_for_timeout(500)
            type_line(page, "HALF-598")
            check(
                "5 a half-open socket is reclaimed with no prompt",
                third
                and third["open"] is not None
                and third["end"] is None
                and second["end"] == "taken-over"
                and parked(page) == 0
                and reached_child(page, "HALF-598"),
                f"old end={second['end']} new open={third and third['open'] is not None} parked={parked(page)}",
            )

            # --- lost heartbeat (#751) ----------------------------------------
            # A desktop that slept with the tab in front: no page event, and
            # sockets that read OPEN with no byte to come.
            page.wait_for_timeout(DEBOUNCE_MS)
            half_open(third)
            for rec in presence:
                half_open(rec)
            known = len(routed)
            stale = pump(page, lambda: page.evaluate("() => Alpine.store('connection').presenceStale"), 15)
            pump(page, lambda: len(routed) > known and routed[known]["open"] is not None, 15)
            fourth = routed[known] if len(routed) > known else None
            page.wait_for_timeout(500)
            type_line(page, "BEAT-751")
            check(
                "7 a lost heartbeat resumes the console with no page event",
                stale
                and fourth is not None
                and fourth["open"] is not None
                and parked(page) == 0
                and reached_child(page, "BEAT-751"),
                f"stale={stale} new open={fourth and fourth['open'] is not None} parked={parked(page)}",
            )
            writer = fourth or third

            # --- real take over -----------------------------------------------
            ctx_b = browser.new_context(viewport={"width": 1400, "height": 900})
            page_b = open_page(ctx_b)
            prompt = page_b.locator(f'{WIN} [data-act="take-over"]')
            try:
                prompt.wait_for(timeout=30000)
                shown = True
            except Exception:
                shown = False
            if shown:
                prompt.click()
                page_b.wait_for_timeout(1500)
                type_line(page_b, "TAKE-598")
            took = shown and reached_child(page_b, "TAKE-598")
            a_parked = pump(page, lambda: parked(page) == 1, 15)
            check(
                "6 another tab sees the prompt, and its Take over works",
                took and a_parked and writer["end"] == "taken-over",
                f"prompt={shown} B wrote={took} A parked={a_parked} A end={writer and writer['end']}",
            )
            ctx_b.close()
            ctx_a.close()
            browser.close()
    finally:
        stop(proc)
    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
