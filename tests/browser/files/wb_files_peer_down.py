"""The FILES panel of a project whose peer stops answering says so once, in
words, and shows no error row inside a folder.

Two REAL daemons and the TCP proxy of `console/wb_console_peer_unanswered.py`:
its `down()` cuts every live connection, as a dead `ssh -L` does.

Scenario 1  the peer project opens and its root lists `docs` and `src`
Scenario 2  the link drops before the fleet knows, and `docs` is opened: no
            "Error" row, the folder closes, and the footer names the peer as
            not connected (the failed read asked the fleet)
Scenario 3  the line offers Try again and the daemon's diagnosis behind a
            closed Details; the raw socket text is not in the line; the rows dim
Scenario 4  while the peer is down, `src` (never read) does not open
Scenario 5  the link comes back and the fleet is read: the line goes, the rows
            are not dimmed, and `src` opens with its file
Scenario 6  zero `pageerror` events over the whole pass

Before the fix, scenario 2 drew "Error (Error: … did not answer …)" inside the
folder, and the footer printed the socket error.

Scratch stores only. Every process is stopped by its own handle, NEVER by name
(`ralphy.exe` doubles as the orchestrator on this host).

Writes .ralphy/screenshots/files-peer-down.png.
Run: python tests/browser/files/wb_files_peer_down.py   (exit 0 = all pass)
"""

import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from playwright.sync_api import sync_playwright

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "console"))
import wb_console_peer_unanswered as base  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

SH = base.SH
REF = base.REF
results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


FILES = """
() => {
  const host = document.querySelector('.files-pane .wb-host');
  const rows = host ? [...host.querySelectorAll('.wb-row')].map(r => ({
    title: (r.querySelector('.wb-title') || {}).textContent?.trim() || '',
    error: r.classList.contains('wb-error') || r.classList.contains('wb-status'),
    expanded: r.classList.contains('wb-expanded'),
  })) : [];
  const line = document.querySelector('.files-pane .files-peer-down');
  const stale = [...document.querySelectorAll('.files-pane .files-stale:not(.files-peer-down)')]
    .filter(el => el.offsetParent !== null).map(el => el.textContent.trim());
  const det = line && line.querySelector('details.session-detail');
  return {
    rows,
    dim: !!host && host.classList.contains('peer-down'),
    line: line ? line.querySelector('span').textContent : null,
    button: line ? line.querySelector('button').textContent : null,
    detail: det && det.offsetParent !== null ? det.querySelector('p').textContent : null,
    detailOpen: det ? det.open : null,
    stale,
  };
}
"""


def files(page):
    return page.evaluate(FILES)


def wait_files(page, pred, timeout):
    deadline = time.time() + timeout
    got = files(page)
    while time.time() < deadline:
        got = files(page)
        if pred(got):
            return got
        time.sleep(0.25)
    return got


def click_expander(page, title):
    page.evaluate(
        """(title) => {
          const row = [...document.querySelectorAll('.files-pane .wb-host .wb-row')]
            .find(r => r.querySelector('.wb-title')?.textContent.trim() === title);
          row.querySelector('.wb-expander').dispatchEvent(new MouseEvent('click', { bubbles: true }));
        }""",
        title,
    )


def titles(state):
    return [r["title"] for r in state["rows"]]


