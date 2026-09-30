"""#495 browser acceptance: a peer on another machine, reached through a tunnel.

One Playwright pass over a REAL daemon. The "peer" is a stub HTTP listener on
loopback that answers `/api/peer/hello` and `/api/repos`. The descriptor names
a `[tunnel]` whose local port is the stub's port, and the daemon runs
`command_test_child` in place of `ssh` (`RALPHY_DAEMON_SSH_OVERRIDE`): the child
exits at once, so the tunnel is "closed" whenever the daemon has to ask. No real
SSH connection is made.

Scenario 1  the tunnel peer's group header reads `svrapp: Linux`
Scenario 2  that header carries the Linux penguin, and no separate daemon name
Scenario 3  with the stub CLOSED and the page reloaded, the header's state is
            `tunnel-closed`, not painted as a fault, and its tooltip says
            `reconnecting` and never `WSL`
Scenario 4  no page errors were thrown

Boots a daemon on 7495 over a SCRATCH `RALPHY_DAEMON_DIR`. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` is also the
orchestrator on this host).

Needs `cargo build -p ralphy-cli --bin ralphy` (the UI is embedded) and
`cargo build -p ralphy-daemon --bins` (the `ssh` stand-in) first.
Writes docs/screenshots/495-tunnel-header-2026-09-29.png (gitignored — commit
it with `git add -f`).
Run: python crates/ralphy-daemon/tests/wb_tunnel_495.py   (exit 0 = all pass)
"""

import http.server
import json
import os
import shutil
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

PORT = 7495
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXT = ".exe" if os.name == "nt" else ""
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy" + EXT)
SSH_STAND_IN = os.path.join(REPO_ROOT, "target", "debug", "command_test_child" + EXT)
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")

