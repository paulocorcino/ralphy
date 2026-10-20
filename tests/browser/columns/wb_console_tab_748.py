"""A console in its own tab (#748, ADR-0037 §3d) browser acceptance.

The + at the end of the tab strip lists the consoles on the stage. A pick
opens that console in its own tab: the window moves from the stage into the
tab's pane and fills it. The tab belongs to this device and writes nothing to
the desk; the × puts the window back on the stage at its rect.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7748. The fixture and
the daemon helpers are `wb_columns_473.py`'s (w-a is maximized on the desk;
w-a and w-b are live shells).

T1  the + lists the consoles of the stage: the column list's rows and the
    console that opened that list
T2  a pick opens one active tab; w-b is in the tab's pane, not on the stage,
    fills the pane and shows no maximize, Slice or close; the desk is unchanged
T3  typing in the tab reaches the shell, and the PTY takes the pane's width
T4  on Consoles w-b is not on the stage; a second pick activates the same tab
T5  the case of the report: a Slice on Consoles while w-b has a tab; going
    between the tabs many times keeps each tab as it was; the column list
    names w-b as in its own tab
T6  another device sees no tab, and w-b on its stage
T7  after a reload the tab is back and active, with w-b in it, and the grid
    of Consoles is back
T8  at a phone width w-b fills the tab
T9  a tab for the grid's first console takes it out of the grid
T10 the × returns each window to the stage at its rect; it keeps running, and
    the desk keeps its rects; the maximize is the grid's (w-c, after T9)
T11 closing a console closes its tab

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/columns/wb_console_tab_748.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7748
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "748-console-tab-pane-2026-10-20.png")
WIDE = {"width": 2400, "height": 1000}
PHONE = {"width": 400, "height": 800}
FLOOR = 42  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


# `T.HELPERS` finds a window on the stage only; these find it on the page.
PAGE_HELPERS = """() => {
  window.__P = (id) => [...document.querySelectorAll('.session-window')].find((w) => w._deskId === id) || null;
  window.__where = (id) => { const w = __P(id); return !w ? null : w.closest('#stage') ? 'stage' : w.closest('#console-tabs-pane') ? 'pane' : 'other'; };
  window.__inline = (id) => { const s = __P(id).style; return [s.left, s.top, s.width, s.height].join(' '); };
}"""
TAB_IDS = f"() => {T.TABS}.tabs.map((t) => t.id)"
ACTIVE = f"() => {T.TABS}.active"
PLUS_ROWS = "() => [...document.querySelectorAll('.console-tab-menu .console-tab-item')].map((b) => b.dataset.id).sort()"
COLUMN_ROWS = "() => [...document.querySelectorAll('.column-menu .column-item')].map((b) => b.dataset.id).sort()"
FILLS = """(id) => { const w = __P(id).getBoundingClientRect(), p = document.getElementById('console-tabs-pane').getBoundingClientRect();
  return Math.abs(w.width - p.width) < 2 && Math.abs(w.height - p.height) < 2 && Math.abs(w.left - p.left) < 2 && Math.abs(w.top - p.top) < 2; }"""
CENTER = """() => { const r = document.querySelector('.tabbody').getBoundingClientRect();
  return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2)?.closest('.session-window')?._deskId ?? null; }"""
SHOWN_BUTTONS = """(id) => [...__P(id).querySelectorAll('.session-max, .session-column, .session-close, .session-lock')]
  .filter((b) => b.getClientRects().length > 0).map((b) => b.className)"""
TEXT = "(id) => { const b = __P(id)._term.term.buffer.active; let s = ''; for (let i = 0; i < b.length; i++) s += b.getLine(i)?.translateToString(true) + '\\n'; return s; }"
COLS_OF = "(id) => __P(id)._term?.term?.cols ?? 0"


def open_plus(page):
    page.locator(".tab-add").click()
    page.wait_for_selector(".console-tab-menu .console-tab-item", state="visible", timeout=5000)
    page.wait_for_timeout(150)


def pick(page, id):
    open_plus(page)
    page.locator(f".console-tab-menu .console-tab-item[data-id='{id}']").click()
    page.wait_for_timeout(600)


def activate(page, id):
    page.evaluate(f"(id) => {T.TABS}.activate(id)", id)
    page.wait_for_timeout(400)


def helpers(page):
    page.evaluate(T.HELPERS)
    page.evaluate(PAGE_HELPERS)


def boot_here(page, want=5):
    """`T.boot` without its activate('consoles'): the restored tab must stay."""
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function(
        "(n) => document.querySelectorAll('.session-window').length === n", arg=want, timeout=20000
    )
    helpers(page)
    page.wait_for_timeout(1500)


def desk_state():
    return {k: (v.get("rect"), bool(v.get("max"))) for k, v in T.desk_windows().items()}


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbtab_daemon_")
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
            ctx = browser.new_context(viewport=dict(WIDE))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page)
            helpers(page)
            page.wait_for_timeout(1500)
            desk_before = desk_state()
            b_rect = page.evaluate("() => __inline('w-b')")

            # T1 -------------------------------------------------------------
            T.open_menu(page, "w-a")
            column_rows = page.evaluate(COLUMN_ROWS)
            page.evaluate(f"() => {{ {T.COLS}.columnMenu = false; }}")
            open_plus(page)
            plus_rows = page.evaluate(PLUS_ROWS)
            check("T1 the + lists the column list's rows and w-a",
                  plus_rows == sorted(column_rows + ["w-a"]), f"plus={plus_rows} column={column_rows}")
            page.keyboard.press("Escape")
            page.mouse.click(5, 500)
            page.wait_for_timeout(200)

            # T2 -------------------------------------------------------------
            pick(page, "w-b")
            check("T2 one console tab opened", page.evaluate(TAB_IDS).count("console:w-b") == 1,
                  str(page.evaluate(TAB_IDS)))
            check("T2 …and it is active", page.evaluate(ACTIVE) == "console:w-b", page.evaluate(ACTIVE))
            check("T2 w-b is in the tab's pane, not on the stage", page.evaluate("() => __where('w-b')") == "pane",
                  str(page.evaluate("() => __where('w-b')")))
            check("T2 w-b fills the pane", page.evaluate(FILLS, "w-b"))
            check("T2 w-b is what the tab shows", page.evaluate(CENTER) == "w-b", str(page.evaluate(CENTER)))
            shown = page.evaluate(SHOWN_BUTTONS, "w-b")
            check("T2 no maximize, Slice, lock or close in the tab", shown == [], str(shown))
            check("T2 the stage is hidden", page.evaluate(
                "() => document.querySelector('.consoles-tab').classList.contains('tab-hidden')"))
            check("T2 w-b keeps its stage rect", page.evaluate("() => __inline('w-b')") == b_rect,
                  page.evaluate("() => __inline('w-b')"))
            page.wait_for_timeout(1500)
            check("T2 the desk is unchanged", desk_state() == desk_before, str(desk_state()))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # T3 -------------------------------------------------------------
            # The shell echoes what it reads: the text in the buffer is the
            # proof that the keys reached the PTY.
            page.evaluate("() => __P('w-b')._term.term.focus()")
            page.keyboard.type("echo tab748")
            page.keyboard.press("Enter")
            try:
                page.wait_for_function("(id) => /tab748/.test((" + TEXT + ")(id))", arg="w-b", timeout=8000)
            except Exception:
                pass
            text = page.evaluate(TEXT, "w-b")
            check("T3 typing in the tab reaches the shell", "tab748" in text, repr(text.strip()[-200:]))
            check("T3 the PTY takes the pane's width", page.evaluate(COLS_OF, "w-b") > 150,
                  str(page.evaluate(COLS_OF, "w-b")))

            # T4 -------------------------------------------------------------
            activate(page, "consoles")
            check("T4 on Consoles w-b stays in the hidden pane, not on the stage",
                  page.evaluate("() => __where('w-b')") == "pane")
            check("T4 the stage shows", page.evaluate(
                "() => !document.querySelector('.consoles-tab').classList.contains('tab-hidden')"))
            pick(page, "w-b")
            check("T4 a second pick activates the same tab",
                  page.evaluate(TAB_IDS).count("console:w-b") == 1 and page.evaluate(ACTIVE) == "console:w-b",
                  str(page.evaluate(TAB_IDS)))

            # T5 -------------------------------------------------------------
            activate(page, "consoles")
            T.open_column(page, "w-a", "w-c")
            check("T5 setup: a Slice on Consoles", T.shell_cols(page) == ["w-a", "w-c"], str(T.shell_cols(page)))
            T.open_menu(page, "w-a")
            reason = page.evaluate(
                "() => document.querySelector(\".column-menu .column-item[data-id='w-b']\")?.title ?? null")
            disabled = page.evaluate(
                "() => document.querySelector(\".column-menu .column-item[data-id='w-b']\")?.disabled ?? null")
            check("T5 the column list names w-b as in its own tab", disabled is True and reason == "In its own tab",
                  f"{disabled} {reason}")
            page.evaluate(f"() => {{ {T.COLS}.columnMenu = false; }}")
            stage_rects = page.evaluate(
                "() => [...document.querySelectorAll('#stage .session-window')].map((w) => w._deskId + ' ' + __inline(w._deskId)).sort()")
            for _ in range(4):
                activate(page, "console:w-b")
                activate(page, "consoles")
            check("T5 after many switches the grid is the same", T.shell_cols(page) == ["w-a", "w-c"],
                  str(T.shell_cols(page)))
            check("T5 …and painted", sorted(page.evaluate(
                "() => [...document.querySelectorAll('#stage .session-window.column')].map((w) => w._deskId)")) == ["w-a", "w-c"])
            check("T5 …and the stage rects are the same", page.evaluate(
                "() => [...document.querySelectorAll('#stage .session-window')].map((w) => w._deskId + ' ' + __inline(w._deskId)).sort()")
                == stage_rects)
            activate(page, "console:w-b")
            check("T5 the tab still shows w-b, filling it", page.evaluate(CENTER) == "w-b" and page.evaluate(FILLS, "w-b"),
                  str(page.evaluate(CENTER)))

            # T6 -------------------------------------------------------------
            ctx_b = browser.new_context(viewport=dict(WIDE))
            page_b = ctx_b.new_page()
            page_b.goto(BASE)
            T.boot(page_b)
            helpers(page_b)
            check("T6 another device sees no console tab",
                  not [t for t in page_b.evaluate(TAB_IDS) if t.startswith("console:")], str(page_b.evaluate(TAB_IDS)))
            check("T6 …and w-b on its stage at its rect",
                  page_b.evaluate("() => __where('w-b')") == "stage" and page_b.evaluate("() => __inline('w-b')") == b_rect)
            ctx_b.close()

            # T7 -------------------------------------------------------------
            page.reload()
            boot_here(page)
            check("T7 after a reload the tab is back and active", page.evaluate(ACTIVE) == "console:w-b",
                  f"{page.evaluate(ACTIVE)} {page.evaluate(TAB_IDS)}")
            check("T7 w-b is in the tab", page.evaluate("() => __where('w-b')") == "pane")
            check("T7 …and fills it", page.evaluate(FILLS, "w-b"))
            check("T7 the grid of Consoles is back", T.shell_cols(page) == ["w-a", "w-c"], str(T.shell_cols(page)))

            # T8 -------------------------------------------------------------
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(800)
            check("T8 at a phone width w-b fills the tab", page.evaluate(FILLS, "w-b"))
            page.set_viewport_size(dict(WIDE))
            page.wait_for_timeout(800)

            # T9 -------------------------------------------------------------
            pick(page, "w-a")
            check("T9 w-a is in its tab", page.evaluate("() => __where('w-a')") == "pane")
            check("T9 …and out of the grid", "w-a" not in T.shell_cols(page), str(T.shell_cols(page)))
            check("T9 the tab shows w-a", page.evaluate(CENTER) == "w-a", str(page.evaluate(CENTER)))
            activate(page, "console:w-b")
            check("T9 the other tab shows w-b", page.evaluate(CENTER) == "w-b", str(page.evaluate(CENTER)))

            # T10 ------------------------------------------------------------
            page.locator(".tab.active .tab-close").click()
            page.wait_for_timeout(600)
            check("T10 the tab closed", "console:w-b" not in page.evaluate(TAB_IDS), str(page.evaluate(TAB_IDS)))
            check("T10 w-b is back on the stage at its rect",
                  page.evaluate("() => __where('w-b')") == "stage" and page.evaluate("() => __inline('w-b')") == b_rect,
                  f"{page.evaluate('() => __where(`w-b`)')} {page.evaluate('() => __inline(`w-b`)')}")
            check("T10 …and it is running", page.evaluate("() => !__P('w-b').classList.contains('ended')"))
            check("T10 …with no tab class", page.evaluate(
                "() => !__P('w-b').classList.contains('in-tab') && !__P('w-b').classList.contains('tab-shown')"))
            activate(page, "console:w-a")
            page.locator(".tab.active .tab-close").click()
            page.wait_for_timeout(600)
            check("T10 w-a is back on the stage", page.evaluate("() => __where('w-a')") == "stage")
            page.wait_for_timeout(1500)
            after = desk_state()
            check("T10 the desk keeps every rect",
                  {k: v[0] for k, v in after.items()} == {k: v[0] for k, v in desk_before.items()}, str(after))
            # T9 took w-a, the grid's first console, out of the grid: the grid
            # ended and its other console, w-c, kept the maximize.
            check("T10 …and the grid's maximize, nothing from the tabs", after["w-c"][1] and not after["w-a"][1]
                  and not after["w-b"][1], str(after))

            # T11 ------------------------------------------------------------
            pick(page, "w-c")
            check("T11 setup: w-c has a tab", "console:w-c" in page.evaluate(TAB_IDS))
            page.evaluate("() => __P('w-c').querySelector('.session-close').click()")
            T.confirm(page)
            try:
                page.wait_for_function(f"() => !{T.TABS}.tabs.some((t) => t.id === 'console:w-c')", timeout=8000)
            except Exception:
                pass
            check("T11 closing the console closes its tab", "console:w-c" not in page.evaluate(TAB_IDS),
                  str(page.evaluate(TAB_IDS)))

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
