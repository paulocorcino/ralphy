"""Browser acceptance: the Properties dialog for a project on an OLDER peer.

A real local daemon relays `file.stat` and `tree.size` to a peer that does not
know them. The peer is a stub on loopback that answers `/api/peer/hello` with
the protocol of the daemon under test and answers every other command the way a
daemon that predates the two verbs does: `{"status":"error","message":"unknown verb"}`.
The page must say that the host runs an older daemon, and must not show a size
or a fact that nobody read.

Scenario a  a file of the peer: the rows are the name, the path and one line that
            names the older daemon; there is no Size, Type or Modified row
Scenario b  a folder of the peer: the same, and no spinner is left running
Scenario c  the local project still works in the same page (the control: the
            same dialog, the same daemon, a project that is not on the peer)

Boots a Localhost daemon on 7456 over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon
is stopped by its own subprocess handle, NEVER by name.

Run: python tests/browser/files/wb_properties_peer.py   (exit 0 = all pass)
"""

import http.server
import json
import os
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "console"))
from wb_console_peer_unanswered import PROTOCOL  # noqa: E402  the peer protocol of the daemon under test

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7456
BASE = f"http://127.0.0.1:{PORT}/"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SH = "Alpine.$data(document.querySelector('[x-data]'))"
PROPS = "Alpine.$data(document.querySelector('[x-data=\"wbPropertiesDialog\"]'))"

PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
PEER_NAME = "old-peer"
PEER_TOKEN = "peer-fixture-token"
SLUG = "owner/old-peer-repo"
OLDER = "This host runs an older daemon. Update it to see properties."

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


