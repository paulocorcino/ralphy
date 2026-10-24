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
T12 Slice in a tab (2026-10-20): the list shows over the tab; w-a opens Right
    and w-f Down, each in its box of the pane; the desk is unchanged
T13 typing in a console of the tab grid reaches its shell; Alt+Shift+arrows
    move the one focus mark inside the tab
T14 going between the tabs many times keeps the tab grid and its boxes; a
    reload brings the grid back
T15 at a phone width only the tab's first console is painted
T16 Swap puts w-l in w-f's box and w-f back on the stage; Restore takes w-a
    back to the stage at its rect
T17 the × returns every window of the grid to the stage; the desk keeps its
    rects and its maximize
T18 the shadow (2026-10-24): w-f in its tab leaves a shadow on the stage at
    its rect, with its name; fence "one" still counts it, and its columns
    button is off
T19 a drag of fence "one" moves the shadow and writes w-f's new desk rect
T20 a click on the shadow shows w-f's tab
T21 the column list of the maximized w-a shows w-a as "This console", and
    w-f under fence "one" as in its own tab
T22 the × puts w-f on the stage at its new rect, inside its fence, and the
    shadow goes

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
SHOT_GRID = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "748-console-tab-grid-2026-10-20.png")
WIDE = {"width": 2400, "height": 1000}
PHONE = {"width": 400, "height": 800}
FLOOR = 73  # every check above the floor check; pinned after the first green run
SHOT_SHADOW = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "748-console-tab-shadow-2026-10-24.png")

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
# A window's box in the pane, in px: [left, top, width, height].
BOX = """(id) => { const w = __P(id).getBoundingClientRect(), p = document.getElementById('console-tabs-pane').getBoundingClientRect();
  return [w.left - p.left, w.top - p.top, w.width, w.height].map(Math.round); }"""
PANE = "() => { const r = document.getElementById('console-tabs-pane').getBoundingClientRect(); return [Math.round(r.width), Math.round(r.height)]; }"
SHOWN_IN_TAB = "() => [...document.querySelectorAll('#console-tabs-pane .session-window.tab-shown')].map((w) => w._deskId).sort()"


def near(a, b):
    return all(abs(x - y) <= 2 for x, y in zip(a, b))


def tab_slice(page, from_id, id, swap=False):
    """The Slice list of a console in a tab, then a click on its open icon or on a row (a swap)."""
    page.evaluate("(id) => __P(id).querySelector('.session-column').click()", from_id)
    page.wait_for_function(
        "() => { const m = document.querySelector('.column-menu');"
        " return m.getClientRects().length > 0 && getComputedStyle(m).visibility === 'visible'; }",
        timeout=5000,
    )
    page.wait_for_timeout(150)
    sel = ".column-item" if swap else ".column-open"
    page.locator(f".column-menu {sel}[data-id='{id}']").click()
    page.wait_for_timeout(600)


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


SHADOW = """(id) => { const w = __P(id), s = w?._tabShadow;
  return !s || !s.isConnected ? null : { where: s.closest('#stage') ? 'stage' : 'other',
    rect: [s.style.left, s.style.top, s.style.width, s.style.height].join(' '), text: s.textContent.trim() }; }"""
SHADOWS = "() => document.querySelectorAll('#stage .console-shadow').length"
FENCE = """(id) => { const f = [...document.querySelectorAll('.fence')].find((x) => x.dataset.fenceId === id);
  return { count: f.querySelector('.fence-count').textContent, columnsOff: f.querySelector('.fence-columns').disabled }; }"""
GROUP_OF = """(id) => { const b = document.querySelector(`.column-menu .column-item[data-id='${id}']`);
  return b ? { group: b.closest('.column-group')?.querySelector('.column-group-name')?.textContent ?? null,
    off: b.disabled, tip: b.title } : null; }"""


