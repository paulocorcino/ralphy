"""#689 browser check: a gzip replay, and the live frames that wait behind it.

One REAL daemon on a scratch `RALPHY_DAEMON_DIR` (the operator's own desk and
login policy are untouched), with `RALPHY_DAEMON_AGENT_OVERRIDE` set to
`session_test_child`. The desk holds one console.

The page's `DecompressionStream` is wrapped so that each decode ends 1.5 s
late. Every `/ws/session` socket is routed through Playwright. On a socket
whose `session-open` says `replay_encoding: "gzip"`, the route forwards the
replay frame, and the check then sends two live frames of its own (`MARK-A`,
`MARK-B`) on that socket, so both arrive while the replay is still decoding.
A send made inside the route's message handler did not reach the page,
and a listener the page adds does not see a frame the route sends
(Playwright sync API, measured 2026-10-12). So the marks go out from the
main loop, and the page's clock is read right after the sends.

1  The page asked for gzip (`gzip=1` in the socket URL).
2  The reattach's `session-open` says the replay is gzip.
3  The page decoded it (the wrapper counted a decode).
4  The marks were sent before the decode ended.
5  The terminal was written the replay (with `GOT:HELLO-689`) first, and
   `MARK-A` then `MARK-B` last: the live frames were written after the
   replay, in order.
   The writes are counted, not the rows: a snapshot repaints from the top of
   the screen, so rows alone do not show the order.
6  The daemon's traffic line for that socket has `replay_raw_bytes` larger
   than `replay_bytes`.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_gzip_689.py   (exit 0 = all pass)
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7532
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")

TAG_TERMINAL = 0x01
TAG_COMMAND = 0x02

# Each decode ends 1.5 s after its stream starts; `__decodedAt` is when.
SLOW_DECODE = """(() => {
  const Real = globalThis.DecompressionStream;
  if (typeof Real !== 'function') return;
  window.__decodes = 0;
  globalThis.DecompressionStream = class extends Real {
    constructor(format) {
      super(format);
      window.__decodes += 1;
      this.__late = super.readable.pipeThrough(new TransformStream({
        start: () => new Promise((resolve) => setTimeout(() => {
          window.__decodedAt = performance.now();
          resolve();
        }, 1500)),
      }));
    }
    get readable() { return this.__late; }
  };
})();"""

HELPERS = """() => {
  window.__W = () => document.querySelector('#stage .session-window');
}"""
BUFFER = """() => { const b = __W()?._term?.term?.buffer.active; if (!b) return null;
  const out = []; for (let y = 0; y < b.length; y++) out.push(b.getLine(y)?.translateToString(true) ?? '');
  return out; }"""

# Note each write to the terminal that holds the replay text or a mark, in order.
WRITES = """() => {
  const t = __W()._term.term;
  const write = t.write.bind(t);
  window.__writes = [];
  t.write = (data, done) => {
    const text = typeof data === 'string' ? data : new TextDecoder().decode(data);
    for (const [needle, label] of [['GOT:HELLO-689', 'replay'], ['MARK-A', 'MARK-A'], ['MARK-B', 'MARK-B']])
      if (text.includes(needle)) window.__writes.push(label);
    return write(data, done);
  };
}"""

results = []
# Every routed socket: its URL, its `session-open` payload, the session id
# bytes of its gzip replay once that went out, and its route.
routed = []


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
    d = tempfile.mkdtemp(prefix="wb689g_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb689@example.com"],
        ["git", "config", "user.name", "wb689"],
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
    empty = tempfile.mkdtemp(prefix="wb689g_empty_")
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
FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')


def traffic_lines(log_path):
    text = ANSI.sub("", Path(log_path).read_text(encoding="utf-8", errors="replace"))
    out = []
    for line in text.splitlines():
        if "console socket traffic" in line:
            out.append({k: v.strip('"') for k, v in FIELD.findall(line)})
    return out


def route(ws):
    rec = {"url": ws.url, "open": None, "replay_of": None, "ws": ws}
    routed.append(rec)
    server = ws.connect_to_server()

    def from_server(message):
        ws.send(message)
        data = message if isinstance(message, (bytes, bytearray)) else message.encode("utf-8")
        if not data:
            return
        if data[0] == TAG_COMMAND and rec["open"] is None:
            try:
                msg = json.loads(bytes(data[1:]).decode("utf-8"))
            except ValueError:
                return
            if msg.get("verb") == "session-open":
                rec["open"] = msg.get("payload") or {}
        elif (
            data[0] == TAG_TERMINAL
            and rec["replay_of"] is None
            and (rec["open"] or {}).get("replay_encoding") == "gzip"
        ):
            rec["replay_of"] = bytes(data[1:9])

    server.on_message(from_server)


def terminal_frame(session, text):
    return bytes([TAG_TERMINAL]) + session + text.encode("utf-8")


def wait_for(page, js, timeout_s=15, arg=None):
    try:
        page.wait_for_function(js, arg=arg, timeout=int(timeout_s * 1000))
        return True
    except Exception:
        return False


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb689g_daemon_")
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
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            page.add_init_script(SLOW_DECODE)
            page.route_web_socket(re.compile(r"/ws/session"), route)
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.evaluate("() => Alpine.store('tabs').activate('consoles')")
            page.wait_for_function("() => !!document.querySelector('#stage .session-window')", timeout=20000)
            page.evaluate(HELPERS)
            page.wait_for_function("() => __W()?._term?.term?.cols > 0 && __W()._term.sessionId != null", timeout=20000)
            first = routed[0]["url"] if routed else None
            check("1 the page asks for gzip", first and "gzip=1" in first, f"url={first}")

            page.evaluate("() => __W().querySelector('.xterm-helper-textarea').focus()")
            page.keyboard.type("HELLO-689")
            page.keyboard.press("Enter")
            saw = wait_for(page, f"() => ({BUFFER})().some((l) => l.includes('GOT:HELLO-689'))")
            check("the child echoed the line", saw)

            page.evaluate(WRITES)
            # A notice of the page in the terminal: the terminal no longer
            # matches the session's bytes, so the reconnect gets the snapshot,
            # not a resume of the bytes it missed.
            page.evaluate("() => __W()._term.term.write('')")
            known = len(routed)
            if not page.evaluate("() => __W()._term.resume(true)"):
                check("the terminal reconnects", False)
                sys.exit(1)
            # Playwright delivers socket events only while the page is being waited on.
            deadline = time.time() + 15
            while time.time() < deadline and not (len(routed) > known and routed[known]["replay_of"]):
                page.wait_for_timeout(20)
            rec = routed[known] if len(routed) > known else None
            check(
                "2 the reattach's session-open says gzip",
                rec and (rec["open"] or {}).get("replay_encoding") == "gzip",
                f"open={rec and rec['open']}",
            )
            if rec and rec["replay_of"]:
                rec["ws"].send(terminal_frame(rec["replay_of"], "\r\nMARK-A\r\n"))
                rec["ws"].send(terminal_frame(rec["replay_of"], "MARK-B\r\n"))
            sent_at = page.evaluate("() => performance.now()")
            wait_for(page, "() => window.__decodedAt != null && window.__writes.includes('MARK-B')")
            page.wait_for_timeout(300)
            check("3 the page decoded the replay", page.evaluate("() => window.__decodes") >= 1)
            decoded_at = page.evaluate("() => window.__decodedAt ?? null")
            check(
                "4 the marks were sent before the decode ended",
                decoded_at is not None and sent_at < decoded_at,
                f"sent at {sent_at:.0f} ms, decoded at {decoded_at} ms",
            )
            writes = page.evaluate("() => window.__writes")
            # A live repaint of the screen (ConPTY sends one after a resize) also
            # holds the text, so "replay" can appear more than once.
            check(
                "5 the replay is written first, then MARK-A, then MARK-B",
                writes[:1] == ["replay"] and "MARK-A" in writes and writes[writes.index("MARK-A") :] == ["MARK-A", "MARK-B"],
                f"writes={writes}",
            )
            page.close()
            browser.close()

        # The reattach socket logs its traffic line when it closes.
        line = None
        deadline = time.time() + 15
        while time.time() < deadline and line is None:
            line = next((t for t in traffic_lines(log_path) if int(t.get("replay_bytes") or 0) > 0), None)
            if line is None:
                time.sleep(0.3)
        raw = int((line or {}).get("replay_raw_bytes") or 0)
        wire = int((line or {}).get("replay_bytes") or 0)
        check("6 the traffic line has a raw replay larger than the wire replay", raw > wire > 0, f"raw={raw} wire={wire}")
    finally:
        stop(proc)
    passed = sum(results)
    print(f"\n{passed}/{len(results)} checks passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
