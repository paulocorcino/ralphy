"""A console sleeps while the Consoles tab is hidden (#738 item 3) browser
acceptance.

A file tab hides the Consoles tab with `visibility: hidden`, so the observer
still reports every console as visible. Before this check, no console slept
while a file tab was shown, and each one kept its `/ws/session` socket open.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7738. The daemon and
fixture helpers are `wb_columns_473.py`'s. The desk holds 2 plain consoles,
side by side, both inside the viewport.

G1  with a file tab shown, after the dormant delay + 5 s, both console sockets are closed
G2  back on the Consoles tab, each console opens a new `/ws/session` within 10 s
G3  each console shows again the marker line typed into it before the tab was hidden

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_tab_hidden_738.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7738
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
VIEW = {"width": 1600, "height": 900}
IDS = ["w-a", "w-b"]
# `DORMANT_AFTER_MS` in wb-console-gpu.ts.
DORMANT_AFTER_MS = 15000
TABS = "Alpine.store('tabs')"

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"{'PASS' if ok else 'FAIL'} {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    out = ""
    for i, wid in enumerate(IDS):
        rect = {"left": 40 + i * 700, "top": 40, "width": 640, "height": 400}
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f'consoleName = "c{i}"\n'
            f"ts = {100 + i}\n"
            f"{T.rect_toml(rect)}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


BUFFER = """(id) => { const b = __W(id)?._term?.term?.buffer.active; if (!b) return '';
  const out = []; for (let y = 0; y < b.length; y++) out.push(b.getLine(y)?.translateToString(true) ?? '');
  return out.join('\\n'); }"""


def pump(page, ms):
    """Playwright delivers socket events only while the page is being waited on."""
    deadline = time.time() + ms / 1000
    while time.time() < deadline:
        page.wait_for_timeout(100)


def wait_until(page, predicate, ms):
    deadline = time.time() + ms / 1000
    while time.time() < deadline:
        if predicate():
            return True
        page.wait_for_timeout(100)
    return predicate()


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wb738_daemon_")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    write_desk(daemon_dir, slug)
    proc = T.launch(daemon_dir)
    errors = []
    # Every `/ws/session` socket the page opened: [url, closed].
    sockets = []

    def on_socket(ws):
        if "/ws/session" not in ws.url:
            return
        rec = [ws.url, False]
        sockets.append(rec)
        ws.on("close", lambda _ws: rec.__setitem__(1, True))

    try:
        if not T.wait_listening(BASE):
            check("G0 daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(args=["--disable-gpu"])
            page = browser.new_page(viewport=dict(VIEW))
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("websocket", on_socket)
            page.goto(BASE)
            T.boot(page, want=len(IDS))
            page.wait_for_function(
                "(ids) => ids.every((id) => __W(id)?._term?.term?.cols > 0 && __W(id)._term.sessionId != null)",
                arg=IDS,
                timeout=30000,
            )
            markers = {}
            for i, wid in enumerate(IDS):
                markers[wid] = f"TABHIDE{i}MARK"
                page.evaluate("(id) => __W(id).querySelector('.xterm-helper-textarea').focus()", wid)
                page.keyboard.type(f"echo {markers[wid]}")
                page.keyboard.press("Enter")
                wait_until(page, lambda w=wid: markers[w] in page.evaluate(BUFFER, w), 10000)

            # G1 ---------------------------------------------------------------
            page.evaluate(f"() => {{ Alpine.store('projects').setOpen('{slug}'); }}")
            page.wait_for_timeout(200)
            page.evaluate(
                f"() => {TABS}.openTab({{ project: '{slug}', path: 'README.md', title: 'README.md', ftype: 'markdown' }})"
            )
            page.wait_for_function(f"() => {TABS}.active !== 'consoles'", timeout=10000)
            live = [s for s in sockets if not s[1]]
            pump(page, DORMANT_AFTER_MS + 5000)
            still = [s[0] for s in live if not s[1]]
            asleep = page.evaluate("(ids) => ids.map((id) => !!__W(id)?._dormant)", IDS)
            check(
                "G1 both console sockets close while a file tab is shown",
                len(live) >= len(IDS) and not still and all(asleep),
                f"open-before={len(live)} still-open={still} dormant={asleep}",
            )

            # G2 ---------------------------------------------------------------
            known = len(sockets)
            page.evaluate(f"() => {TABS}.activate('consoles')")
            fresh = wait_until(page, lambda: len([s for s in sockets[known:] if not s[1]]) >= len(IDS), 10000)
            check(
                "G2 both consoles reconnect within 10 s",
                fresh,
                f"new={[s[0] for s in sockets[known:]]}",
            )

            # G3 ---------------------------------------------------------------
            shown = {
                wid: wait_until(page, lambda w=wid: markers[w] in page.evaluate(BUFFER, w), 10000) for wid in IDS
            }
            check("G3 each console shows its marker line again", all(shown.values()), str(shown))
            if errors:
                print(f"page errors: {errors[:3]}", flush=True)
            browser.close()
    finally:
        T.stop(proc)


if __name__ == "__main__":
    main()
    print(f"\n{sum(results)}/{len(results)} passed")
    sys.exit(0 if len(results) == 3 and all(results) else 1)
