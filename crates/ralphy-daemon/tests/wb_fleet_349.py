"""#349 browser acceptance: the federated sidebar (local fleet, ADR-0052 §5).

One Playwright pass over a REAL daemon proving the slice's browser-facing claim:
a peer announced into this daemon's store shows up in the sidebar as its OWN
environment group, carrying its own repos, and it is MARKED — not removed — when
it stops answering.

The "peer" is a stub HTTP listener this script opens on loopback. It answers the
two routes a peer must answer (`/api/peer/hello` with `protocol_version: 1` and
`/api/repos` with one row) and nothing else — no WSL, no second Ralphy build, no
`wsl.exe`. The environment label `WSL: Ubuntu-22.04` is DATA in the descriptor,
which is the point: the local daemon renders the environment it was told about.

Scenario 1  the peer's environment group header renders with the literal text
            `WSL: Ubuntu-22.04` and the peer's name
Scenario 2  the peer's repo row is present UNDER that header, and the local
            repo's row is still present under its own group — federation is
            additive, never a replacement
Scenario 3  the same `owner/repo` slug registered on BOTH daemons yields TWO
            rows with two distinct `key`s and two distinct paths — the collision
            case the aggregate is keyed for
Scenario 4  a peer row is inert in this slice: clicking it does not open a
            project (no repo operation is federated yet)
Scenario 5  with the stub CLOSED and the page reloaded, the group is still
            there and its state string reads `unreachable` — marked, not removed
Scenario 6  the local rows survive the peer going down: federation must never
            blank the sidebar the operator is working in

Boots a Localhost daemon on 7441 over a SCRATCH `RALPHY_DAEMON_DIR`, so the
operator's own daemon registry and login policy are untouched. The daemon is
stopped by its own subprocess handle, NEVER by name (`ralphy.exe` doubles as the
orchestrator on this host).

Writes docs/screenshots/349-fleet-grouping-2026-07-28.png (gitignored — commit
it with `git add -f`).
Run: python crates/ralphy-daemon/tests/wb_fleet_349.py   (exit 0 = all pass)
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

PORT = 7441
BASE = f"http://127.0.0.1:{PORT}/"

# crates/ralphy-daemon/tests/wb_fleet_349.py -> repo root is 4 dirs up.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
EXE = os.path.join(REPO_ROOT, "target", "debug", "ralphy.exe" if os.name == "nt" else "ralphy")
SHOT_DIR = os.path.join(REPO_ROOT, "docs", "screenshots")
SH = "Alpine.$data(document.querySelector('[x-data]'))"

PEER_ID = "01PEERWSLFIXTURE0000000000"
PEER_NAME = "wsl-box"
PEER_ENV = "WSL: Ubuntu-22.04"
PEER_TOKEN = "peer-fixture-token"
# The COLLIDING slug: the local daemon registers this too, so the sidebar must
# show two rows for it, in two groups.
SHARED_SLUG = "ralphy-lab/shared-repo"
PEER_PATH = "/home/operator/dev/shared-repo"

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
    """The two routes a peer must answer. A wrong bearer is a 401, so the
    descriptor's token is genuinely exercised rather than assumed."""

    protocol_version = "HTTP/1.1"

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
            return
        if self.path == "/api/peer/hello":
            self._json(
                200,
                {
                    "daemon_id": PEER_ID,
                    "name": PEER_NAME,
                    "avatar": "🐺",
                    "environment": PEER_ENV,
                    "protocol_version": 1,
                },
            )
        elif self.path == "/api/repos":
            self._json(
                200,
                [
                    {
                        "slug": SHARED_SLUG,
                        "path": PEER_PATH,
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
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def empty_env(daemon_dir):
    """A scratch registry + empty vendor stores: the operator's own daemon dir
    (and its login policy) is never touched, and the usage scan finds nothing."""
    empty = tempfile.mkdtemp(prefix="wb349_empty_")
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


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed_local_repo():
    """A committed git repo whose slug COLLIDES with the peer's, so the sidebar
    has to keep both. `origin` fixes the slug (ADR-0008 D7)."""
    d = Path(tempfile.mkdtemp(prefix="wb349_local_")) / "shared-repo"
    d.mkdir()
    (d / "README.md").write_text("# shared-repo\n\nThe #349 fleet fixture.\n", encoding="utf-8")
    git(d, "init", "-b", "main")
    git(d, "config", "user.email", "wb349@example.com")
    git(d, "config", "user.name", "wb349")
    git(d, "remote", "add", "origin", f"https://github.com/{SHARED_SLUG}.git")
    git(d, "add", "-A")
    git(d, "commit", "-m", "fixture")
    return str(d)


def seed_peer_descriptor(daemon_dir, port):
    peers = Path(daemon_dir) / "peers"
    peers.mkdir(parents=True, exist_ok=True)
    (peers / f"{PEER_ID}.toml").write_text(
        "\n".join(
            [
                f'daemon_id = "{PEER_ID}"',
                f'name = "{PEER_NAME}"',
                'avatar = "🐺"',
                'address = "127.0.0.1"',
                f"port = {port}",
                f'environment = "{PEER_ENV}"',
                f'token = "{PEER_TOKEN}"',
                "protocol_version = 1",
                "",
                "[nudge]",
                'distro = "Ubuntu-22.04"',
                'unit = "ralphy-daemon.service"',
                "",
            ]
        ),
        encoding="utf-8",
    )


def baptize(daemon_dir):
    """Write `daemon.toml` directly rather than driving `daemon setup`, whose
    prompts are an interactive stdin contract this suite has no stake in. The
    local daemon needs an identity because `/api/fleet` keys the LOCAL rows on
    its `daemon_id`."""
    Path(daemon_dir).mkdir(parents=True, exist_ok=True)
    (Path(daemon_dir) / "daemon.toml").write_text(
        # A REAL ULID: 26 Crockford base32 chars, and `I`/`L`/`O`/`U` are NOT in
        # that alphabet. An invalid one makes the daemon log a warn and serve the
        # whole fleet with an empty `daemon_id` and no name — silently.
        'id = "01ARZ3NDEKTSV4RRFFQ69G5FAV"\nname = "anvil"\navatar = "🐙"\n',
        encoding="utf-8",
    )


def register(daemon_dir, path):
    subprocess.run(
        [EXE, "daemon", "add", path],
        env=dict(os.environ, RALPHY_DAEMON_DIR=daemon_dir),
        check=True,
        capture_output=True,
        encoding="utf-8",
    )


def build():
    # The UI assets are `include_dir!`-embedded, so the binary must be rebuilt
    # after any assets/ui edit or the browser loads yesterday's sidebar.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def launch(daemon_dir):
    return subprocess.Popen(
        [EXE, "daemon", "--port", str(PORT)],
        env=empty_env(daemon_dir),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


# The rendered sidebar, read from the live DOM: every group header with its
# label/name/state, and every row with the group it sits under.
#
# `.env-block` is `display: contents`, so a row is NOT a DOM descendant of its
# header — the grouping is expressed by document ORDER inside the block, which
# is what this walk reads. Every measurement is gated on real layout
# (`offsetParent`/`clientWidth`): a hidden box reads zero and passes a "fits"
# assertion vacuously.
SIDEBAR_EXPR = """
() => {
  const blocks = Array.from(document.querySelectorAll('.projects .env-block'));
  return blocks.map(b => {
    const h = b.querySelector('.env-group');
    const state = h ? h.querySelector('.peer-state') : null;
    return {
      header: h ? {
        label: (h.querySelector('.env-label') || {}).textContent?.trim() ?? '',
        name: (h.querySelector('.env-daemon') || {}).textContent?.trim() ?? '',
        state: state ? state.textContent.trim() : '',
        laid: h.offsetParent !== null && h.clientWidth > 0,
      } : null,
      rows: Array.from(b.querySelectorAll('li.project')).map(r => {
        const n = r.querySelector('.project-slug');
        return {
          slug: n ? n.getAttribute('title') : '',
          peer: r.classList.contains('peer'),
          laid: r.offsetParent !== null && r.clientWidth > 0,
          title: (r.querySelector('.project-head') || {}).getAttribute?.('title') ?? '',
        };
      }),
    };
  });
}
"""


def sidebar(page):
    page.wait_for_function(
        "() => document.querySelectorAll('.projects li.project').length >= 2",
        timeout=15000,
    )
    return page.evaluate(SIDEBAR_EXPR)


def main():
    build()
    daemon_dir = tempfile.mkdtemp(prefix="wb349_daemon_")
    baptize(daemon_dir)
    local_path = seed_local_repo()
    register(daemon_dir, local_path)

    peer_port = free_port()
    stub = start_peer_stub(peer_port)
    seed_peer_descriptor(daemon_dir, peer_port)

    proc = launch(daemon_dir)
    try:
        if not wait_listening(BASE) or proc.poll() is not None:
            print("[FAIL] daemon did not start (or 7441 answers a FOREIGN listener)", flush=True)
            sys.exit(1)

        with sync_playwright() as p:
            browser = p.chromium.launch()
            page = browser.new_page(viewport={"width": 1400, "height": 900})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            page.goto(BASE, wait_until="domcontentloaded")

            blocks = sidebar(page)
            headers = [b["header"] for b in blocks if b["header"]]
            peer_block = next(
                (b for b in blocks if b["header"] and b["header"]["label"] == PEER_ENV), None
            )
            check(
                "the peer's environment group header renders its label and name",
                peer_block is not None
                and peer_block["header"]["laid"]
                and peer_block["header"]["name"] == PEER_NAME,
                "headers={}".format(headers),
            )

            peer_rows = peer_block["rows"] if peer_block else []
            check(
                "the peer's repo row is present under its header",
                len(peer_rows) == 1
                and peer_rows[0]["slug"] == SHARED_SLUG
                and peer_rows[0]["peer"]
                and peer_rows[0]["laid"],
                "rows={}".format(peer_rows),
            )

            local_block = next(
                (b for b in blocks if b["header"] and b["header"]["label"] != PEER_ENV), None
            )
            local_rows = local_block["rows"] if local_block else []
            check(
                "the local repo keeps its own row under its own group",
                len(local_rows) == 1
                and local_rows[0]["slug"] == SHARED_SLUG
                and not local_rows[0]["peer"],
                "rows={}".format(local_rows),
            )
            check(
                "the LOCAL group header names its own environment and daemon too",
                local_block is not None
                and local_block["header"]["label"] != ""
                and local_block["header"]["name"] == "anvil",
                "header={}".format(local_block["header"] if local_block else None),
            )

            # The collision case, read from the app's own state: the same slug on
            # two daemons must be two DISTINCT keys with two distinct paths.
            keyed = page.evaluate(
                f"() => {SH}.projects.map(p => ({{ key: p.key || null, slug: p.slug, path: p.path, daemon: p.daemon || null }}))"
            )
            shared = [r for r in keyed if r["slug"] == SHARED_SLUG]
            keys = {r["key"] for r in shared}
            paths = {r["path"] for r in shared}
            check(
                "the same slug on two daemons is two rows, neither overwriting the other",
                len(shared) == 2 and len(paths) == 2 and PEER_PATH in paths and len(keys) == 2,
                "shared={}".format(shared),
            )

            # POSITIVE CONTROL first: the same click on a LOCAL row must open it.
            # Without this, an unwired `.project-head` handler would satisfy the
            # peer assertion below for the wrong reason.
            page.evaluate(
                """() => {
                  const r = Array.from(document.querySelectorAll('li.project:not(.peer)'))[0];
                  r.querySelector('.project-head').click();
                }"""
            )
            page.wait_for_timeout(300)
            opened = page.evaluate(f"() => {SH}.openSlug")
            check(
                "the control: clicking a LOCAL row does open that project",
                opened == SHARED_SLUG,
                "openSlug={}".format(opened),
            )
            # Close it again so the peer assertion starts from a clean state.
            page.evaluate(
                """() => {
                  const r = Array.from(document.querySelectorAll('li.project:not(.peer)'))[0];
                  r.querySelector('.project-head').click();
                }"""
            )
            page.wait_for_timeout(300)

            # A peer row is inert: the SAME click must not open a project.
            page.evaluate(
                """() => {
                  const r = Array.from(document.querySelectorAll('li.project.peer'))[0];
                  r.querySelector('.project-head').click();
                }"""
            )
            page.wait_for_timeout(300)
            open_slug = page.evaluate(f"() => {SH}.openSlug")
            check(
                "clicking a peer row does not open a project (nothing is federated yet)",
                open_slug is None,
                "openSlug={}".format(open_slug),
            )

            shot = os.path.join(SHOT_DIR, "349-fleet-grouping-2026-07-28.png")
            os.makedirs(SHOT_DIR, exist_ok=True)
            page.screenshot(path=shot)
            print(f"[INFO] screenshot {shot}", flush=True)

            # The peer goes away. `/api/fleet` holds no cached liveness, so a
            # reload is enough — nothing has to be invalidated.
            stub.shutdown()
            stub.server_close()
            page.reload(wait_until="domcontentloaded")

            blocks = sidebar(page)
            down_block = next(
                (b for b in blocks if b["header"] and b["header"]["label"] == PEER_ENV), None
            )
            check(
                "a peer that stopped answering is MARKED unreachable, not removed",
                down_block is not None and down_block["header"]["state"] == "unreachable",
                "header={}".format(down_block["header"] if down_block else None),
            )
            check(
                "the unreachable peer's rows stay listed",
                down_block is not None and len(down_block["rows"]) == 1,
                "rows={}".format(down_block["rows"] if down_block else None),
            )
            local_after = next(
                (b for b in blocks if b["header"] and b["header"]["label"] != PEER_ENV), None
            )
            check(
                "the local sidebar is untouched by the peer going down",
                local_after is not None and len(local_after["rows"]) == 1,
                "rows={}".format(local_after["rows"] if local_after else None),
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
    if len(results) != 11:
        print(f"[FAIL] expected 11 checks, ran {len(results)}", flush=True)
        sys.exit(1)
    sys.exit(0 if all(results) else 1)


if __name__ == "__main__":
    main()
