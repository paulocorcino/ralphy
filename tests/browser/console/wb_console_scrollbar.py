"""A console shows its vertical scrollbar whenever it has text to scroll.

xterm 6 draws its own scrollbar at visibility Auto: the bar shows only while
the mouse is over the terminal or just after a scroll, then fades. On
2026-10-24 a console with 1051 lines of history showed no bar. The workbench
sets the bar to Visible (`showScrollbarWhileNeeded`, wb-console-terminal.ts),
which still hides a bar with nothing to scroll, such as the alternate screen
a full-screen program draws on.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7600. The daemon and
fixture helpers are `wb_columns_473.py`'s. The desk holds one plain console.

S1  right after the write the bar is shown (the scroll alone shows it,
    so this step holds with or without the fix)
S2  with the mouse away, it is still shown well after the fade time: the
    step that fails without the fix
S3  on the alternate screen (nothing to scroll) the bar is hidden; this
    guards against a fix that shows the bar always
S4  back on the normal screen, and past the fade time, the bar is shown

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_scrollbar.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7600
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
VIEW = {"width": 1600, "height": 1000}
# xterm hides an Auto bar 500 ms after the last scroll and fades it in 800 ms.
PAST_FADE_MS = 2500
FLOOR = 5

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    rect = {"left": 300, "top": 80, "width": 700, "height": 420}
    Path(daemon_dir, "desk.toml").write_bytes(
        (
            "[[windows]]\n"
            'id = "w-a"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            'consoleName = "a"\n'
            "ts = 100\n"
            f"{T.rect_toml(rect)}\n"
        ).encode("utf-8")
    )


BAR = """() => {
  const bar = __W('w-a').querySelector('.xterm-scrollable-element > .scrollbar.vertical');
  return bar && { cls: bar.className, opacity: getComputedStyle(bar).opacity };
}"""

WRITE = """(data) => new Promise((done) => __W('w-a')._term.term.write(data, done))"""


def shown(bar):
    return bool(bar) and "visible" in bar["cls"].split() and bar["opacity"] == "1"


def hidden(bar):
    return bool(bar) and "invisible" in bar["cls"].split() and bar["opacity"] == "0"


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbbar_daemon_")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    write_desk(daemon_dir, slug)
    proc = T.launch(daemon_dir)
    errors = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page, want=1)
            page.wait_for_function("() => __W('w-a')?._term?.term?.rows > 0", timeout=15000)

            # S1-S2 -------------------------------------------------------------
            lines = "".join(f"line {i}\r\n" for i in range(400))
            page.evaluate(WRITE, lines)
            # Far from the console, so only the fix can keep the bar shown.
            page.mouse.move(VIEW["width"] - 5, VIEW["height"] - 5)
            page.wait_for_timeout(300)
            check("S1 right after the write the bar is shown", shown(page.evaluate(BAR)),
                  str(page.evaluate(BAR)))
            page.wait_for_timeout(PAST_FADE_MS)
            check("S2 the bar is still shown after the fade time", shown(page.evaluate(BAR)),
                  str(page.evaluate(BAR)))

            # S3 ---------------------------------------------------------------
            page.evaluate(WRITE, "\x1b[?1049h")
            page.wait_for_timeout(300)
            check("S3 on the alternate screen the bar is hidden, not forced on", hidden(page.evaluate(BAR)),
                  str(page.evaluate(BAR)))

            # S4 ---------------------------------------------------------------
            page.evaluate(WRITE, "\x1b[?1049l")
            page.wait_for_timeout(PAST_FADE_MS)
            check("S4 back on the normal screen, past the fade time, the bar is shown", shown(page.evaluate(BAR)),
                  str(page.evaluate(BAR)))

            check("no page errors", not errors, str(errors[:3]))
            browser.close()
    finally:
        T.stop(proc)
    check(f"floor: at least {FLOOR} checks ran", len(results) >= FLOOR, str(len(results)))


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"\n{passed}/{len(results)} passed")
    sys.exit(0 if results and all(results) else 1)
