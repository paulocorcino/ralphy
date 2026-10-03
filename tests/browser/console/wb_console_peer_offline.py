"""A console of a project on a peer that does not answer is a placeholder that
says why, and it comes back when the peer does.

The "peer" is a stub HTTP listener on loopback that answers `/api/peer/hello`
and `/api/repos` (the two routes the fleet probe reads) and nothing else. Its
descriptor has no `[nudge]`, so the peer is "on another machine": a console on
it goes through the relay, like a host reached through ssh. The stub is first
CLOSED, so the daemon's own fleet state for it is `unreachable`.

Scenario 1  a plain console on the peer project is a placeholder, not a dead
            terminal: the peer's state in words, "Try again", and the daemon's
            diagnosis behind a closed "Details"
Scenario 2  an agent console on the same project is the same kind of box
Scenario 3  after a reload both come back as placeholders, and no terminal
            prints "[could not start"
Scenario 4  the peer starts answering: the agent console offers "Relaunch"
            and starts nothing; the plain console launches again by itself
Scenario 5  that launch is refused (the stub has no console), and the box does
            not launch again on the next fleet read: no loop
Scenario 6  zero `pageerror` events over the whole pass

Boots a Localhost daemon on 7463 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as
the orchestrator on this host).

Writes .ralphy/screenshots/console-peer-offline.png and console-peer-back.png.
Run: python tests/browser/console/wb_console_peer_offline.py   (exit 0 = all pass)
"""

import http.server
import json
import os
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7463
BASE = f"http://127.0.0.1:{PORT}/"

# tests/browser/console/<this file> -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
PEER_NAME = "vps-box"
PEER_ENV = "Ubuntu 24.04"
PEER_TOKEN = "peer-fixture-token"
SLUG = "ralphy-lab/remote-repo"
REF = f"{PEER_ID}/{SLUG}"

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


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class PeerStub(http.server.BaseHTTPRequestHandler):
    # HTTP/1.0 closes every connection, so a closed stub does not keep
    # answering on a connection the daemon's client pooled (see wb_fleet_349).
    protocol_version = "HTTP/1.0"

    def _json(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.headers.get("Authorization") != f"Bearer {PEER_TOKEN}":
            self._json(401, {"error": "unauthorized"})
        elif self.path == "/api/peer/hello":
            self._json(
                200,
                {
                    "daemon_id": PEER_ID,
                    "name": PEER_NAME,
                    "avatar": "🐺",
                    "environment": PEER_ENV,
                    "protocol_version": 3,
                },
            )
        elif self.path == "/api/repos":
            self._json(
                200,
                [{"slug": SLUG, "path": "/home/op/remote-repo", "reachable": True, "branch": "main", "dirty": False, "remote": None}],
            )
        else:
            self._json(404, {"error": "not found"})

    def log_message(self, *_args):
        pass


class QuietServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def start_peer_stub(port):
    server = QuietServer(("127.0.0.1", port), PeerStub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def seed_store(daemon_dir, peer_port):
    """A baptized daemon (the fleet keys its rows on `daemon_id`) and one peer
    descriptor with NO `[nudge]`: a peer on another machine."""
    d = Path(daemon_dir)
    d.mkdir(parents=True, exist_ok=True)
    (d / "daemon.toml").write_text('id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"\nname = "anvil"\navatar = "🐙"\n', encoding="utf-8")
    peers = d / "peers"
    peers.mkdir(exist_ok=True)
    (peers / f"{PEER_ID}.toml").write_text(
        "\n".join(
            [
                f'daemon_id = "{PEER_ID}"',
                f'name = "{PEER_NAME}"',
                'avatar = "🐺"',
                'address = "127.0.0.1"',
                f"port = {peer_port}",
                f'environment = "{PEER_ENV}"',
                f'token = "{PEER_TOKEN}"',
                "protocol_version = 3",
                "",
            ]
        ),
        encoding="utf-8",
    )


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild after any assets/ui edit.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb_peer_offline_empty_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
    )
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )


# Every console window as the operator sees it: its kind, whether it is a
# placeholder, the box sentence, the button, the closed detail, and the text of
# a terminal if it has one.
WINDOWS_EXPR = """
() => Array.from(document.querySelectorAll('.session-window')).map(w => {
  const note = w.querySelector('.session-offline');
  const btn = note && note.querySelector('.session-reconnect');
  const det = note && note.querySelector('details.session-detail');
  const b = w._term && w._term.term && w._term.term.buffer.active;
  let screen = '';
  if (b) for (let y = 0; y < b.length; y++) { const l = b.getLine(y); if (l) screen += l.translateToString(true); }
  return {
    kind: w._deskKind,
    placeholder: w.classList.contains('placeholder'),
    text: note ? (note.querySelector(':scope > p') || {}).textContent || '' : '',
    button: btn && !btn.hidden && btn.offsetParent !== null ? btn.textContent : null,
    detail: det && !det.hidden ? det.querySelector('p').textContent : null,
    detailOpen: det ? det.open : null,
    screen,
  };
})
"""


def windows(page):
    return page.evaluate(WINDOWS_EXPR)


