"""One socket per page, and 14 consoles that resume over a slow link (#740).

The page holds ONE WebSocket, `/ws`. Each console is a `console` channel of
it: the page sends `channel.open {channel, kind: "console", ...}` on channel 0,
and the daemon answers with channel frames `[0x04][channel: u32 BE][flags][piece]`.
The pieces of a channel, joined, are one frame of the console codec
(`0x01` terminal, `0x02` command JSON).

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR` (the
operator's own desk and login policy are untouched), with
`RALPHY_DAEMON_AGENT_OVERRIDE` set to `session_test_child`. PORT 7740. The
desk holds 14 consoles, all inside the viewport. Chromium's network is
emulated through CDP (`Network.emulateNetworkConditions`) with a latency of
600 ms. Each console gets a marker line, which the child echoes as `GOT:`.

The page is then hidden, as a locked phone hides it: another tab comes to
the front, and the network goes offline for longer than the stale time of
the heartbeat (6 s), so the old socket carries nothing. Then the network
comes back and the page is shown again.

B1  the page opened exactly one WebSocket at load and exactly one at the resume, both `/ws`
B2  every console resumed on a channel of the new socket, shows its marker line again, and echoes a new line
B3  no console shows `[session closed]`

The other console checks import `PageSockets` and the frame helpers from
here: they find a console by its channel on `/ws`, not by a URL.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_one_socket_740.py   (exit 0 = all pass)
"""

import json
import os
import struct
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")

TAG_TERMINAL = 0x01
TAG_COMMAND = 0x02
TAG_CHANNEL = 0x04
FLAG_MORE = 0x01
# The tag, the channel id and the flags.
CHANNEL_HEADER = 6
# A terminal frame is a tag and an 8-byte session id before the bytes.
FRAME_HEADER = 9


def as_bytes(message):
    """A socket message of Playwright: `bytes` for a binary frame, `str` for text."""
    if isinstance(message, (bytes, bytearray)):
        return bytes(message)
    return str(message).encode("utf-8")


def command_of(data):
    """The `{verb, payload}` of a `[0x02][JSON]` frame, or None."""
    if not data or data[0] != TAG_COMMAND:
        return None
    try:
        msg = json.loads(data[1:].decode("utf-8"))
    except ValueError:
        return None
    return msg if isinstance(msg, dict) else None


def encode_command(verb, payload):
    """A `[0x02][JSON]` command frame, as the page writes one (`encodeCommand`)."""
    return bytes([TAG_COMMAND]) + json.dumps({"id": 0, "verb": verb, "payload": payload}).encode("utf-8")


def channel_piece(data):
    """`(channel, more, piece)` of a channel frame, or None."""
    if len(data) < CHANNEL_HEADER or data[0] != TAG_CHANNEL:
        return None
    (channel,) = struct.unpack(">I", data[1:5])
    return channel, bool(data[5] & FLAG_MORE), data[CHANNEL_HEADER:]


def channel_frame(channel, inner):
    """One channel frame that carries all of `inner` (at most one piece)."""
    return bytes([TAG_CHANNEL]) + struct.pack(">I", channel) + bytes([0]) + inner


def is_page_socket(url):
    return urllib.parse.urlparse(url).path.endswith("/ws")


class Joiner:
    """Joins the pieces of each channel of one socket into inner frames."""

    def __init__(self):
        self.pending = {}

    def push(self, data):
        """`(channel, inner)` when `data` ends an inner frame, else None."""
        piece = channel_piece(data)
        if piece is None:
            return None
        channel, more, body = piece
        self.pending.setdefault(channel, []).append(body)
        if more:
            return None
        return channel, b"".join(self.pending.pop(channel))


