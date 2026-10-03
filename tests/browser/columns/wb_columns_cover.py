"""A console under a full bleed sleeps (ADR-0051 §9, dormant consoles; the
covered amendment) browser acceptance.

Columns and a maximize fill the viewport. The consoles under them are still
inside it, so the IntersectionObserver calls them visible, but nobody sees
them. Each live terminal holds a WebGL context, and Chrome drops the oldest
one past ~16. So a covered console sleeps like one off the viewport, and it
wakes as soon as it is uncovered.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7476. The fixture and
the daemon helpers are `wb_columns_473.py`'s:
  w-a  loose, live shell, maximized
  w-b  loose, live shell, inside the viewport under w-a

C1  setup: w-a is maximized, w-b has a terminal and is inside the viewport;
    only the seen w-a draws with WebGL, so a restore of a large desk does not
    pass Chrome's context limit
C2  after the grace period w-b sleeps; w-a, the full bleed, does not
C3  a restore of w-a uncovers w-b, and it wakes at once with WebGL
C4  maximized again, w-b sleeps again
C5  opened as a column, w-b wakes: a column is never covered

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/columns/wb_columns_cover.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7476
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "columns-cover-2026-10-03.png")
# Tall enough that w-b (top 1100, height 300) is inside the viewport at scroll 0.
VIEW = {"width": 2400, "height": 1600}
# `DORMANT_AFTER_MS` is 15 s.
SLEEP_WAIT_MS = 20000
FLOOR = 12  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


STATE = """(id) => {
  const w = __W(id);
  const ws = document.getElementById('workspace').getBoundingClientRect();
  const r = w.getBoundingClientRect();
  return {
    dormant: w.classList.contains('dormant'),
    term: !!w._term,
    max: w.classList.contains('maximized'),
    column: w.classList.contains('column'),
    gpu: !!w.querySelector('.xterm-screen canvas'),
    inside: r.right > ws.left && r.left < ws.right && r.bottom > ws.top && r.top < ws.bottom,
  };
}"""


def state(page, id):
    return page.evaluate(STATE, id)


def wait_for(page, js, timeout):
    try:
        page.wait_for_function(js, timeout=timeout)
        return True
    except Exception:
        return False


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbcover_daemon_")
    fixture = T.make_fixture_repo()
    slug = T.register_fixture(daemon_dir, fixture)
    T.write_fixture_desk(daemon_dir, slug)
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
            T.boot(page)

            # C1 -------------------------------------------------------------
            a, b = state(page, "w-a"), state(page, "w-b")
            check("C1 w-a is maximized", a["max"], str(a))
            check("C1 w-b has a terminal", b["term"] and not b["dormant"], str(b))
            check("C1 w-b is inside the viewport", b["inside"], str(b))
            check("C1 the seen w-a draws with WebGL", a["gpu"], str(a))
            check("C1 the covered w-b does not", not b["gpu"], str(b))

            # C2 -------------------------------------------------------------
            slept = wait_for(page, "() => __W('w-b').classList.contains('dormant')", SLEEP_WAIT_MS)
            b = state(page, "w-b")
            check("C2 w-b under the maximize sleeps", slept and not b["term"], str(b))
            a = state(page, "w-a")
            check("C2 the maximized w-a keeps its terminal", a["term"] and not a["dormant"], str(a))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # C3 -------------------------------------------------------------
            T.press_max(page, "w-a")
            woke = wait_for(page, "() => !!__W('w-b')._term", 2000)
            b = state(page, "w-b")
            check("C3 restore uncovers w-b and it wakes at once", woke and not b["dormant"], str(b))
            gpu = wait_for(page, "() => !!__W('w-b').querySelector('.xterm-screen canvas')", 2000)
            check("C3 …and draws with WebGL", gpu, str(state(page, "w-b")))

            # C4 -------------------------------------------------------------
            T.press_max(page, "w-a")
            slept = wait_for(page, "() => __W('w-b').classList.contains('dormant')", SLEEP_WAIT_MS)
            check("C4 maximized again, w-b sleeps again", slept, str(state(page, "w-b")))

            # C5 -------------------------------------------------------------
            T.open_column(page, "w-a", "w-b")
            woke = wait_for(page, "() => !!__W('w-b')._term", 2000)
            b = state(page, "w-b")
            check("C5 opened as a column, w-b wakes", woke and b["column"] and not b["dormant"], str(b))

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
