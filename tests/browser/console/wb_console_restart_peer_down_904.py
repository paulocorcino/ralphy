"""A Restart keeps its window while the peer is down and a sibling sleeps (#904).

Two REAL daemons on scratch stores: the main one serves the page, and a peer
hosts the project and runs `session_test_child` as each console. The desk is
a locked fence of 8 plain consoles on the peer project, one of them maximized,
so it covers the other 7.

Steps:
  1  load the page: the 8 windows attach to their sessions
  2  wait until a covered sibling sleeps (the dormancy timer, about 15 s)
  3  stop the peer daemon, and wait until the fleet calls it unreachable
  4  Restart the maximized window and confirm

Claims (one PASS/FAIL line each):
  - a covered sibling is dormant before the Restart
  - 8 windows remain
  - restarted window is the peer placeholder
  - 0 pageerror

Before the fix, step 4 woke the sleeping sibling, its peer-down strip threw,
and the restarted window was never built: 7 windows remained.

Scratch stores only. Every process is stopped by its own handle, NEVER by name
(`ralphy.exe` doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_restart_peer_down_904.py   (exit 0 = all pass)
"""

import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.stdout.reconfigure(encoding="utf-8")

REPO_ROOT = Path(__file__).resolve().parents[3]
TARGET = REPO_ROOT / "target" / "debug"
EXE = TARGET / ("ralphy.exe" if os.name == "nt" else "ralphy")
CHILD = TARGET / ("session_test_child.exe" if os.name == "nt" else "session_test_child")
MAIN_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAZ"
PEER_NAME = "wsl-fake"
PEER_ENV = "WSL: Fake-22.04"
TOKEN = "restart-peer-down-token"
# The peer protocol of the daemon under test: a descriptor with another
# version is refused before the handshake.
PROTOCOL = int(
    re.search(
        r"pub const PEER_PROTOCOL_VERSION: u32 = (\d+);",
        (REPO_ROOT / "crates" / "ralphy-daemon" / "src" / "peer.rs").read_text(encoding="utf-8"),
    ).group(1)
)
VIEW = {"width": 1920, "height": 1000}

# The operator's SomosChat fence: 8 consoles at their real rects.
FENCE = (20.0, 2850.0, 2293.0, 1461.0)
WINS = [
    ("w-1", "somoschat #1", 43.0, 2898.0, 675.0, 422.0),
    ("w-2", "somoschat #2", 739.0, 3746.0, 631.0, 413.0),
    ("w-5", "somoschat #5", 49.0, 3746.0, 675.0, 398.0),
    ("w-chan", "Channel2api", 738.0, 3332.0, 634.0, 396.0),
    ("w-tele", "Telegram2api", 1386.0, 3332.0, 560.0, 404.0),
    ("w-prod", "SChat Product", 724.0, 2896.0, 647.0, 414.0),
    ("w-plane", "SChat Channel Plane", 1384.0, 2894.0, 561.0, 417.0),
    ("w-waba", "Waba", 45.0, 3335.0, 681.0, 392.0),
]
TARGET_ID = "w-prod"
FIND = f"[...document.querySelectorAll('.session-window')].find((w) => w._deskId === '{TARGET_ID}')"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'} {name} {detail}", flush=True)


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


def build():
    if os.environ.get("RALPHY_TEST_SKIP_BUILD") == "1":
        return
    # The UI assets are `include_dir!`-embedded: rebuild after any assets/ui edit.
    subprocess.run(["cargo", "build", "-p", "ralphy-cli", "--bin", "ralphy"], cwd=REPO_ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "-p", "ralphy-daemon", "--bin", "session_test_child"], cwd=REPO_ROOT, check=True
    )


