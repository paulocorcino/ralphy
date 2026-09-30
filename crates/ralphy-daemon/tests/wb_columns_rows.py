"""Rows inside a column (ADR-0051 §5 and §8, rows amendment) browser acceptance:
"Slice" opens a console to the Right, as a new column, or Down, as a
new row of the caller's column.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7474. The fixture and
the daemon helpers are `wb_columns_473.py`'s:
  w-a  loose, live shell, maximized
  w-b  loose, live shell
  w-c  loose, placeholder
  w-f  placeholder inside fence f-one
  w-l  placeholder inside fence f-lock

D1  the title bar button is "Slice", and its list starts with Right
    and Down, Right chosen
D2  Down opens w-b below w-a: one column, two rows of equal height that fill
    the viewport
D3  Right from the lower row opens w-c as a new column of one row
D4  Alt+Shift+↓ walks the rows and wraps; Alt+Shift+→ from a lower row lands
    on the last row of a shorter column; Alt+Shift+← goes back to the same row
D5  the swap icon moves a console between rows of two columns
D6  restore of a lower row lets the row above fill the column
D7  restore of the first console moves the desk's maximize to the next console
    in reading order
D8  after a reload the grid and the Down choice come back
D9  a flat list stored before rows reads as one row per column, and is
    written back as a grid
D10 a phone width paints only the first console; wider brings the rows back
D11 nothing about rows reaches the desk, and only one console is maximized

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python crates/ralphy-daemon/tests/wb_columns_rows.py   (exit 0 = all pass)
"""

import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7474
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SH = T.SH
SHOT = os.path.join(T.REPO_ROOT, "docs", "screenshots", "rows-in-columns-2026-09-29.png")
SHOT_MENU = os.path.join(T.REPO_ROOT, "docs", "screenshots", "rows-in-columns-menu-2026-09-29.png")
VIEW = {"width": 2000, "height": 1000}
PHONE = {"width": 480, "height": 1000}
FLOOR = 32  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


BOXES = """() => {
  const ws = document.getElementById('workspace');
  const o = ws.getBoundingClientRect();
  const out = { ws: { w: ws.clientWidth, h: ws.clientHeight } };
  for (const w of document.querySelectorAll('#stage .session-window.column, #stage .session-window.maximized')) {
    const r = w.getBoundingClientRect();
    out[w._deskId] = { left: r.left - o.left, top: r.top - o.top, width: r.width, height: r.height,
                       max: w.classList.contains('maximized'), column: w.classList.contains('column') };
  }
  return out;
}"""


def grid(page):
    return page.evaluate(f"() => JSON.parse(JSON.stringify({SH}.columns))")


def boxes(page):
    return page.evaluate(BOXES)


def near(a, b, tol=2):
    return abs(a - b) <= tol


def pick_dir(page, d):
    page.locator(f".column-menu .column-dir-btn[data-dir='{d}']").click()
    page.wait_for_timeout(100)


def add(page, from_id, d, id):
    T.open_menu(page, from_id)
    pick_dir(page, d)
    page.locator(f".column-menu .column-item[data-id='{id}']").click()
    page.wait_for_timeout(500)


def swap(page, from_id, id):
    T.open_menu(page, from_id)
    page.locator(f".column-menu .column-swap[data-id='{id}']").click()
    page.wait_for_timeout(500)


