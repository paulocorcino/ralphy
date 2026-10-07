"""The page hands out a bounded number of WebGL contexts (dormant consoles,
the GPU budget) browser acceptance.

Chrome keeps 16 live WebGL contexts per renderer process and drops the oldest
past that. A desk restored as a cascade has every console inside the viewport
and uncovered, so every one of them asked for a context: on 2026-10-04 a
reload logged "Too many active WebGL contexts" four times and four consoles
lost their GPU renderer for good. Disposing the xterm WebGL addon does not
free a slot either; only an explicit `loseContext` does.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7464. The daemon and
fixture helpers are `wb_columns_473.py`'s. The desk holds 20 plain consoles
in a cascade, all inside the viewport.

G1  after the restore, no context was refused or lost
G2  exactly the budget of windows draw with WebGL ...
G3  ... and they are the windows on top
G4  a click on the bottom window gives it a context; the count stays at the budget
G5  a maximize leaves only the maximized window on the GPU
G6  a restore gives the budget back, and still no context was refused or lost
G7  a closed window's context goes to the next window in line

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/console/wb_console_gpu_budget.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7464
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
VIEW = {"width": 1920, "height": 1080}
COUNT = 20
# `GPU_BUDGET` in wb-console.ts.
BUDGET = 12
# The addon waits 3 s for a lost context to come back before it gives up.
LOSS_WAIT_MS = 3500
# The real GPU, as on the operator's machine; without these flags headless
# Chromium draws WebGL with SwiftShader.
GPU_ARGS = ["--use-angle=d3d11", "--ignore-gpu-blocklist", "--enable-gpu"] if os.name == "nt" else []
FLOOR = 10  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    out = ""
    for i in range(COUNT):
        rect = {"left": 40 + i * 30, "top": 40 + i * 25, "width": 600, "height": 360}
        out += (
            "[[windows]]\n"
            f'id = "w-{i:02d}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f'consoleName = "c{i:02d}"\n'
            f"ts = {100 + i}\n"
            f"{T.rect_toml(rect)}\n\n"
        )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


GPU = """() => [...document.querySelectorAll('#stage .session-window')].map((w) => ({
  id: w._deskId,
  z: parseInt(w.style.zIndex, 10) || 0,
  gpu: !!w.querySelector('.xterm-screen canvas'),
}))"""


def gpu_ids(page):
    return sorted(w["id"] for w in page.evaluate(GPU) if w["gpu"])


def top_ids(page, n):
    rows = sorted(page.evaluate(GPU), key=lambda w: -w["z"])
    return sorted(w["id"] for w in rows[:n])


def bottom_id(page):
    return min(page.evaluate(GPU), key=lambda w: w["z"])["id"]


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbgpu_daemon_")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    write_desk(daemon_dir, slug)
    proc = T.launch(daemon_dir)
    errors = []
    refused = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            browser = p.chromium.launch(args=GPU_ARGS)
            ctx = browser.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on(
                "console",
                lambda m: refused.append(m.text)
                if "Too many active WebGL" in m.text or "webglcontextlost" in m.text
                else None,
            )
            page.goto(BASE)
            T.boot(page, want=COUNT)
            page.wait_for_function(
                "() => [...document.querySelectorAll('#stage .session-window')]"
                ".every((w) => w._term?.term?.cols > 0)",
                timeout=30000,
            )
            page.wait_for_timeout(LOSS_WAIT_MS)

            # G1-G3 -------------------------------------------------------------
            check("G1 no context was refused or lost", not refused, str(refused[:3]))
            got = gpu_ids(page)
            check(f"G2 exactly {BUDGET} windows draw with WebGL", len(got) == BUDGET, str(len(got)))
            check("G3 they are the windows on top", got == top_ids(page, BUDGET), str(got))

            # G4 ---------------------------------------------------------------
            low = bottom_id(page)
            T.click_centre(page, low)
            page.wait_for_timeout(300)
            got = gpu_ids(page)
            check("G4 the clicked bottom window draws with WebGL", low in got, f"{low} {got}")
            check(f"G4 the count stays at {BUDGET}", len(got) == BUDGET, str(len(got)))

            # G5 ---------------------------------------------------------------
            T.press_max(page, low)
            page.wait_for_timeout(500)
            got = gpu_ids(page)
            check("G5 only the maximized window draws with WebGL", got == [low], str(got))

            # G6 ---------------------------------------------------------------
            T.press_max(page, low)
            page.wait_for_timeout(LOSS_WAIT_MS)
            got = gpu_ids(page)
            check(f"G6 a restore gives {BUDGET} contexts back", len(got) == BUDGET, str(len(got)))
            check("G6 still no context was refused or lost", not refused, str(refused[:3]))

            # G7 ---------------------------------------------------------------
            before = gpu_ids(page)
            top = top_ids(page, 1)[0]
            page.evaluate("(id) => __W(id).querySelector('.session-close').click()", top)
            T.confirm(page)
            page.wait_for_function("(id) => !__W(id)", arg=top, timeout=5000)
            page.wait_for_timeout(300)
            got = gpu_ids(page)
            check(
                f"G7 a closed window's context goes to the next one: still {BUDGET}",
                len(got) == BUDGET and top not in got and set(before) - {top} < set(got),
                f"{top} {got}",
            )

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