class PageSockets:
    """Every WebSocket a page opened, and the console channels of its `/ws`.

    `attach(page)` listens to `framesent` and `framereceived`, so it changes
    nothing on the wire. A check that routes the page socket through
    Playwright feeds `opened`, `sent`, `received` and `closed` itself.

    A console channel is a dict: `n` (its index), `socket` (the index of its
    page socket), `id`, `payload` (the open), `session` (the `id` of the open,
    None for a launch), `since`, `t0`, `open` (the payload of its first
    `session-open`), `end` (the reason of its `session-end`), `closed`,
    `first_frame_ms`, `first_frame_bytes`, `replay_bytes` (the size of the
    first terminal frame after a `session-open` that says `replay`) and
    `replay_session` (the 8-byte session id of that frame)."""

    def __init__(self):
        self.urls = []
        self.sockets = []
        self.channels = []

    def attach(self, page):
        page.on("websocket", self._socket)
        return self

    def _socket(self, ws):
        sock = self.opened(ws.url)
        if sock is None:
            return
        ws.on("framesent", lambda message: self.sent(sock, as_bytes(message)))
        ws.on("framereceived", lambda message: self.received(sock, as_bytes(message)))
        ws.on("close", lambda _ws: self.closed(sock))

    def opened(self, url):
        """A socket the page opened: the record of a page socket, else None."""
        self.urls.append(url)
        if not is_page_socket(url):
            return None
        sock = {"n": len(self.sockets), "url": url, "closed": False, "joiner": Joiner(), "by_id": {}}
        self.sockets.append(sock)
        return sock

    def sent(self, sock, data):
        msg = command_of(data)
        if not msg:
            return
        payload = msg.get("payload") or {}
        if msg.get("verb") == "channel.open" and payload.get("kind") == "console":
            rec = {
                "n": len(self.channels),
                "socket": sock["n"],
                "id": payload.get("channel"),
                "payload": payload,
                "session": payload.get("id"),
                "since": payload.get("since"),
                "t0": time.time(),
                "open": None,
                "end": None,
                "closed": False,
                "first_frame_ms": None,
                "first_frame_bytes": None,
                "replay_bytes": None,
                "replay_session": None,
            }
            sock["by_id"][rec["id"]] = rec
            self.channels.append(rec)
        elif msg.get("verb") == "channel.close":
            rec = sock["by_id"].get(payload.get("channel"))
            if rec:
                rec["closed"] = True

    def received(self, sock, data):
        joined = sock["joiner"].push(data)
        if joined is not None:
            rec = sock["by_id"].get(joined[0])
            if rec:
                self._inner(rec, joined[1])
            return
        msg = command_of(data)
        if msg and msg.get("verb") == "channel.closed":
            rec = sock["by_id"].get((msg.get("payload") or {}).get("channel"))
            if rec:
                rec["closed"] = True

    @staticmethod
    def _inner(rec, inner):
        if not inner:
            return
        if inner[0] == TAG_TERMINAL:
            if rec["first_frame_ms"] is None:
                rec["first_frame_ms"] = round((time.time() - rec["t0"]) * 1000, 1)
                rec["first_frame_bytes"] = len(inner) - FRAME_HEADER
            if (rec["open"] or {}).get("replay") is True and rec["replay_bytes"] is None:
                rec["replay_bytes"] = len(inner) - FRAME_HEADER
                rec["replay_session"] = inner[1:FRAME_HEADER]
            return
        msg = command_of(inner)
        if not msg:
            return
        if msg.get("verb") == "session-open" and rec["open"] is None:
            rec["open"] = msg.get("payload") or {}
        elif msg.get("verb") == "session-end":
            rec["end"] = (msg.get("payload") or {}).get("reason")

    def closed(self, sock):
        sock["closed"] = True
        for rec in sock["by_id"].values():
            rec["closed"] = True

    def consoles_of(self, session):
        return [c for c in self.channels if c["session"] == session]

    def live(self):
        return [c for c in self.channels if not c["closed"]]