def by_kind(wins, kind):
    return next((w for w in wins if w["kind"] == kind), None)


def wait_for(page, pred, timeout=15):
    deadline = time.time() + timeout
    got = windows(page)
    while time.time() < deadline:
        got = windows(page)
        if pred(got):
            return got
        time.sleep(0.25)
    return got


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb_peer_offline_")
    peer_port = free_port()
    seed_store(daemon_dir, peer_port)

    proc = launch(daemon_dir)
    stub = None
    try:
        if not wait_listening(BASE) or proc.poll() is not None:
            print(f"[FAIL] daemon did not start (or {PORT} answers a FOREIGN listener)", flush=True)
            sys.exit(1)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            sockets = []
            page.on("websocket", lambda ws: sockets.append(ws.url) if "/ws/session" in ws.url else None)
            page.goto(BASE, wait_until="domcontentloaded")
            page.wait_for_function("() => !!window.WBConsole", timeout=8000)
            page.wait_for_function(
                f"() => {SH}.fleetPeers.some(p => p.daemon_id === '{PEER_ID}' && p.state === 'unreachable')",
                timeout=20000,
            )

            # --- scenario 1: a plain console on the closed peer --------------
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{REF}', plain: true }})")
            wins = wait_for(page, lambda ws: (by_kind(ws, "console") or {}).get("placeholder"))
            plain = by_kind(wins, "console") or {}
            check("a refused plain console becomes a placeholder", plain.get("placeholder"), f"win={plain}")
            check(
                "the box names the peer and its state",
                plain.get("text") == f"Ralphy on {PEER_ENV} is not running.",
                f"text={plain.get('text')!r}",
            )
            check("the one action is Try again", plain.get("button") == "Try again", f"button={plain.get('button')!r}")
            check(
                "the daemon's diagnosis is behind a closed Details",
                bool(plain.get("detail")) and plain.get("detailOpen") is False,
                f"detail={plain.get('detail')!r}",
            )

            # --- scenario 2: an agent console on the same peer ----------------
            page.evaluate(f"() => window.WBConsole.open({{ repo: '{REF}', agent: 'claude' }})")
            wins = wait_for(page, lambda ws: (by_kind(ws, "agent") or {}).get("placeholder"))
            agent = by_kind(wins, "agent") or {}
            check(
                "a refused agent console is the same kind of box",
                agent.get("placeholder") and agent.get("text") == plain.get("text") and agent.get("button") == "Try again",
                f"win={agent}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "console-peer-offline.png"))

            # --- scenario 3: a reload restores both as placeholders -----------
            page.reload(wait_until="domcontentloaded")
            page.wait_for_function("() => !!window.WBConsole", timeout=8000)
            wins = wait_for(
                page,
                lambda ws: len(ws) == 2
                and all(w["placeholder"] and w["text"] == f"Ralphy on {PEER_ENV} is not running." for w in ws),
                timeout=25,
            )
            check(
                "after a reload both consoles are peer placeholders",
                len(wins) == 2 and all(w["placeholder"] for w in wins),
                f"wins={[(w['kind'], w['text']) for w in wins]}",
            )
            check(
                "no terminal prints a raw refusal",
                not any("[could not start" in w["screen"] for w in wins),
                f"screens={[w['screen'][-120:] for w in wins]}",
            )

            # --- scenario 4: the peer answers again ---------------------------
            stub = start_peer_stub(peer_port)
            before = len(sockets)
            page.evaluate(f"() => {SH}.loadRepos({{ git: false }})")
            wins = wait_for(
                page,
                lambda ws: (by_kind(ws, "agent") or {}).get("button") == "Relaunch"
                and (by_kind(ws, "console") or {}).get("text") == f"{PEER_ENV} did not start this console.",
                timeout=25,
            )
            agent = by_kind(wins, "agent") or {}
            plain = by_kind(wins, "console") or {}
            check(
                "the agent console offers Relaunch and starts nothing",
                agent.get("text") == f"{PEER_ENV} is available again." and agent.get("button") == "Relaunch",
                f"win={agent}",
            )
            launched = sockets[before:]
            check(
                "the plain console launched again by itself, once",
                len(launched) == 1 and "console=1" in launched[0],
                f"sockets={launched}",
            )
            page.screenshot(path=os.path.join(SHOT_DIR, "console-peer-back.png"))

            # --- scenario 5: the refused relaunch does not loop ---------------
            check(
                "the refused relaunch is a placeholder again",
                plain.get("placeholder") and plain.get("button") == "Try again",
                f"win={plain}",
            )
            after = len(sockets)
            page.evaluate(f"() => {SH}.loadRepos({{ git: false }})")
            page.wait_for_timeout(3000)
            check("a second fleet read launches nothing", len(sockets) == after, f"sockets={sockets[after:]}")

            browser.close()
            # --- scenario 6 ----------------------------------------------------
            check("zero pageerror events captured", not thrown, f"got={thrown}")
    finally:
        stop(proc)
        if stub:
            stub.shutdown()
            stub.server_close()

    # The count floor: an early exit must not report success on a few checks.
    ok = all(results) and len(results) == 12
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