class OldPeer(http.server.BaseHTTPRequestHandler):
    """A peer from before `file.stat` and `tree.size`."""

    protocol_version = "HTTP/1.0"
    commands = []

    def _json(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _authorized(self):
        if self.headers.get("Authorization") != f"Bearer {PEER_TOKEN}":
            self._json(401, {"error": "unauthorized"})
            return False
        return True

    def do_GET(self):
        if not self._authorized():
            return
        if self.path == "/api/peer/hello":
            self._json(200, {"daemon_id": PEER_ID, "name": PEER_NAME, "avatar": "🐺",
                             "environment": "WSL: Ubuntu-22.04", "protocol_version": PROTOCOL})
        elif self.path == "/api/repos":
            self._json(200, [{"slug": SLUG, "path": "/home/me/old", "reachable": True,
                              "branch": "main", "dirty": False, "remote": None}])
        else:
            self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self._authorized():
            return
        size = int(self.headers.get("Content-Length", "0"))
        command = json.loads(self.rfile.read(size) or b"{}")
        OldPeer.commands.append(command.get("verb"))
        self._json(200, {"status": "error", "message": "unknown verb"})

    def log_message(self, *_args):
        pass


class Quiet(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def free_port():
    import socket

    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def wait_listening(base, timeout=25):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            urllib.request.urlopen(base, timeout=1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def seed_local():
    d = Path(tempfile.mkdtemp(prefix="wbpropspeer_local_")) / "local-repo"
    d.mkdir()
    (d / "a.txt").write_text("hello", encoding="utf-8")
    subprocess.run(["git", "init", "-b", "main"], cwd=d, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "p@example.com"], cwd=d, check=True)
    subprocess.run(["git", "config", "user.name", "p"], cwd=d, check=True)
    subprocess.run(["git", "add", "-A"], cwd=d, check=True)
    subprocess.run(["git", "commit", "-m", "fixture"], cwd=d, check=True, capture_output=True)
    return str(d)


def seed_daemon_dir(peer_port):
    d = Path(tempfile.mkdtemp(prefix="wbpropspeer_reg_"))
    (d / "daemon.toml").write_text('id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"\nname = "anvil"\navatar = "🐙"\n', encoding="utf-8")
    peers = d / "peers"
    peers.mkdir()
    (peers / f"{PEER_ID}.toml").write_text(
        "\n".join([
            f'daemon_id = "{PEER_ID}"', f'name = "{PEER_NAME}"', 'avatar = "🐺"',
            'address = "127.0.0.1"', f"port = {peer_port}", 'environment = "WSL: Ubuntu-22.04"',
            f'token = "{PEER_TOKEN}"', f"protocol_version = {PROTOCOL}", "",
        ]),
        encoding="utf-8",
    )
    return str(d)


def rows(page):
    return page.evaluate(f"() => {PROPS}.propsRows()")


def main():
    build()
    stub = Quiet(("127.0.0.1", free_port()), OldPeer)
    threading.Thread(target=stub.serve_forever, daemon=True).start()
    daemon_dir = seed_daemon_dir(stub.server_address[1])
    empty = tempfile.mkdtemp(prefix="wbpropspeer_empty_")
    env = dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir, RALPHY_USAGE_DIR=empty,
               RALPHY_CLAUDE_PROJECTS_DIR=empty, RALPHY_CODEX_DIR=empty,
               RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
               RALPHY_KIMI_DIR=empty, RALPHY_KIMI_CODE_DIR=empty)
    subprocess.run([EXE, "daemon", "add", seed_local()], env=env, check=True, capture_output=True)
    proc = subprocess.Popen([EXE, "daemon", "--port", str(PORT)], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        if not wait_listening(BASE):
            check(f"daemon listening on {PORT}", False)
            sys.exit(1)
        check(f"daemon listening on {PORT}", True)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_context(viewport={"width": 1440, "height": 900}).new_page()
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE)
            page.wait_for_selector("[x-data]", timeout=8000)
            page.wait_for_function("() => Alpine.store('projects').projects.length >= 1", timeout=15000)

            # The page asks the project that is open; open the peer's by its ref.
            peer_ref = f"{PEER_ID}/{SLUG}"
            page.evaluate("(r) => Alpine.store('projects').setOpen(r)", arg=peer_ref)

            for path, is_folder, what in (("src/a.txt", False, "a file"), ("src", True, "a folder")):
                page.evaluate(f"(d) => {PROPS}.openProperties(d)", arg={"path": path, "isFolder": is_folder})
                page.wait_for_function(f"() => !{PROPS}.propsBusy()", timeout=15000)
                got = rows(page)
                labels = [r["label"] for r in got]
                facts = next((r["value"] for r in got if r["label"] == "Facts"), None)
                check(f"{what} of the peer names the older daemon", facts == OLDER, str(got))
                check(f"{what} of the peer shows no size and no type", not {"Size", "Type", "Modified"} & set(labels), str(labels))
                check(f"{what} of the peer leaves no spinner", not any(r.get("pending") for r in got), str(got))
                page.evaluate(f"() => {PROPS}.propsClose()")

            asked = sorted(set(OldPeer.commands))
            check("the local daemon relayed both verbs to the peer", "file.stat" in asked and "tree.size" in asked, str(asked))

            # The control: the local project of the same page is not on the peer.
            local = page.evaluate("() => Alpine.store('projects').projects.find(p => !p.daemon || p.daemon !== '" + PEER_ID + "')?.slug")
            page.evaluate("(r) => Alpine.store('projects').setOpen(r)", arg=local)
            page.evaluate(f"(d) => {PROPS}.openProperties(d)", arg={"path": "a.txt", "isFolder": False})
            page.wait_for_function(f"() => !{PROPS}.propsBusy()", timeout=15000)
            got = rows(page)
            size = next((r["value"] for r in got if r["label"] == "Size"), None)
            check("c: the local project still shows its size", size == "5 bytes", str(got))
            check("no page error was thrown", not thrown, str(thrown))
            browser.close()
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except Exception:
            proc.kill()
        stub.shutdown()

    failed = results.count(False)
    print(f"\n{len(results) - failed}/{len(results)} checks passed", flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