def shadow_checks(page):
    """T18-T22: the shadow of a console in its own tab (ADR-0037 §3d, 2026-10-24)."""
    activate(page, "consoles")
    page.evaluate("() => document.getElementById('workspace').scrollTo(0, 0)")
    f_rect = page.evaluate("() => __inline('w-f')")
    # T18 -----------------------------------------------------------
    pick(page, "w-f")
    activate(page, "consoles")
    sh = page.evaluate(SHADOW, "w-f")
    check("T18 w-f in its tab leaves a shadow on the stage at its rect",
          sh is not None and sh["where"] == "stage" and sh["rect"] == f_rect, f"{sh} want {f_rect}")
    check("T18 ...one shadow, with the console name", page.evaluate(SHADOWS) == 1 and sh and "(claude)" in sh["text"],
          str(sh))
    fence = page.evaluate(FENCE, "f-one")
    check("T18 fence one still counts w-f, and its columns button is off",
          fence == {"count": "(1 console)", "columnsOff": True}, str(fence))
    page.screenshot(path=SHOT_SHADOW)
    # T19 -----------------------------------------------------------
    grab = page.evaluate(
        "() => { const f = [...document.querySelectorAll('.fence')].find((x) => x.dataset.fenceId === 'f-one');"
        " const r = f.querySelector('.fence-grab').getBoundingClientRect();"
        " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }"
    )
    page.mouse.move(grab["x"], grab["y"])
    page.mouse.down()
    page.mouse.move(grab["x"] + 60, grab["y"] + 40, steps=10)
    page.mouse.up()
    page.wait_for_timeout(600)
    want = {"left": 80 + 60, "top": 100 + 40, "width": 400, "height": 300}
    moved = page.evaluate(SHADOW, "w-f")
    check("T19 the fence drag moves the shadow", moved and moved["rect"].startswith("140px 140px"), str(moved))
    got = T.poll_desk(lambda d: d.get("w-f", {}).get("rect") == want)
    check("T19 ...and writes w-f's new desk rect", bool(got) and got["w-f"]["rect"] == want,
          str(T.desk_windows().get("w-f", {}).get("rect")))
    # T20 -----------------------------------------------------------
    page.evaluate("() => __P('w-f')._tabShadow.click()")
    page.wait_for_timeout(400)
    check("T20 a click on the shadow shows w-f's tab", page.evaluate(ACTIVE) == "console:w-f", page.evaluate(ACTIVE))
    # T21 -----------------------------------------------------------
    activate(page, "consoles")
    T.press_max(page, "w-a")
    T.open_menu(page, "w-a")
    me = page.evaluate(GROUP_OF, "w-a")
    check("T21 the column list shows w-a as This console", me and me["off"] and me["tip"] == "This console", str(me))
    f = page.evaluate(GROUP_OF, "w-f")
    check("T21 ...and w-f under fence one, as in its own tab",
          f == {"group": "one", "off": True, "tip": "In its own tab"}, str(f))
    page.evaluate(f"() => {{ {T.COLS}.columnMenu = false; }}")
    T.press_max(page, "w-a")
    # T22 -----------------------------------------------------------
    activate(page, "console:w-f")
    page.locator(".tab.active .tab-close").click()
    page.wait_for_timeout(600)
    check("T22 the x puts w-f on the stage at its new rect",
          page.evaluate("() => __where('w-f')") == "stage"
          and page.evaluate("() => __inline('w-f')") == "140px 140px 400px 300px",
          page.evaluate("() => __inline('w-f')"))
    check("T22 ...inside its fence, and the shadow is gone",
          page.evaluate(FENCE, "f-one")["count"] == "(1 console)" and page.evaluate(SHADOWS) == 0,
          f"{page.evaluate(FENCE, 'f-one')} {page.evaluate(SHADOWS)}")


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
            check("T1 the + lists the column list's rows, w-a included",
                  plus_rows == column_rows and "w-a" in plus_rows, f"plus={plus_rows} column={column_rows}")
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
            check("T2 no maximize, lock or close in the tab; Slice shows", shown == ["session-column"], str(shown))
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

            # T12 -----------------------------------------------------------
            desk_t12 = desk_state()
            a_rect = page.evaluate("() => __inline('w-a')")
            pick(page, "w-b")
            tab_slice(page, "w-b", "w-a")
            page.evaluate(f"() => {T.COLS}.setColumnDir('down')")
            tab_slice(page, "w-a", "w-f")
            page.evaluate(f"() => {T.COLS}.setColumnDir('right')")
            grid = page.evaluate(f"() => {T.TABS}.tabOf('w-b')?.grid")
            check("T12 the tab holds the grid", grid == [["w-b"], ["w-a", "w-f"]], str(grid))
            check("T12 the grid's consoles are in the pane",
                  [page.evaluate("(id) => __where(id)", i) for i in ("w-b", "w-a", "w-f")] == ["pane"] * 3)
            W, H = page.evaluate(PANE)
            boxes = {i: page.evaluate(BOX, i) for i in ("w-b", "w-a", "w-f")}
            check("T12 each console has its box", near(boxes["w-b"], [0, 0, W / 2, H])
                  and near(boxes["w-a"], [W / 2, 0, W / 2, H / 2]) and near(boxes["w-f"], [W / 2, H / 2, W / 2, H / 2]),
                  f"pane={W}x{H} {boxes}")
            check("T12 Restore and Slice show in the grid, no lock or close",
                  sorted(page.evaluate(SHOWN_BUTTONS, "w-a")) == ["session-column", "session-max"],
                  str(page.evaluate(SHOWN_BUTTONS, "w-a")))
            page.wait_for_timeout(1500)
            check("T12 the desk is unchanged", desk_state() == desk_t12, str(desk_state()))
            page.screenshot(path=SHOT_GRID)

            # T13 -----------------------------------------------------------
            page.evaluate("() => __P('w-a')._term.term.focus()")
            page.keyboard.type("echo grid748")
            page.keyboard.press("Enter")
            try:
                page.wait_for_function("(id) => /grid748/.test((" + TEXT + ")(id))", arg="w-a", timeout=8000)
            except Exception:
                pass
            check("T13 typing in w-a in the grid reaches its shell", "grid748" in page.evaluate(TEXT, "w-a"))
            page.evaluate("() => __P('w-a').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))")
            page.keyboard.press("Alt+Shift+ArrowLeft")
            page.wait_for_timeout(300)
            check("T13 Alt+Shift+← moves the focus mark to w-b, the only one",
                  page.evaluate("() => __focused()") == ["w-b"], str(page.evaluate("() => __focused()")))

            # T14 -----------------------------------------------------------
            for _ in range(3):
                activate(page, "consoles")
                activate(page, "console:w-b")
            now = {i: page.evaluate(BOX, i) for i in boxes}
            check("T14 after many switches the boxes are the same", all(near(now[i], boxes[i]) for i in boxes), str(now))
            page.reload()
            boot_here(page, want=4)
            grid = page.evaluate(f"() => {T.TABS}.tabOf('w-b')?.grid")
            check("T14 after a reload the grid is back", grid == [["w-b"], ["w-a", "w-f"]], str(grid))
            now = {i: page.evaluate(BOX, i) for i in boxes}
            check("T14 …and painted", page.evaluate(SHOWN_IN_TAB) == ["w-a", "w-b", "w-f"]
                  and all(near(now[i], boxes[i]) for i in boxes), f"{page.evaluate(SHOWN_IN_TAB)} {now}")

            # T15 -----------------------------------------------------------
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(800)
            check("T15 at a phone width only the first console is painted", page.evaluate(SHOWN_IN_TAB) == ["w-b"],
                  str(page.evaluate(SHOWN_IN_TAB)))
            check("T15 …and it fills the tab", page.evaluate(FILLS, "w-b"))
            page.set_viewport_size(dict(WIDE))
            page.wait_for_timeout(800)

            # T16 -----------------------------------------------------------
            f_rect = page.evaluate("() => __inline('w-f')")
            tab_slice(page, "w-f", "w-l", swap=True)
            grid = page.evaluate(f"() => {T.TABS}.tabOf('w-b')?.grid")
            check("T16 Swap puts w-l in w-f's box",
                  grid == [["w-b"], ["w-a", "w-l"]] and near(page.evaluate(BOX, "w-l"), boxes["w-f"]), str(grid))
            check("T16 …and w-f is back on the stage at its rect",
                  page.evaluate("() => __where('w-f')") == "stage" and page.evaluate("() => __inline('w-f')") == f_rect)
            page.evaluate("() => __P('w-a').querySelector('.session-max').click()")
            page.wait_for_timeout(600)
            grid = page.evaluate(f"() => {T.TABS}.tabOf('w-b')?.grid")
            check("T16 Restore takes w-a out of the tab", grid == [["w-b"], ["w-l"]], str(grid))
            check("T16 …back on the stage at its rect",
                  page.evaluate("() => __where('w-a')") == "stage" and page.evaluate("() => __inline('w-a')") == a_rect,
                  page.evaluate("() => __inline('w-a')"))
            check("T16 the other two share the tab", near(page.evaluate(BOX, "w-l"), [W / 2, 0, W / 2, H]),
                  str(page.evaluate(BOX, "w-l")))

            # T17 -----------------------------------------------------------
            page.locator(".tab.active .tab-close").click()
            page.wait_for_timeout(600)
            check("T17 the × puts every window of the grid on the stage",
                  [page.evaluate("(id) => __where(id)", i) for i in ("w-b", "w-l")] == ["stage", "stage"])
            check("T17 …with no box of the tab", page.evaluate(
                "() => ['w-b','w-l'].every((id) => !__P(id).style.getPropertyValue('--tab-col-count'))"))
            page.wait_for_timeout(1500)
            check("T17 the desk keeps its rects and its maximize", desk_state() == desk_t12, str(desk_state()))

            shadow_checks(page)

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