# An init script that counts the console channels inside the page, for a check
# that reads the counts with `page.evaluate`. It wraps `WebSocket` before any
# page script runs and reads the frames of each page socket `/ws`:
# `__wsCount` counts the console channels opened and ended (a `channel.close`
# of the page, a `channel.closed` of the daemon, or the end of the socket),
# `__wsOpens` keeps each console open payload, `__wsSent` counts the terminal
# and command frames the consoles sent, `__wsSentTermText` keeps the text of
# each terminal frame, and `__pageSockets` keeps every page socket.
CONSOLE_SPY = """
(() => {
  const Native = window.WebSocket;
  window.__wsCount = { created: 0, closed: 0 };
  window.__wsOpens = [];
  window.__wsSent = { term: 0, cmd: 0 };
  window.__wsSentTermText = [];
  window.__pageSockets = [];
  const decoder = new TextDecoder();
  const command = (u) => {
    if (u[0] !== 2) return null;
    try { return JSON.parse(decoder.decode(u.subarray(1))); } catch { return null; }
  };
  const join = (parts) => {
    const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
    let at = 0;
    for (const p of parts) { out.set(p, at); at += p.length; }
    return out;
  };
  function Spy(url, protocols) {
    const sock = protocols === undefined ? new Native(url) : new Native(url, protocols);
    if (new URL(String(url), location.href).pathname !== '/ws') return sock;
    window.__pageSockets.push(sock);
    const consoles = new Set();
    const pending = new Map();
    const end = (id) => { if (consoles.delete(id)) window.__wsCount.closed += 1; };
    const inner = (u) => {
      if (u[0] === 1) {
        window.__wsSent.term += 1;
        window.__wsSentTermText.push(decoder.decode(u.subarray(9)));
      } else if (u[0] === 2) window.__wsSent.cmd += 1;
    };
    const piece = (u) => {
      const id = new DataView(u.buffer, u.byteOffset, u.byteLength).getUint32(1);
      if (!consoles.has(id)) return;
      const parts = pending.get(id) || [];
      parts.push(u.subarray(6));
      if (u[5] & 1) { pending.set(id, parts); return; }
      pending.delete(id);
      inner(join(parts));
    };
    const nsend = sock.send.bind(sock);
    sock.send = (d) => {
      try {
        const u = new Uint8Array(d);
        const m = command(u);
        if (u[0] === 4 && u.length >= 6) piece(u);
        else if (m && m.verb === 'channel.open' && m.payload && m.payload.kind === 'console') {
          consoles.add(m.payload.channel);
          window.__wsCount.created += 1;
          window.__wsOpens.push(m.payload);
        } else if (m && m.verb === 'channel.close' && m.payload) end(m.payload.channel);
      } catch {}
      return nsend(d);
    };
    sock.addEventListener('message', (ev) => {
      if (!(ev.data instanceof ArrayBuffer)) return;
      const m = command(new Uint8Array(ev.data));
      if (m && m.verb === 'channel.closed' && m.payload) end(m.payload.channel);
    });
    sock.addEventListener('close', () => { for (const id of [...consoles]) end(id); });
    return sock;
  }
  Spy.prototype = Native.prototype;
  // The client reads `WebSocket.OPEN`; without these its readyState guards
  // compare against `undefined` and every send is dropped.
  Spy.CONNECTING = Native.CONNECTING;
  Spy.OPEN = Native.OPEN;
  Spy.CLOSING = Native.CLOSING;
  Spy.CLOSED = Native.CLOSED;
  window.WebSocket = Spy;
})();
"""

# Drop the page socket as a dropped link drops it: Chromium's offline
# emulation leaves an open WebSocket up, so the socket is closed in the page
# and its own handler gets the `1006 / wasClean=false` close a dropped link
# gives. Needs `CONSOLE_SPY`.
DROP_PAGE_SOCKET = """() => {
  const s = window.__pageSockets[window.__pageSockets.length - 1];
  const onclose = s.onclose; s.onclose = null; s.close();
  setTimeout(() => onclose({ code: 1006, wasClean: false }), 50); }"""


# --- the check ----------------------------------------------------------------