def main():
    os.makedirs(base.SHOT_DIR, exist_ok=True)
    base.build()
    for port in (base.LOCAL_PORT, base.PEER_PORT):
        if base.listening(port, 0.5):
            print(f"[FAIL] port {port} answers a FOREIGN listener", flush=True)
            sys.exit(1)
    local_store = Path(tempfile.mkdtemp(prefix="wb_files_down_local_"))
    peer_store = Path(tempfile.mkdtemp(prefix="wb_files_down_peer_"))
    (local_store / "daemon.toml").write_text(f'id = "{base.LOCAL_ID}"\nname = "anvil"\navatar = "🐙"\n', encoding="utf-8")
    (peer_store / "daemon.toml").write_text(f'id = "{base.PEER_ID}"\nname = "far-box"\navatar = "🐺"\n', encoding="utf-8")
    repo = base.seed_repo()
    for rel in ("src/main.txt", "docs/guide.txt"):
        (repo / rel).parent.mkdir(parents=True, exist_ok=True)
        (repo / rel).write_text("x\n", encoding="utf-8")
    subprocess.run([str(base.PEER_EXE), "daemon", "add", str(repo)], env=base.env_for(peer_store), check=True, capture_output=True)
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
            page.goto(f"http://127.0.0.1:{base.LOCAL_PORT}/", wait_until="domcontentloaded")
            page.wait_for_function("() => !!window.WBConsole", timeout=10000)
            page.wait_for_function(
                f"() => {SH}.fleetPeers.some(p => p.daemon_id === '{base.PEER_ID}' && p.state === 'reachable')",
                timeout=30000,
            )
            env = page.evaluate(f"() => {SH}.fleetPeers.find(p => p.daemon_id === '{base.PEER_ID}').environment")

            # --- scenario 1 ----------------------------------------------------
            page.evaluate(f"() => {SH}.toggle('{REF}')")
            state = wait_files(page, lambda s: {"docs", "src"} <= set(titles(s)), 20)
            check("the peer project lists docs and src", {"docs", "src"} <= set(titles(state)), f"rows={titles(state)}")

            # --- scenario 2 ----------------------------------------------------
            proxy.down()
            click_expander(page, "docs")
            state = wait_files(page, lambda s: s["line"], 20)
            check(
                "a folder that fails to load draws no error row",
                not any(r["error"] or r["title"].startswith("Error") for r in state["rows"]),
                f"rows={state['rows']}",
            )
            check(
                "…and closes",
                not any(r["title"] == "docs" and r["expanded"] for r in state["rows"]),
                f"rows={state['rows']}",
            )
            check(
                "the failed read asked the fleet: the line names the peer",
                state["line"] == f"{env} is not connected. The list shown is the last one read.",
                f"line={state['line']!r}",
            )

            # --- scenario 3 ----------------------------------------------------
            check(
                "the line offers Try again, with the diagnosis behind a closed Details",
                state["button"] == "Try again" and state["detail"] and state["detailOpen"] is False,
                f"button={state['button']!r} detail={state['detail']!r}",
            )
            check(
                "no line in the footer carries the raw socket text",
                not any("os error" in t or "127.0.0.1" in t for t in [state["line"] or ""] + state["stale"]),
                f"stale={state['stale']}",
            )
            check("the rows dim", state["dim"], f"dim={state['dim']}")
            page.screenshot(path=str(base.SHOT_DIR / "files-peer-down.png"))

            # --- scenario 4 ----------------------------------------------------
            click_expander(page, "src")
            page.wait_for_timeout(1500)
            state = files(page)
            check(
                "src, never read, does not open while the peer is down",
                not any(r["title"] == "src" and r["expanded"] for r in state["rows"]) and "main.txt" not in titles(state),
                f"rows={state['rows']}",
            )

            # --- scenario 5 ----------------------------------------------------
            proxy.up()
            base.fleet_read(page)
            state = wait_files(page, lambda s: s["line"] is None and not s["dim"], 25)
            check("the peer back, the line goes and the rows are not dimmed", state["line"] is None and not state["dim"], f"state={state}")
            click_expander(page, "src")
            state = wait_files(page, lambda s: "main.txt" in titles(s), 15)
            check("src opens with its file", "main.txt" in titles(state), f"rows={titles(state)}")

            browser.close()
            # --- scenario 6 ----------------------------------------------------
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
    ok = all(results) and len(results) == 11
    print(f"\n{sum(results)}/{len(results)} checks passed", flush=True)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
