"""A session list that did not hear from a peer never starts that peer's
consoles a second time.

Two REAL daemons: a local one and a peer "on another machine" (a descriptor
with no `[nudge]`), with a TCP proxy between them that can drop, standing in
for the ssh tunnel. The proxy's `down()` closes its port and cuts every live
connection, which is what a dead `ssh -L` looks like.

Scenario 1  five plain consoles on the peer project: five PTYs on the peer
Scenario 2  the link drops and the page reloads: no console is launched
            again, the five windows are placeholders, the peer keeps five PTYs
Scenario 3  the link comes back: the five windows attach to the SAME five
            sessions, still without a launch
Scenario 4  the peer answers the fleet probe but not its session list: the
            windows wait, say the peer does not answer, and launch nothing,
            a click on "Try again" included;
            once the list answers, they attach to the same sessions
Scenario 5  the link drops, the page reloads, and the link comes back 1.5 s
            later, during the restore: no launch, five PTYs
Scenario 6  the next reload with the link up: five windows on the same five
            sessions, nothing adopted in a cascade
Scenario 7  zero `pageerror` events over the whole pass

Before the fix, scenario 2 launched all five consoles again (`console=1`), and
a peer without the record join (every release up to rc.38) started a second
PTY for each. `RALPHY_TEST_PEER_EXE=<path to an older ralphy>` runs the peer
from another build; the checks are the same.

Scratch stores only. Every process is stopped by its own handle, NEVER by name
(`ralphy.exe` doubles as the orchestrator on this host).

Writes .ralphy/screenshots/console-peer-unanswered.png.
Run: python tests/browser/console/wb_console_peer_unanswered.py   (exit 0 = all pass)
"""

import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[3]
EXE = REPO_ROOT / "target" / "debug" / ("ralphy.exe" if os.name == "nt" else "ralphy")
PEER_EXE = Path(os.environ.get("RALPHY_TEST_PEER_EXE") or EXE)
SHOT_DIR = REPO_ROOT / ".ralphy" / "screenshots"
LOCAL_PORT = 7477
PEER_PORT = 7478
LOCAL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
TOKEN = "peer-unanswered-token"
SLUG = "ralphy-lab/unanswered-repo"
REF = f"{PEER_ID}/{SLUG}"
N = 5
SH = "Alpine.$data(document.querySelector('[x-data]'))"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def listening(port, timeout):
    deadline = time.time() + timeout
    while True:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return True
        except OSError:
            if time.time() >= deadline:
                return False
            time.sleep(0.3)


class Proxy:
    """127.0.0.1:<port> -> the peer daemon. With `mute_list` set, a connection
    that asks for the peer's session list is cut: the fleet probe answers and
    the list does not, as when only the slower read times out."""

    def __init__(self, port):
        self.port = port
        self.lock = threading.Lock()
        self.pairs = []
        self.listener = None
        self.mute_list = False
        self.up()

    def up(self):
        s = socket.socket()
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", self.port))
        s.listen(64)
        self.listener = s
        threading.Thread(target=self._accept, args=(s,), daemon=True).start()

    def down(self):
        with self.lock:
            s, self.listener = self.listener, None
            pairs, self.pairs = self.pairs, []
        if s:
            s.close()
        for pair in pairs:
            for x in pair:
                try:
                    x.close()
                except OSError:
                    pass

    def _accept(self, s):
        while True:
            try:
                c, _ = s.accept()
            except OSError:
                return
            try:
                u = socket.create_connection(("127.0.0.1", PEER_PORT))
            except OSError:
                c.close()
                continue
            with self.lock:
                if self.listener is not s:
                    c.close()
                    u.close()
                    return
                self.pairs.append((c, u))
            for src, dst, outbound in ((c, u, True), (u, c, False)):
                threading.Thread(target=self._pipe, args=(src, dst, outbound), daemon=True).start()

    def _pipe(self, src, dst, outbound):
        try:
            while True:
                d = src.recv(65536)
                if not d:
                    break
                # Every request on the connection, not only the first: the
                # daemon's client pools its peer connections.
                if outbound and self.mute_list and b" /api/sessions" in d:
                    break
                dst.sendall(d)
        except OSError:
            pass
        for x in (src, dst):
            try:
                x.close()
            except OSError:
                pass


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def seed_repo():
    root = Path(tempfile.mkdtemp(prefix="wb_unanswered_repo_"))
    (root / "README.md").write_text("# unanswered\n", encoding="utf-8")
    git(root, "init", "-b", "main")
    git(root, "-c", "user.email=u@example.com", "-c", "user.name=u", "remote", "add", "origin", f"https://github.com/{SLUG}.git")
    git(root, "add", "-A")
    git(root, "-c", "user.email=u@example.com", "-c", "user.name=u", "commit", "-m", "fixture")
    return root