PORT = 7740
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
TARGET = os.environ.get("RALPHY_WB_TARGET") or os.path.join(REPO_ROOT, "target", "debug")
EXE = os.path.join(TARGET, "ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = os.path.join(TARGET, "session_test_child.exe" if os.name == "nt" else "session_test_child")
CONSOLES = 14
IDS = [f"w-{i:02d}" for i in range(CONSOLES)]
COLS = 5
VIEW = {"width": 2400, "height": 1000}
LATENCY_MS = 600
# `socketsAreStale` in wb-connection-store.ts: three missed heartbeats.
STALE_MS = 6000
HIDDEN_MS = STALE_MS + 6000

HELPERS = """() => {
  window.__W = (id) => [...document.querySelectorAll('#stage .session-window')].find((w) => w._deskId === id) || null;
}"""
BUFFER = """(id) => { const b = __W(id)?._term?.term?.buffer.active; if (!b) return '';
  let out = ''; for (let y = 0; y < b.length; y++) out += (b.getLine(y)?.translateToString(true) ?? '') + '\\n';
  return out; }"""

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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild or the browser loads an old bundle.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True)


def make_fixture_repo():
    d = tempfile.mkdtemp(prefix="wb740_fixture_")
    (Path(d) / "README.md").write_text("# fixture\n", encoding="utf-8")
    for args in (
        ["git", "init"],
        ["git", "config", "user.email", "wb740@example.com"],
        ["git", "config", "user.name", "wb740"],
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
    out = ""
    for i, wid in enumerate(IDS):
        left = 20 + (i % COLS) * 470
        top = 20 + (i // COLS) * 320
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f'consoleName = "{wid}"\n'
            f"ts = {100 + i}\n"
            f"rect = {{ left = {left}, top = {top}, width = 450, height = 300 }}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def launch(daemon_dir, log_path):
    empty = tempfile.mkdtemp(prefix="wb740_empty_")
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


def pump(page, until, timeout_s):
    """Playwright delivers socket events only while the page is being waited on."""
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        if until():
            return True
        page.wait_for_timeout(100)
    return until()


def type_line(page, wid, text):
    page.evaluate("([id, t]) => __W(id)._term.term.paste(t + '\\r')", [wid, text])


def echoed(page, wid, token):
    return any(line.startswith("GOT:") and token in line for line in page.evaluate(BUFFER, wid).split("\n"))


def network(cdp, offline):
    cdp.send(
        "Network.emulateNetworkConditions",
        {"offline": offline, "latency": LATENCY_MS, "downloadThroughput": -1, "uploadThroughput": -1},
    )


def hide(page, ctx):
    """Another tab comes to the front. Headless Chromium may keep the page
    `visible`; then the state is stubbed and the event sent, as the page sees it."""
    other = ctx.new_page()
    other.bring_to_front()
    stubbed = page.evaluate("() => document.visibilityState") != "hidden"
    if stubbed:
        page.evaluate(
            "() => { Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'hidden' });"
            " Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });"
            " document.dispatchEvent(new Event('visibilitychange')); }"
        )
    return other, stubbed


def show(page, other, stubbed):
    other.close()
    page.bring_to_front()
    if stubbed:
        page.evaluate(
            "() => { delete document.visibilityState; delete document.hidden;"
            " document.dispatchEvent(new Event('visibilitychange')); }"
        )


def boot(page):
    """The desk shows its 14 consoles, and each one holds its session."""
    try:
        page.wait_for_selector("[x-data]", timeout=120000)
        page.evaluate("() => Alpine.store('tabs').activate('consoles')")
        page.wait_for_function(
            "(n) => document.querySelectorAll('#stage .session-window').length === n", arg=CONSOLES, timeout=120000
        )
        page.evaluate(HELPERS)
        page.wait_for_function(
            "(ids) => ids.every((id) => __W(id)?._term?.term?.cols > 0 && __W(id)._term.sessionId != null)",
            arg=IDS,
            timeout=120000,
        )
        return True
    except Exception as exc:  # the claims below say what failed
        print(f"the consoles did not start: {exc}".splitlines()[0], flush=True)
        return False


def closed_consoles(page):
    page.evaluate(HELPERS)
    return [w for w in IDS if "[session closed]" in page.evaluate(BUFFER, w)]


def scenario(page, ctx, cdp, seen):
    handshake = page.evaluate("() => WBDaemon.pageHandshakeMs?.()")
    for i, wid in enumerate(IDS):
        type_line(page, wid, f"MARK-740-{i:02d}")
    marked = pump(page, lambda: all(echoed(page, w, f"MARK-740-{i:02d}") for i, w in enumerate(IDS)), 60)
    at_load = list(seen.urls)
    print(f"load: {len(at_load)} socket(s), handshake {handshake} ms, markers echoed={marked}", flush=True)

    other, stubbed = hide(page, ctx)
    network(cdp, True)
    pump(page, lambda: False, HIDDEN_MS / 1000)
    known_sockets = len(seen.sockets)
    known_channels = len(seen.channels)
    # Whether the heartbeat went stale while the page was hidden.
    stale = page.evaluate("() => Alpine.store('connection').socketsAreStale()")
    network(cdp, False)
    show(page, other, stubbed)

    def resumed():
        fresh = [c for c in seen.channels[known_channels:] if c["socket"] >= known_sockets and c["open"]]
        return {c["session"] for c in fresh}

    sessions = {page.evaluate("(id) => __W(id)._term.sessionId", w) for w in IDS}
    back = pump(page, lambda: sessions <= resumed(), 90)
    after = seen.urls[len(at_load) :]
    check(
        "B1 the page opened exactly one WebSocket at load and exactly one at the resume, both `/ws`",
        len(at_load) == 1 and len(after) == 1 and all(is_page_socket(u) for u in at_load + after),
        f"load={at_load} resume={after} stale-when-shown={stale} stubbed-visibility={stubbed}",
    )

    for i, wid in enumerate(IDS):
        type_line(page, wid, f"AGAIN-740-{i:02d}")
    again = pump(page, lambda: all(echoed(page, w, f"AGAIN-740-{i:02d}") for i, w in enumerate(IDS)), 60)
    shown = [w for i, w in enumerate(IDS) if echoed(page, w, f"MARK-740-{i:02d}")]
    check(
        "B2 every console resumed on the new socket, shows its marker again, and echoes a new line",
        marked and back and len(shown) == CONSOLES and again,
        f"resumed={len(resumed() & sessions)}/{CONSOLES} markers={len(shown)}/{CONSOLES} echo-after={again}",
    )


def main():
    from playwright.sync_api import sync_playwright

    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb740_daemon_")
    log_path = os.path.join(daemon_dir, "daemon-stderr.log")
    slug = register_fixture(daemon_dir, make_fixture_repo())
    write_desk(daemon_dir, slug)
    proc = launch(daemon_dir, log_path)
    seen = PageSockets()
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(args=["--disable-webgl", "--disable-gpu"])
            ctx = browser.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            seen.attach(page)
            cdp = ctx.new_cdp_session(page)
            cdp.send("Network.enable")
            network(cdp, False)
            page.goto(BASE, timeout=180000)
            if boot(page):
                scenario(page, ctx, cdp, seen)
            else:
                check(
                    "B1 the page opened exactly one WebSocket at load and exactly one at the resume, both `/ws`",
                    False,
                    f"no resume: {len(seen.urls)} socket(s) at load, {len(seen.urls) - len(seen.sockets)} not `/ws`",
                )
                check("B2 every console resumed on the new socket, shows its marker again, and echoes a new line", False)
            closed = closed_consoles(page)
            check("B3 no console shows `[session closed]`", not closed, f"closed={closed}")
            if errors:
                print(f"page errors: {errors[:3]}", flush=True)
            browser.close()
    finally:
        stop(proc)


if __name__ == "__main__":
    main()
    print(f"\n{sum(results)}/{len(results)} passed")
    sys.exit(0 if len(results) == 3 and all(results) else 1)
