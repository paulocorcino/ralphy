"""A live console whose peer stops answering waits for the fleet. It does not
dial the peer again until the fleet says the peer is back.

Two REAL daemons and the TCP proxy of `wb_console_peer_unanswered.py`: its
`down()` cuts every live connection, as a dead `ssh -L` does.

The page reaches the console sockets through a WebSocket route that closes
the page side cleanly, with 1000, when the daemon drops the socket: a proxy in
the path can do that. On a direct localhost socket Chromium reports the same
drop as `1005, wasClean=false`, which hid the case. Measured 2026-10-05 from a
browser behind dev tunnels: two consoles of a peer that left the network
printed "[session closed]" at once and never tried again; this route gives the
same result on the code before the fix. `RALPHY_TEST_DIRECT=1` runs without the
route.

Scenario 1  two plain consoles run on the peer
Scenario 2  the link drops: each window shows one line that names the peer's
            state, with "Try again" and the daemon's diagnosis behind a closed
            "Details"; the terminal keeps its text
Scenario 3  the link stays down for HOLD_S (180 s, past the old give-up):
            no window opens a socket, none prints "[session closed]", and
            each still holds its session
Scenario 4  the link comes back and the fleet is read: every window attaches
            to its own session again, the line goes, nothing is launched
Scenario 5  zero `pageerror` events over the whole pass

Before the fix, scenario 3 opened a socket per window every 1 to 15 s, and
each one failed. After 11 failed opens (`MAX_FAILED_REOPENS` is 10) the window
gave up with "[session closed]", 120 to about 156 s after the drop with the
1-2-4-8-15 s backoff and its 30% jitter, and it did not come back with the
peer. `RALPHY_TEST_HOLD_S` shortens the wait for a quick run.

Scratch stores only. Every process is stopped by its own handle, NEVER by name
(`ralphy.exe` doubles as the orchestrator on this host).

Writes .ralphy/screenshots/console-peer-hold.png.
Run: python tests/browser/console/wb_console_peer_hold.py   (exit 0 = all pass)
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

import wb_console_peer_unanswered as base

sys.stdout.reconfigure(encoding="utf-8")

N = 2
# Past the old give-up: 11 failed opens, 1+2+4+8+15*7 = 120 s of backoff plus
# up to 30% jitter.
HOLD_S = int(os.environ.get("RALPHY_TEST_HOLD_S") or 180)
SH = base.SH
REF = base.REF
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


# The windows of the peer project: session id, the hold line, its button and
# its closed detail, and the end of the terminal's text.
WINS = f"""
() => Array.from(document.querySelectorAll('.session-window'))
  .filter(w => w._deskRepo === '{REF}')
  .map(w => {{
    const strip = w.querySelector('.session-peer-down');
    const btn = strip && strip.querySelector('.session-reconnect');
    const det = strip && strip.querySelector('details.session-detail');
    const b = w._term && w._term.term && w._term.term.buffer.active;
    let screen = '';
    if (b) for (let y = 0; y < b.length; y++) {{ const l = b.getLine(y); if (l) screen += l.translateToString(true); }}
    return {{
      id: w._deskId,
      session: (w._term && w._term.sessionId) ?? null,
      open: !!(w._term && w._term.ws && w._term.ws.readyState === 1),
      placeholder: w.classList.contains('placeholder'),
      hold: strip ? strip.querySelector('.session-peer-down-text').textContent : null,
      button: btn && !btn.hidden ? btn.textContent : null,
      detail: det && !det.hidden ? det.querySelector('p').textContent : null,
      detailOpen: det ? det.open : null,
      screen,
    }};
  }})
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