def env_for(store, token=None):
    empty = tempfile.mkdtemp(prefix="wb_unanswered_usage_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=str(store),
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        RALPHY_COPILOT_DB=os.path.join(empty, "copilot-none.db"),
        RALPHY_CURSOR_DIR=empty,
        RALPHY_GEMINI_DIR=empty,
    )
    # Without a distro the peer is "on another machine": consoles go through the relay.
    env.pop("WSL_DISTRO_NAME", None)
    if token:
        env["RALPHY_DAEMON_TOKEN"] = token
    return env


def peer_ptys():
    req = urllib.request.Request(
        f"http://127.0.0.1:{PEER_PORT}/api/sessions?local=1",
        headers={"Authorization": f"Bearer {TOKEN}"},
    )
    with urllib.request.urlopen(req, timeout=5) as r:
        return sorted(s["id"] for s in json.loads(r.read()))


# The windows of the peer project: desk id, session id, placeholder or not.
WINS = f"""
() => Array.from(document.querySelectorAll('.session-window'))
  .filter(w => w._deskRepo === '{REF}')
  .map(w => ({{
    id: w._deskId,
    session: (w._term && w._term.sessionId) ?? null,
    placeholder: w.classList.contains('placeholder'),
    text: (w.querySelector('.session-offline > p') || {{}}).textContent || '',
  }}))
"""


def windows(page):
    return page.evaluate(WINS)


def wait_for(page, pred, timeout):
    deadline = time.time() + timeout
    got = windows(page)
    while time.time() < deadline:
        got = windows(page)
        if pred(got):
            return got
        time.sleep(0.25)
    return got


def reload(page):
    page.reload(wait_until="domcontentloaded")
    page.wait_for_function("() => !!window.WBConsole", timeout=10000)


def fleet_read(page):
    page.evaluate(f"() => {SH}.loadRepos({{ git: false }})")


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild after any assets/ui edit.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)


