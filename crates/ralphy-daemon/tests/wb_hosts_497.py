"""#497 browser acceptance: add, edit and remove a host from the workbench.

One Playwright pass over a REAL daemon on a SCRATCH `RALPHY_DAEMON_DIR`. The
daemon runs the real `ralphy host …` verbs with the real `ssh`, but only
against closed loopback ports, so nothing signs in anywhere. A stub HTTP
listener plays the peer's daemon behind the seeded tunnel descriptor, as in
`wb_tunnel_495.py`.

Scenario 1  with a fleet of one (no group headers), the Projects header has the
            Hosts button, and it opens the form with the SSH config hosts;
            the password field is a text field, so no browser offers to save it
Scenario 2  a typed address on a closed port: Next shows the help panel with
            its three tabs, the failure, and the wrong-address note
Scenario 3  a seeded tunnel peer: every group header shows its system's icon
            (the penguin for the Linux host, the local system's for the
            local group), and no group has a menu
Scenario 4  Hosts now opens on Your hosts, with the host's row
Scenario 5  Edit fills the form with the host's connection
Scenario 6  Remove asks in the row, with the token option unchecked
Scenario 7  Remove runs `ralphy host remove`: the group goes away with no page
            reload, the descriptor file is deleted, and the dialog shows the
            form, because no host is left
Scenario 8  no page errors were thrown

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
is also the orchestrator on this host).

Needs `cargo build -p ralphy-cli --bin ralphy` first (the UI is embedded).
Writes .ralphy/screenshots/2026-09-29-issue-497-*.png.
Run: python crates/ralphy-daemon/tests/wb_hosts_497.py   (exit 0 = all pass)
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

PORT = 7497
BASE = f"http://127.0.0.1:{PORT}/"

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXT = ".exe" if os.name == "nt" else ""
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy" + EXT)
SHOT_DIR = os.path.join(REPO_ROOT, ".ralphy", "screenshots")

# A real ULID: Crockford base32 has no I, L, O or U.
PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FC7"
PEER_NAME = "svrapp"
PEER_ENV = "Linux"
PEER_TOKEN = "hosts-fixture-token"
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


def daemon_env(daemon_dir):
    empty = tempfile.mkdtemp(prefix="wb497_empty_")
    return dict(
        os.environ,
        RALPHY_DAEMON_DIR=daemon_dir,
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
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
    d = Path(tempfile.mkdtemp(prefix="wb497_local_")) / "local-repo"
    d.mkdir()
    (d / "README.md").write_bytes(b"# local-repo\n")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb497@example.com")
    git(d, "config", "user.name", "wb497")
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


def seed_descriptor(daemon_dir, stub_port, ssh_port):
    """A tunnel peer whose `ssh` destination is a closed loopback port."""
    peers = Path(daemon_dir) / "peers"
    peers.mkdir(parents=True, exist_ok=True)
    text = "\n".join(
        [
            f'daemon_id = "{PEER_ID}"',
            f'name = "{PEER_NAME}"',
            'avatar = "🐙"',
            'address = "127.0.0.1"',
            f"port = {stub_port}",
            f'environment = "{PEER_ENV}"',
            f'token = "{PEER_TOKEN}"',
            "protocol_version = 3",
            "",
            "[tunnel]",
            f'destination = "ssh://127.0.0.1:{ssh_port}"',
            "peer_port = 7257",
            f"local_port = {stub_port}",
            "",
        ]
    )
    path = peers / f"{PEER_ID}.toml"
    path.write_bytes(text.encode("utf-8"))
    return path


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=daemon_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def shot(page, slug):
    os.makedirs(SHOT_DIR, exist_ok=True)
    path = os.path.join(SHOT_DIR, f"2026-09-29-issue-497-{slug}.png")
    page.screenshot(path=path)
    print(f"[INFO] screenshot {path}", flush=True)


VISIBLE = "el => !!el && el.getClientRects().length > 0"


def main():
    if not os.path.isfile(EXE):
        print(f"[FAIL] missing {EXE}: build it first", flush=True)
        sys.exit(1)
    daemon_dir = tempfile.mkdtemp(prefix="wb497_daemon_")
    setup(daemon_dir)
    register_local_repo(daemon_dir)
    stub_port = free_port()
    closed_port = free_port()
    stub = start_peer_stub(stub_port)

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
            page.wait_for_function(
                "() => document.querySelectorAll('.projects .project').length > 0", timeout=20000
            )

            # 1. Fleet of one: no headers, the button, the dialog.
            headers = page.evaluate(
                f"() => Array.from(document.querySelectorAll('.projects .env-group')).filter({VISIBLE}).length"
            )
            page.click(".side-add-host")
            page.wait_for_function(
                "() => { const d = document.querySelector('[aria-label=\"Hosts\"][role=dialog]');"
                " return !!d && d.getClientRects().length > 0; }",
                timeout=10000,
            )
            first_option = page.evaluate("() => document.querySelector('.host-alias-pick option').textContent")
            password_type = page.evaluate("() => document.querySelector('#host-password').type")
            check(
                "fleet of one: the header button opens the dialog",
                headers == 0 and first_option == "Hosts in your SSH config",
                f"headers={headers} option={first_option!r}",
            )
            check(
                "the password field is not a password input, so the browser offers no save",
                password_type == "text",
                f"type={password_type!r}",
            )
            shot(page, "add-host-connection")

            # 2. A closed port: the help panel.
            page.fill("#host-address", "127.0.0.1")
            page.fill("#host-port", str(closed_port))
            page.click(".host-foot .btn.accent")
            page.wait_for_function(
                f"() => ({VISIBLE})(document.querySelector('.host-help'))", timeout=30000
            )
            tabs = page.evaluate(
                "() => Array.from(document.querySelectorAll('.host-help .seg-btn')).map(b => b.textContent.trim())"
            )
            failure = page.evaluate("() => document.querySelector('.host-modal .side-error').textContent")
            page.click(".host-help button.seg-btn:nth-of-type(2)")
            mac = page.evaluate("() => document.querySelector('.host-help').textContent")
            check(
                "a closed port shows the help panel with three tabs",
                tabs == ["Windows", "macOS", "Linux"]
                and str(closed_port) in failure
                and "Remote Login" in mac
                and "ipconfig getifaddr en0" in mac
                and "launchctl" not in mac,
                f"tabs={tabs} failure={failure!r}",
            )
            shot(page, "add-host-help")
            page.keyboard.press("Escape")

            # 3. A tunnel peer: a system icon on every group, and no menu.
            descriptor = seed_descriptor(daemon_dir, stub_port, closed_port)
            page.click(".side-refresh")
            page.wait_for_function(
                "(label) => Array.from(document.querySelectorAll('.projects .env-group .env-label'))"
                ".some(l => l.textContent.trim() === label)",
                arg=LABEL,
                timeout=20000,
            )
            icons = page.evaluate(
                f"""() => Array.from(document.querySelectorAll('.projects .env-group')).filter({VISIBLE}).map(g => {{
                    const shown = Array.from(g.querySelectorAll('.os-icon > *')).filter({VISIBLE});
                    return [g.querySelector('.env-label').textContent.trim(),
                            shown.map(e => e.matches('svg') ? e.querySelector('use').getAttribute('href') : e.className)];
                }})"""
            )
            local_icon = {"nt": "bi bi-windows"}.get(os.name, "bi bi-apple" if sys.platform == "darwin" else "#os-linux")
            menus = page.evaluate("() => document.querySelectorAll('.group-menu').length")
            check(
                "every group shows its system's icon, and no group has a menu",
                len(icons) == 2
                and icons[0][1] == [local_icon]
                and icons[1] == [LABEL, ["#os-linux"]]
                and menus == 0,
                f"icons={icons} menus={menus}",
            )
            shot(page, "group-icons")

            # 4. Hosts opens on the list.
            page.click(".side-add-host")
            page.wait_for_function(
                f"() => ({VISIBLE})(document.querySelector('.host-list'))", timeout=10000
            )
            tab = page.evaluate("() => document.querySelector('.host-tabs .seg-btn.on').textContent.trim()")
            row = page.evaluate(
                "() => [document.querySelector('.host-row-name').textContent.trim(),"
                " document.querySelector('.host-row-dest').textContent.trim()]"
            )
            check(
                "Hosts opens on Your hosts, with the host's row",
                tab == "Your hosts (1)" and row == [PEER_NAME, f"ssh://127.0.0.1:{closed_port}"],
                f"tab={tab!r} row={row}",
            )
            shot(page, "hosts-list")

            # 5. Edit: the form has the host's connection.
            page.click(".host-row .btn:text-is('Edit')")
            page.wait_for_function(f"() => ({VISIBLE})(document.querySelector('#host-address'))", timeout=10000)
            form = page.evaluate(
                "() => [document.querySelector('.host-tabs .seg-btn.on').textContent.trim(),"
                " document.querySelector('#host-address').value, document.querySelector('#host-port').value,"
                " document.querySelector('#host-user').value, document.querySelector('#host-password').value]"
            )
            check(
                "Edit fills the form with the host's connection",
                form == [f"Edit {PEER_NAME}", "127.0.0.1", str(closed_port), "", ""],
                f"form={form}",
            )
            shot(page, "hosts-edit")
            page.click(".host-tabs .seg-btn:text-is('Your hosts (1)')")

            # 6. Remove asks in the row.
            page.click(".host-row .btn:text-is('Remove')")
            page.wait_for_function(f"() => ({VISIBLE})(document.querySelector('.host-remove'))", timeout=10000)
            unchecked = page.evaluate("() => !document.querySelector('.host-remove input[type=checkbox]').checked")
            check("Remove asks in the row, the token option unchecked", unchecked)
            shot(page, "hosts-remove")

            # 7. Remove: `ralphy host remove` forgets the silent host.
            page.click(".host-remove .btn.danger")
            page.wait_for_function(
                "(label) => !Array.from(document.querySelectorAll('.projects .env-group .env-label'))"
                ".some(l => l.textContent.trim() === label)",
                arg=LABEL,
                timeout=45000,
            )
            page.wait_for_function(f"() => ({VISIBLE})(document.querySelector('#host-address'))", timeout=10000)
            tabs_shown = page.evaluate(f"() => ({VISIBLE})(document.querySelector('.host-tabs'))")
            check(
                "Remove drops the group and the descriptor; with no host left the dialog shows the form",
                not descriptor.exists() and not tabs_shown,
                f"descriptor_exists={descriptor.exists()} tabs_shown={tabs_shown}",
            )

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
    if len(results) != 9:
        print(f"[FAIL] expected 9 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