def restore(page, id):
    T.press_max(page, id)


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbrows_daemon_")
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

            # D1 -------------------------------------------------------------
            title = page.evaluate("() => __colBtn('w-a')?.title")
            check("D1 the title bar button is 'Slice'", title == "Slice", str(title))
            T.open_menu(page, "w-a")
            d1 = page.evaluate(
                "() => { const m = document.querySelector('.column-menu');"
                " const b = [...m.querySelectorAll('.column-dir-btn')];"
                " return { first: m.firstElementChild.className, labels: b.map((x) => x.textContent.trim()),"
                "   pressed: b.map((x) => x.getAttribute('aria-pressed')) }; }"
            )
            check("D1 the list starts with the direction choice", d1["first"] == "column-dir", str(d1))
            check("D1 …Right and Down", d1["labels"] == ["Right", "Down"], str(d1))
            check("D1 …Right chosen when nothing is stored", d1["pressed"] == ["true", "false"], str(d1))
            os.makedirs(os.path.dirname(SHOT_MENU), exist_ok=True)
            page.screenshot(path=SHOT_MENU)
            page.keyboard.press("Escape")
            page.evaluate(f"() => {{ {SH}.columnMenu = false; }}")
            page.wait_for_timeout(150)

            # D2 -------------------------------------------------------------
            add(page, "w-a", "down", "w-b")
            check("D2 Down makes one column of two rows", grid(page) == [["w-a", "w-b"]], str(grid(page)))
            b = boxes(page)
            W, H = b["ws"]["w"], b["ws"]["h"]
            a_, b_ = b.get("w-a"), b.get("w-b")
            ok = a_ and b_ and near(a_["left"], 0) and near(b_["left"], 0) and near(a_["width"], W) and near(b_["width"], W)
            check("D2 both rows span the full width", ok, str(b))
            ok = a_ and b_ and near(a_["top"], 0) and near(a_["height"], H / 2) and near(b_["top"], H / 2) and near(b_["height"], H / 2)
            check("D2 …with equal heights that fill the viewport", ok, str(b))
            check("D2 only the top row is maximized", a_ and a_["max"] and b_ and not b_["max"], str(b))
            dir_now = page.evaluate(f"() => {SH}.columnDir")
            check("D2 the choice is kept", dir_now == "down", str(dir_now))

            # D3 -------------------------------------------------------------
            add(page, "w-b", "right", "w-c")
            check("D3 Right from a lower row opens a new column",
                  grid(page) == [["w-a", "w-b"], ["w-c"]], str(grid(page)))
            b = boxes(page)
            c_ = b.get("w-c")
            ok = c_ and near(c_["left"], W / 2) and near(c_["width"], W / 2) and near(c_["top"], 0) and near(c_["height"], H)
            check("D3 the new column is one full-height row on the right half", ok, str(c_))
            ok = near(b["w-a"]["width"], W / 2) and near(b["w-b"]["width"], W / 2) and near(b["w-b"]["top"], H / 2)
            check("D3 the two rows narrow to the left half", ok, str(b))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # D4 -------------------------------------------------------------
            T.click_centre(page, "w-a")
            walk = []
            for key in ["ArrowDown", "ArrowDown", "ArrowDown", "ArrowRight", "ArrowLeft", "ArrowUp"]:
                page.keyboard.press(f"Alt+Shift+{key}")
                page.wait_for_timeout(250)
                walk.append(page.evaluate("() => __focused()[0] ?? null"))
            check("D4 ↓ walks the rows and wraps; → clamps to the last row; ← keeps the row",
                  walk == ["w-b", "w-a", "w-b", "w-c", "w-a", "w-b"], str(walk))
            screen = page.evaluate(
                "() => { const b = __W('w-a')._term.term.buffer.active; let t = '';"
                " for (let i = 0; i < b.length; i++) t += b.getLine(i)?.translateToString(true) + '\\n'; return t; }"
            )
            check("D4 the chord never reached the shell", "[1;4" not in screen and "\x1b" not in screen)

            # D5 -------------------------------------------------------------
            swap(page, "w-c", "w-b")
            check("D5 the swap moves consoles between rows of two columns",
                  grid(page) == [["w-a", "w-c"], ["w-b"]], str(grid(page)))

            # D6 -------------------------------------------------------------
            restore(page, "w-c")
            check("D6 restore of a lower row removes only that row", grid(page) == [["w-a"], ["w-b"]], str(grid(page)))
            a_ = boxes(page).get("w-a")
            check("D6 …and the row above fills the column", a_ and near(a_["top"], 0) and near(a_["height"], H), str(a_))

            # D7 -------------------------------------------------------------
            add(page, "w-a", "down", "w-c")
            check("D7 setup", grid(page) == [["w-a", "w-c"], ["w-b"]], str(grid(page)))
            restore(page, "w-a")
            check("D7 restore of the first console", grid(page) == [["w-c"], ["w-b"]], str(grid(page)))
            got = T.poll_desk(lambda d: d.get("w-c", {}).get("max") and not d.get("w-a", {}).get("max"))
            maxed = sorted(i for i, r in (got or {}).items() if r.get("max"))
            check("D7 …moves the desk's maximize to the next in reading order (the row below)",
                  maxed == ["w-c"], str(maxed))

            # D8 -------------------------------------------------------------
            add(page, "w-c", "down", "w-a")
            before = grid(page)
            check("D8 setup", before == [["w-c", "w-a"], ["w-b"]], str(before))
            T.reload(page)
            try:
                page.wait_for_function(f"() => {SH}.columns.flat().length === 3", timeout=8000)
            except Exception:
                pass
            check("D8 after a reload the grid comes back", grid(page) == before, str(grid(page)))
            check("D8 …with the Down choice", page.evaluate(f"() => {SH}.columnDir") == "down")
            b = boxes(page)
            ok = near(b.get("w-a", {}).get("top", -1), H / 2) and near(b.get("w-a", {}).get("left", -1), 0)
            check("D8 …painted as rows", ok, str(b))

            # D10 ------------------------------------------------------------
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(700)
            d10 = page.evaluate(
                "() => ({ columns: document.querySelectorAll('.session-window.column').length,"
                " max: [...document.querySelectorAll('.session-window.maximized')].map((w) => w._deskId) })"
            )
            check("D10 a phone width paints only the first console", d10 == {"columns": 0, "max": ["w-c"]}, str(d10))
            check("D10 …and keeps the grid", grid(page) == before, str(grid(page)))
            page.set_viewport_size(dict(VIEW))
            page.wait_for_timeout(700)
            b = boxes(page)
            ok = all(b.get(i, {}).get("column") for i in ("w-a", "w-b", "w-c")) and near(b["w-a"]["top"], H / 2)
            check("D10 wider again: the rows come back", ok, str(b))

            # D11 ------------------------------------------------------------
            page.wait_for_timeout(1500)
            raw = T.desk_raw()
            check("D11 nothing about columns or rows reaches the desk",
                  "column" not in json.dumps(raw) and "rows" not in json.dumps(raw), json.dumps(raw)[:200])
            maxed = [w["id"] for w in raw["windows"] if w.get("max")]
            check("D11 only the first console is maximized in the desk", maxed == ["w-c"], str(maxed))

            # D9 -------------------------------------------------------------
            page.evaluate("() => WBView.patch({ columns: ['w-c', 'w-gone', 'w-b'] })")
            T.reload(page)
            try:
                page.wait_for_function(f"() => {SH}.columns.flat().length === 2", timeout=8000)
            except Exception:
                pass
            check("D9 a flat stored list reads as one row per column",
                  grid(page) == [["w-c"], ["w-b"]], str(grid(page)))
            st = page.evaluate("() => WBView.read()?.columns ?? null")
            check("D9 …and is written back as a grid", st == [["w-c"], ["w-b"]], str(st))

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