def main():
    os.makedirs(SHOT_DIR, exist_ok=True)
    build()
    for port in (LOCAL_PORT, PEER_PORT):
        if listening(port, 0.5):
            print(f"[FAIL] port {port} answers a FOREIGN listener", flush=True)
            sys.exit(1)
    local_store = Path(tempfile.mkdtemp(prefix="wb_unanswered_local_"))
    peer_store = Path(tempfile.mkdtemp(prefix="wb_unanswered_peer_"))
    (local_store / "daemon.toml").write_text(f'id = "{LOCAL_ID}"\nname = "anvil"\navatar = "🐙"\n', encoding="utf-8")
    (peer_store / "daemon.toml").write_text(f'id = "{PEER_ID}"\nname = "far-box"\navatar = "🐺"\n', encoding="utf-8")
    subprocess.run([str(PEER_EXE), "daemon", "add", str(seed_repo())], env=env_for(peer_store), check=True, capture_output=True)
    proxy_port = free_port()
    (local_store / "peers").mkdir()
    (local_store / "peers" / f"{PEER_ID}.toml").write_text(
        "\n".join(
            [
                f'daemon_id = "{PEER_ID}"',
                'name = "far-box"',
                'avatar = "🐺"',
                'address = "127.0.0.1"',
                f"port = {proxy_port}",
                'environment = "macOS 12"',
                f'token = "{TOKEN}"',
                "protocol_version = 3",
                "",
            ]
        ),
        encoding="utf-8",
    )

    procs = []
    proxy = None
    try:
        procs.append(
            subprocess.Popen(
                [str(PEER_EXE), "daemon", "--port", str(PEER_PORT)],
                env=env_for(peer_store, TOKEN),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        )
        procs.append(
            subprocess.Popen(
                [str(EXE), "daemon", "--port", str(LOCAL_PORT)],
                env=env_for(local_store),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        )
        if not (listening(PEER_PORT, 30) and listening(LOCAL_PORT, 30)):
            print("[FAIL] a daemon did not start", flush=True)
            sys.exit(1)
        proxy = Proxy(proxy_port)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_page(viewport={"width": 1500, "height": 950})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            launches = []
            page.on(
                "websocket",
                lambda ws: launches.append(ws.url) if "/ws/session" in ws.url and "console=1" in ws.url else None,
            )
            page.goto(f"http://127.0.0.1:{LOCAL_PORT}/", wait_until="domcontentloaded")
            page.wait_for_function("() => !!window.WBConsole", timeout=10000)
            page.wait_for_function(
                f"() => {SH}.fleetPeers.some(p => p.daemon_id === '{PEER_ID}' && p.state === 'reachable')",
                timeout=30000,
            )

            # --- scenario 1 ----------------------------------------------------
            for _ in range(N):
                page.evaluate(f"() => window.WBConsole.open({{ repo: '{REF}', plain: true }})")
                page.wait_for_timeout(300)
            wins = wait_for(page, lambda ws: len(ws) == N and all(w["session"] for w in ws), 30)
            # The desk is written after the announcement; give the flush a moment.
            page.wait_for_timeout(2500)
            first = peer_ptys()
            owned = {w["id"]: w["session"] for w in wins}
            check(f"{N} consoles run on the peer", len(first) == N and len(owned) == N, f"ptys={first} wins={owned}")

            # --- scenario 2 ----------------------------------------------------
            launches.clear()
            proxy.down()
            reload(page)
            wins = wait_for(page, lambda ws: len(ws) == N and all(w["placeholder"] for w in ws), 15)
            check(
                "with the link down, the reload leaves placeholders",
                len(wins) == N and all(w["placeholder"] for w in wins),
                f"wins={[(w['session'], w['placeholder'], w['text']) for w in wins]}",
            )
            check("…and launches nothing", not launches, f"launches={launches}")
            check("…and the peer keeps its PTYs", peer_ptys() == first, f"ptys={peer_ptys()}")

            # --- scenario 3 ----------------------------------------------------
            proxy.up()
            fleet_read(page)
            wins = wait_for(page, lambda ws: {w["id"]: w["session"] for w in ws} == owned, 25)
            check(
                "the link back, every window attaches to its own session",
                {w["id"]: w["session"] for w in wins} == owned,
                f"wins={[(w['id'], w['session'], w['text']) for w in wins]}",
            )
            check("…without a launch", not launches, f"launches={launches}")
            check("…and the peer still has the same PTYs", peer_ptys() == first, f"ptys={peer_ptys()}")

            # --- scenario 4 ----------------------------------------------------
            launches.clear()
            proxy.mute_list = True
            reload(page)
            # Two fleet reads that call the peer reachable: each box asks the
            # list again, and the list still did not hear from the peer.
            for _ in range(2):
                page.wait_for_timeout(4000)
                fleet_read(page)
            page.wait_for_timeout(4000)
            wins = windows(page)
            check(
                "the peer answers its probe but not its list: the windows wait and say so",
                len(wins) == N and all(w["placeholder"] and w["text"].endswith("does not answer.") for w in wins),
                f"wins={[(w['session'], w['placeholder'], w['text']) for w in wins]}",
            )
            check("…and launch nothing", not launches, f"launches={launches}")
            # The operator's click asks the list too, and does not launch on it.
            # A DOM click: the cascade covers the first box's button, and this
            # checks what the click does, not where it lands.
            page.evaluate("() => document.querySelector('.session-window.placeholder .session-reconnect').click()")
            page.wait_for_timeout(4000)
            wins = windows(page)
            check(
                "a click on Try again while the list is silent launches nothing",
                not launches and all(w["placeholder"] for w in wins),
                f"launches={launches} wins={[(w['session'], w['text']) for w in wins]}",
            )
            proxy.mute_list = False
            fleet_read(page)
            wins = wait_for(page, lambda ws: {w["id"]: w["session"] for w in ws} == owned, 25)
            check(
                "the list answers again: every window attaches to its own session",
                {w["id"]: w["session"] for w in wins} == owned and not launches,
                f"wins={[(w['id'], w['session'], w['text']) for w in wins]} launches={launches}",
            )

            # --- scenario 5 ----------------------------------------------------
            launches.clear()
            proxy.down()
            reload(page)
            page.wait_for_timeout(1500)
            proxy.up()
            fleet_read(page)
            wins = wait_for(page, lambda ws: {w["id"]: w["session"] for w in ws} == owned, 25)
            check("the link back during the restore launches nothing", not launches, f"launches={launches}")
            check("…and the peer still has the same PTYs", peer_ptys() == first, f"ptys={peer_ptys()}")

            # --- scenario 6 ----------------------------------------------------
            reload(page)
            page.wait_for_timeout(6000)
            wins = windows(page)
            check(
                "the next reload shows the same windows on the same sessions, none adopted",
                {w["id"]: w["session"] for w in wins} == owned,
                f"wins={[(w['id'], w['session']) for w in wins]}",
            )
            page.screenshot(path=str(SHOT_DIR / "console-peer-unanswered.png"))

            browser.close()
            # --- scenario 7 ----------------------------------------------------
            check("zero pageerror events captured", not thrown, f"got={thrown}")
    finally:
        if proxy:
            proxy.down()
        for proc in procs:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except Exception:
                proc.kill()
        for store in (local_store, peer_store):
            shutil.rmtree(store, ignore_errors=True)

    # The count floor: an early exit must not report success on a few checks.
    ok = all(results) and len(results) == 15
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