def env_for(store, token=None):
    empty = tempfile.mkdtemp(prefix="wb_904_usage_")
    env = dict(
        os.environ,
        RALPHY_DAEMON_DIR=str(store),
        RALPHY_DAEMON_AGENT_OVERRIDE=str(CHILD),
        RALPHY_USAGE_DIR=empty,
        RALPHY_CLAUDE_PROJECTS_DIR=empty,
        RALPHY_CODEX_DIR=empty,
        RALPHY_OPENCODE_DB=os.path.join(empty, "none.db"),
        RALPHY_KIMI_DIR=empty,
        RALPHY_KIMI_CODE_DIR=empty,
        RALPHY_COPILOT_DB=os.path.join(empty, "copilot-none.db"),
        RALPHY_CURSOR_DIR=empty,
    )
    if token:
        env["RALPHY_DAEMON_TOKEN"] = token
    return env


def git(cwd, *args):
    subprocess.run(["git", "-c", "user.email=u@example.com", "-c", "user.name=u", *args], cwd=cwd, check=True, capture_output=True)


def seed_repo():
    root = Path(tempfile.mkdtemp(prefix="wb_904_repo_"))
    (root / "README.md").write_text("# fixture\n", encoding="utf-8")
    git(root, "init", "-b", "main")
    git(root, "add", "-A")
    git(root, "commit", "-m", "fixture")
    return root


def rect_toml(left, top, width, height):
    return f"rect = {{ left = {left}, top = {top}, width = {width}, height = {height} }}"


def desk_toml(ref):
    text = ""
    for n, (wid, name, left, top, width, height) in enumerate(WINS):
        text += (
            "[[windows]]\n"
            f'id = "{wid}"\nrepo = "{ref}"\nagent = "console"\nkind = "console"\n'
            f"max = {'true' if wid == TARGET_ID else 'false'}\n"
            f'consoleName = "{name}"\nts = {100 + n}\n{rect_toml(left, top, width, height)}\n\n'
        )
    return text + (
        '[[fences]]\nid = "f-somos"\nname = "SomosChat"\nlocked = true\nts = 50\n'
        f"{rect_toml(*FENCE)}\n"
    )


def seed(main_store, peer_store, peer_port, repo):
    (peer_store / "daemon.toml").write_text(f'id = "{PEER_ID}"\nname = "{PEER_NAME}"\navatar = "🦊"\n', encoding="utf-8")
    out = subprocess.run(
        [str(EXE), "daemon", "add", str(repo)], env=env_for(peer_store), check=True, capture_output=True, encoding="utf-8"
    ).stdout
    slug = out.strip().split("registered ", 1)[1].split(" →")[0].strip()
    (main_store / "daemon.toml").write_text(f'id = "{MAIN_ID}"\nname = "ralphy"\navatar = "🐺"\n', encoding="utf-8")
    (main_store / "peers").mkdir()
    (main_store / "peers" / f"{PEER_ID}.toml").write_text(
        f'daemon_id = "{PEER_ID}"\nname = "{PEER_NAME}"\navatar = "🦊"\naddress = "127.0.0.1"\n'
        f'port = {peer_port}\nenvironment = "{PEER_ENV}"\ntoken = "{TOKEN}"\n'
        f"protocol_version = {PROTOCOL}\n",
        encoding="utf-8",
    )
    (main_store / "desk.toml").write_text(desk_toml(f"{PEER_ID}/{slug}"), encoding="utf-8")


def start(store, port, token=None):
    return subprocess.Popen(
        [str(EXE), "daemon", "--port", str(port)],
        env=env_for(store, token),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )


def stop(proc):
    if proc.poll() is not None:
        return
    if os.name == "nt":
        # By its own PID, with its tree: the consoles a daemon spawned outlive a
        # bare terminate on Windows and hold `session_test_child.exe` open.
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True, check=False)
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def restart_target(page):
    page.evaluate(f"() => {{ window.__old = {FIND}; window.__old.querySelector('.session-restart').click(); }}")
    confirm = page.locator(".wb-confirm .btn.danger, .wb-confirm .btn.accent")
    confirm.wait_for(timeout=5000)
    confirm.click()