# A real ULID: Crockford base32 has no I, L, O or U.
PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC5"
PEER_NAME = "svrapp"
PEER_ENV = "Linux"
PEER_TOKEN = "tunnel-fixture-token"
PEER_SLUG = "ralphy-lab/remote-repo"
LABEL = f"{PEER_NAME}: {PEER_ENV}"

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
    protocol_version = "HTTP/1.1"

    def _json(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        # The daemon pools peer connections, and a kept-alive one is still
        # served by its handler thread after `server_close()`: the "closed"
        # stub would keep answering.
        self.send_header("Connection", "close")
        self.close_connection = True
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.headers.get("Authorization") != f"Bearer {PEER_TOKEN}":
            self._json(401, {"error": "unauthorized"})
            return
        if self.path == "/api/peer/hello":
            self._json(
                200,
                {
                    "daemon_id": PEER_ID,
                    "name": PEER_NAME,
                    "avatar": "🐙",
                    "environment": PEER_ENV,
                    "protocol_version": 3,
                },
            )
        elif self.path == "/api/repos":
            self._json(
                200,
                [
                    {
                        "slug": PEER_SLUG,
                        "path": "/home/operator/dev/remote-repo",
                        "reachable": True,
                        "branch": "main",
                        "dirty": False,
                        "remote": None,
                    }
                ],
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


def stand_in_copy(folder):
    # The daemon never kills its ssh, so a stand-in can outlive this script.
    # A running target/debug image would make the next cargo build that
    # relinks it fail on Windows (os error 5).
    copy = os.path.join(folder, os.path.basename(SSH_STAND_IN))
    shutil.copy2(SSH_STAND_IN, copy)
    return copy


def daemon_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb495_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        RALPHY_DAEMON_SSH_OVERRIDE=stand_in_copy(empty),
        RALPHY_TEST_EXIT_CODE="0",
    )


def setup(daemon_dir):
    subprocess.run(
        [EXE, "daemon", "setup", "--name", "local", "--avatar", "1"],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def register_local_repo(daemon_dir):
    """One local repo: a fleet of one has no group headers at all."""
    d = Path(tempfile.mkdtemp(prefix="wb495_local_")) / "local-repo"
    d.mkdir()
    (d / "README.md").write_bytes(b"# local-repo\n")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb495@example.com")
    git(d, "config", "user.name", "wb495")
    git(d, "remote", "add", "origin", "https://github.com/ralphy-lab/local-repo.git")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    subprocess.run(
        [EXE, "daemon", "add", str(d)],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )


def seed_descriptor(daemon_dir, port):
    peers = Path(daemon_dir) / "peers"
    peers.mkdir(parents=True, exist_ok=True)
    text = "\n".join(
        [
            f'daemon_id = "{PEER_ID}"',
            f'name = "{PEER_NAME}"',
            'avatar = "🐙"',
            'address = "127.0.0.1"',
            f"port = {port}",
            f'environment = "{PEER_ENV}"',
            f'token = "{PEER_TOKEN}"',
            "protocol_version = 3",
            "",
            "[tunnel]",
            f'destination = "{PEER_NAME}"',
            "peer_port = 7257",
            f"local_port = {port}",
            "",
        ]
    )
    (peers / f"{PEER_ID}.toml").write_bytes(text.encode("utf-8"))


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=daemon_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The header of the tunnel peer's group, read from the live DOM.
HEADER_EXPR = """
(label) => {
  const h = Array.from(document.querySelectorAll('.projects .env-group'))
    .find(g => (g.querySelector('.env-label') || {}).textContent?.trim() === label);
  if (!h) return null;
  const shown = el => !!el && el.getClientRects().length > 0;
  const state = Array.from(h.querySelectorAll('.peer-state')).find(shown);
  return {
    label: h.querySelector('.env-label').textContent.trim(),
    title: h.getAttribute('title') || '',
    penguin: shown(h.querySelector('.os-icon use[href="#os-linux"]')?.closest('svg')),
    daemonName: shown(h.querySelector('.env-daemon')),
    stateClass: state ? state.className : '',
    laid: shown(h),
  };
}
"""


def header(page, want_state=None):
    page.wait_for_function(
        """([label, want]) => {
          const h = Array.from(document.querySelectorAll('.projects .env-group'))
            .find(g => (g.querySelector('.env-label') || {}).textContent?.trim() === label);
          if (!h) return false;
          if (!want) return true;
          return Array.from(h.querySelectorAll('.peer-state')).some(s => s.classList.contains(want));
        }""",
        arg=[LABEL, want_state],
        timeout=20000,
    )
    return page.evaluate(HEADER_EXPR, LABEL)


def main():
    for exe in (EXE, SSH_STAND_IN):
        if not os.path.isfile(exe):
            print(f"[FAIL] missing {exe}: build it first", flush=True)
            sys.exit(1)
    daemon_dir = tempfile.mkdtemp(prefix="wb495_daemon_")
    setup(daemon_dir)
    register_local_repo(daemon_dir)
    peer_port = free_port()
    stub = start_peer_stub(peer_port)
    seed_descriptor(daemon_dir, peer_port)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE) or proc.poll() is not None:
            print(f"[FAIL] daemon did not start (or {PORT} answers a FOREIGN listener)", flush=True)
            sys.exit(1)

        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE, wait_until="domcontentloaded")

            h = header(page, "reachable")
            check(
                "the tunnel peer's header reads `svrapp: Linux`",
                h is not None and h["label"] == LABEL and h["laid"],
                "header={}".format(h),
            )
            check(
                "the header carries the Linux penguin and no separate daemon name",
                h is not None and h["penguin"] and not h["daemonName"],
                "header={}".format(h),
            )

            stub.shutdown()
            stub.server_close()
            page.reload(wait_until="domcontentloaded")
            h = header(page, "tunnel-closed")
            classes = (h or {}).get("stateClass", "").split()
            check(
                "a closed tunnel is `tunnel-closed`, not a fault",
                "tunnel-closed" in classes and "fault" not in classes,
                "header={}".format(h),
            )
            check(
                "its tooltip says reconnecting and never WSL",
                h is not None and "reconnecting" in h["title"] and "WSL" not in h["title"],
                "title={}".format((h or {}).get("title")),
            )
            os.makedirs(SHOT_DIR, exist_ok=True)
            shot = os.path.join(SHOT_DIR, "495-tunnel-header-2026-09-29.png")
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            check("no page errors were thrown", not thrown, "got={}".format(thrown))
            browser.close()
    finally:
        stop(proc)
        try:
            stub.shutdown()
            stub.server_close()
        except Exception:
            pass

    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    # Floor: a deleted scenario must not pass silently as "everything green".
    if len(results) != 5:
        print(f"[FAIL] expected 5 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