def main():
    os.makedirs(base.SHOT_DIR, exist_ok=True)
    base.build()
    for port in (base.LOCAL_PORT, base.PEER_PORT):
        if base.listening(port, 0.5):
            print(f"[FAIL] port {port} answers a FOREIGN listener", flush=True)
            sys.exit(1)
    local_store = Path(tempfile.mkdtemp(prefix="wb_hold_local_"))
    peer_store = Path(tempfile.mkdtemp(prefix="wb_hold_peer_"))
    (local_store / "daemon.toml").write_text(f'id = "{base.LOCAL_ID}"\nname = "anvil"\navatar = "🐙"\n', encoding="utf-8")
    (peer_store / "daemon.toml").write_text(f'id = "{base.PEER_ID}"\nname = "far-box"\navatar = "🐺"\n', encoding="utf-8")
    subprocess.run(
        [str(base.PEER_EXE), "daemon", "add", str(base.seed_repo())],
        env=base.env_for(peer_store),
        check=True,
        capture_output=True,
    )
    proxy_port = base.free_port()
    (local_store / "peers").mkdir()
    (local_store / "peers" / f"{base.PEER_ID}.toml").write_text(
        "\n".join(
            [
                f'daemon_id = "{base.PEER_ID}"',
                'name = "far-box"',
                'avatar = "🐺"',
                'address = "127.0.0.1"',
                f"port = {proxy_port}",
                'environment = "macOS 12"',
                f'token = "{base.TOKEN}"',
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
                [str(base.PEER_EXE), "daemon", "--port", str(base.PEER_PORT)],
                env=base.env_for(peer_store, base.TOKEN),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        )
        procs.append(
            subprocess.Popen(
                [str(base.EXE), "daemon", "--port", str(base.LOCAL_PORT)],
                env=base.env_for(local_store),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        )
        if not (base.listening(base.PEER_PORT, 30) and base.listening(base.LOCAL_PORT, 30)):
            print("[FAIL] a daemon did not start", flush=True)
            sys.exit(1)
        proxy = base.Proxy(proxy_port)

        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True, args=["--disable-webgl", "--disable-gpu"])
            page = browser.new_page(viewport={"width": 1500, "height": 950})
            thrown = []
            page.on("pageerror", lambda e: thrown.append(str(e)))
            sockets = []
            page.on("websocket", lambda ws: sockets.append(ws.url) if "/ws/session" in ws.url else None)
            if os.environ.get("RALPHY_TEST_DIRECT") != "1":

                def via_proxy(ws):
                    server = ws.connect_to_server()
                    server.on_close(lambda code, reason: ws.close(code=1000, reason="proxy"))
                page.route_web_socket(re.compile(r"/ws/session"), via_proxy)
            page.goto(f"http://127.0.0.1:{base.LOCAL_PORT}/", wait_until="domcontentloaded")
            page.wait_for_function("() => !!window.WBConsole", timeout=10000)
            page.wait_for_function(
                f"() => {SH}.fleetPeers.some(p => p.daemon_id === '{base.PEER_ID}' && p.state === 'reachable')",
                timeout=30000,
            )

            # --- scenario 1 ----------------------------------------------------
            for _ in range(N):
                page.evaluate(f"() => window.WBConsole.open({{ repo: '{REF}', plain: true }})")
                page.wait_for_timeout(300)
            wins = wait_for(page, lambda ws: len(ws) == N and all(w["session"] for w in ws), 30)
            owned = {w["id"]: w["session"] for w in wins}
            check(f"{N} consoles run on the peer", len(owned) == N and all(owned.values()), f"wins={owned}")

            # --- scenario 2 ----------------------------------------------------
            # The peer's own handshake names its environment, not the descriptor.
            env = page.evaluate(f"() => {SH}.fleetPeers.find(p => p.daemon_id === '{base.PEER_ID}').environment")
            proxy.down()
            wins = wait_for(page, lambda ws: len(ws) == N and all(w["hold"] for w in ws), 25)
            check(
                "the link down, each window names the peer's state",
                len(wins) == N and all(w["hold"] == f"Ralphy is not running on {env}." for w in wins),
                f"holds={[w['hold'] for w in wins]}",
            )
            check(
                "…offers Try again, with the diagnosis behind a closed Details",
                all(w["button"] == "Try again" and w["detail"] and w["detailOpen"] is False for w in wins),
                f"wins={[(w['button'], w['detail'], w['detailOpen']) for w in wins]}",
            )
            check(
                "…and stays a console with its text, not a placeholder",
                all(not w["placeholder"] and w["session"] == owned[w["id"]] for w in wins),
                f"wins={[(w['placeholder'], w['session']) for w in wins]}",
            )
            page.screenshot(path=str(base.SHOT_DIR / "console-peer-hold.png"))

            # --- scenario 3 ----------------------------------------------------
            before = len(sockets)
            page.wait_for_timeout(HOLD_S * 1000)
            check(
                f"for {HOLD_S} s with the peer down, no window opens a socket",
                len(sockets) == before,
                f"sockets={len(sockets) - before}",
            )
            wins = windows(page)
            check(
                "…none gives up, and each still holds its session",
                len(wins) == N
                and all("[session closed]" not in w["screen"] and w["hold"] and w["session"] == owned[w["id"]] for w in wins),
                f"wins={[(w['session'], w['hold'], w['screen'][-60:]) for w in wins]}",
            )

            # --- scenario 4 ----------------------------------------------------
            before = len(sockets)
            proxy.up()
            base.fleet_read(page)
            wins = wait_for(
                page,
                lambda ws: {w["id"]: w["session"] for w in ws} == owned and all(w["open"] and not w["hold"] for w in ws),
                25,
            )
            reopened = sockets[before:]
            check(
                "the link back, every window attaches to its own session",
                {w["id"]: w["session"] for w in wins} == owned and all(w["open"] and not w["hold"] for w in wins),
                f"wins={[(w['id'], w['session'], w['open'], w['hold']) for w in wins]}",
            )
            check(
                "…once each, and nothing is launched",
                len(reopened) == N and not any("console=1" in u for u in reopened),
                f"sockets={reopened}",
            )

            browser.close()
            # --- scenario 5 ----------------------------------------------------
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
    ok = all(results) and len(results) == 9
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