def target_state(page):
    return page.evaluate(
        f"""() => {{
          const w = {FIND};
          return {{
            total: document.querySelectorAll('.session-window').length,
            present: !!w,
            fresh: !!w && w !== window.__old,
            placeholder: !!w && w.classList.contains('placeholder'),
            text: (w && w.querySelector('.session-offline')?.textContent?.trim()) || '',
          }};
        }}"""
    )


PEER = f"Alpine.store('projects').fleetPeers.find((p) => p.daemon_id === '{PEER_ID}')"


def fleet_read(page):
    page.evaluate("() => Alpine.store('projects').loadRepos({ git: false })")


def run(page, peer):
    thrown = []
    page.on("pageerror", lambda e: thrown.append(str(e)))
    page.goto(page.base, wait_until="domcontentloaded")
    page.wait_for_selector("[x-data]", timeout=10000)
    page.evaluate("() => { Alpine.store('tabs').activate('consoles'); }")
    page.wait_for_function(
        "() => [...document.querySelectorAll('.session-window')]"
        ".filter((w) => w._term?.sessionId != null).length === 8",
        timeout=60000,
    )
    page.evaluate("() => { const ws = document.getElementById('workspace'); ws.scrollLeft = 0; ws.scrollTop = 2830; }")
    # The names the page knows the peer by, read while it is reachable.
    page.wait_for_function(f"() => {PEER}?.state === 'reachable'", timeout=30000)
    names = page.evaluate(f"() => {{ const p = {PEER}; return [p.name, p.environment].filter(Boolean); }}")

    # The maximized window covers its siblings, and a covered window sleeps
    # when its dormancy timer runs out. The page has no switch to sleep a
    # window now, so the check waits for the timer.
    dormant = 0
    try:
        page.wait_for_function("() => document.querySelectorAll('.session-window.dormant').length > 0", timeout=90000)
        dormant = page.evaluate("() => document.querySelectorAll('.session-window.dormant').length")
    except Exception as e:  # the claim below reports it
        print("  no sibling slept:", str(e).splitlines()[0], flush=True)
    check("a covered sibling is dormant before the Restart", dormant > 0, f"dormant={dormant}")

    stop(peer)
    fleet_read(page)
    page.wait_for_function(f"() => {PEER}?.state !== 'reachable'", timeout=60000)

    restart_target(page)
    deadline = time.time() + 30
    state = target_state(page)
    while time.time() < deadline and not (state["fresh"] and state["placeholder"]):
        page.wait_for_timeout(500)
        state = target_state(page)
    # The siblings' strips and the placeholder's redraws land after the build.
    page.wait_for_timeout(3000)
    state = target_state(page)
    check("8 windows remain", state["total"] == 8, f"windows={state['total']}")
    check(
        "restarted window is the peer placeholder",
        state["fresh"] and state["placeholder"] and any(n in state["text"] for n in names),
        f"names={names} state={state}",
    )
    check("0 pageerror", not thrown, f"got={thrown}")


def main():
    build()
    main_port, peer_port = free_port(), free_port()
    main_store = Path(tempfile.mkdtemp(prefix="wb_904_main_"))
    peer_store = Path(tempfile.mkdtemp(prefix="wb_904_peer_"))
    repo = seed_repo()
    seed(main_store, peer_store, peer_port, repo)
    procs = []
    try:
        peer = start(peer_store, peer_port, TOKEN)
        procs.append(peer)
        procs.append(start(main_store, main_port))
        if not (listening(peer_port, 30) and listening(main_port, 30)):
            print("FAIL a daemon did not start", flush=True)
            sys.exit(1)
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_page(viewport=VIEW)
            page.base = f"http://127.0.0.1:{main_port}/"
            run(page, peer)
            browser.close()
    finally:
        for proc in procs:
            stop(proc)
        for path in (main_store, peer_store, repo):
            shutil.rmtree(path, ignore_errors=True)

    # The count floor: an early exit must not report success on a few checks.
    ok = all(results) and len(results) == 4
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
