"""A console in its own tab (#748, ADR-0037 §3d) browser acceptance.

The + at the end of the tab strip lists the consoles on the stage. A pick
opens that console in its own tab, maximized on this page only. The tab
belongs to this device: the desk never records its maximize.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7748. The fixture and
the daemon helpers are `wb_columns_473.py`'s (w-a is maximized on the desk).

T1  the + lists the consoles of the stage: the column list's rows and the
    console that opened that list
T2  a pick opens one tab, active, with w-b maximized and no Restore on it;
    the desk does not record w-b as maximized
T3  a second pick activates the same tab
T4  a column opened in the tab keeps its Restore
T5  another device sees no tab and no maximize of w-b
T6  after a reload the tab comes back active, with w-b maximized and its column
T7  at a phone width only the main console shows
T8  the × returns w-b to its rect; it keeps running, and the desk is unchanged
T9  closing a console closes its tab

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
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "748-console-tab-2026-10-20.png")
WIDE = {"width": 2400, "height": 1000}
PHONE = {"width": 400, "height": 800}
FLOOR = 22  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


MAX = "() => [...document.querySelectorAll('#stage .session-window.maximized')].map((w) => w._deskId).sort()"
TAB_IDS = f"() => {T.TABS}.tabs.map((t) => t.id)"
ACTIVE = f"() => {T.TABS}.active"
PLUS_ROWS = "() => [...document.querySelectorAll('.console-tab-menu .console-tab-item')].map((b) => b.dataset.id).sort()"
COLUMN_ROWS = "() => [...document.querySelectorAll('.column-menu .column-item')].map((b) => b.dataset.id).sort()"
RESTORE = "(id) => { const b = __W(id)?.querySelector('.session-max'); return b && { hidden: b.hidden, title: b.title }; }"


def open_plus(page):
    page.locator(".tab-add").click()
    page.wait_for_selector(".console-tab-menu .console-tab-item", state="visible", timeout=5000)
    page.wait_for_timeout(150)


def pick(page, id):
    open_plus(page)
    page.locator(f".console-tab-menu .console-tab-item[data-id='{id}']").click()
    page.wait_for_timeout(600)


def boot_here(page, want=5):
    """`T.boot` without its activate('consoles'): the restored tab must stay."""
    page.wait_for_selector("[x-data]", timeout=8000)
    page.wait_for_function(
        "(n) => document.querySelectorAll('#stage .session-window').length === n", arg=want, timeout=20000
    )
    page.evaluate(T.HELPERS)
    page.wait_for_timeout(1200)


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
            check("T2 w-b is maximized", "w-b" in page.evaluate(MAX), str(page.evaluate(MAX)))
            btn = page.evaluate(RESTORE, "w-b")
            check("T2 w-b shows no Restore", btn and btn["hidden"], str(btn))
            check("T2 the stage shows", page.evaluate(
                "() => !document.querySelector('.consoles-tab').classList.contains('tab-hidden')"))
            page.wait_for_timeout(1500)
            desk = T.desk_windows()
            check("T2 the desk does not record w-b as maximized", not desk["w-b"].get("max"),
                  str({k: v.get("max") for k, v in desk.items()}))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # T3 -------------------------------------------------------------
            page.evaluate(f"() => {T.TABS}.activate('consoles')")
            page.wait_for_timeout(300)
            check("T3 back on Consoles, w-b stays maximized", "w-b" in page.evaluate(MAX))
            pick(page, "w-b")
            check("T3 a second pick activates the same tab",
                  page.evaluate(TAB_IDS).count("console:w-b") == 1 and page.evaluate(ACTIVE) == "console:w-b",
                  str(page.evaluate(TAB_IDS)))

            # T4 -------------------------------------------------------------
            T.open_column(page, "w-b", "w-c")
            check("T4 a column opens in the tab", T.shell_cols(page) == ["w-b", "w-c"], str(T.shell_cols(page)))
            btn = page.evaluate(RESTORE, "w-c")
            check("T4 the column keeps Restore", btn and not btn["hidden"] and btn["title"] == "Restore", str(btn))
            page.wait_for_timeout(1500)
            desk = T.desk_windows()
            check("T4 the desk still records no maximize of w-b", not desk["w-b"].get("max"),
                  str({k: v.get("max") for k, v in desk.items()}))

            # T5 -------------------------------------------------------------
            ctx_b = browser.new_context(viewport=dict(WIDE))
            page_b = ctx_b.new_page()
            page_b.goto(BASE)
            T.boot(page_b)
            check("T5 another device sees no console tab",
                  not [t for t in page_b.evaluate(TAB_IDS) if t.startswith("console:")], str(page_b.evaluate(TAB_IDS)))
            check("T5 …and no maximize of w-b", "w-b" not in page_b.evaluate(MAX), str(page_b.evaluate(MAX)))
            ctx_b.close()

            # T6 -------------------------------------------------------------
            page.reload()
            boot_here(page)
            check("T6 after a reload the tab is back and active", page.evaluate(ACTIVE) == "console:w-b",
                  f"{page.evaluate(ACTIVE)} {page.evaluate(TAB_IDS)}")
            check("T6 w-b is maximized", "w-b" in page.evaluate(MAX), str(page.evaluate(MAX)))
            check("T6 its column is back", T.shell_cols(page) == ["w-b", "w-c"], str(T.shell_cols(page)))

            # T7 -------------------------------------------------------------
            page.set_viewport_size(dict(PHONE))
            page.wait_for_timeout(800)
            check("T7 at a phone width no column is painted",
                  page.evaluate("() => !document.querySelector('#stage .session-window.column')"))
            check("T7 …and w-b is still maximized", "w-b" in page.evaluate(MAX), str(page.evaluate(MAX)))
            page.set_viewport_size(dict(WIDE))
            page.wait_for_timeout(800)

            # T8 -------------------------------------------------------------
            page.locator(".tab.active .tab-close").click()
            page.wait_for_timeout(600)
            check("T8 the tab closed", "console:w-b" not in page.evaluate(TAB_IDS), str(page.evaluate(TAB_IDS)))
            check("T8 w-b is not maximized", "w-b" not in page.evaluate(MAX), str(page.evaluate(MAX)))
            check("T8 w-b is still on the stage and not ended",
                  page.evaluate("() => !!__W('w-b') && !__W('w-b').classList.contains('ended')"))
            page.wait_for_timeout(1500)
            desk = T.desk_windows()
            check("T8 the desk keeps w-a as its maximize",
                  desk["w-a"].get("max") and not desk["w-b"].get("max"),
                  str({k: v.get("max") for k, v in desk.items()}))

            # T9 -------------------------------------------------------------
            pick(page, "w-c")
            check("T9 setup: w-c has a tab", "console:w-c" in page.evaluate(TAB_IDS))
            page.evaluate("() => __W('w-c').querySelector('.session-close').click()")
            T.confirm(page)
            try:
                page.wait_for_function(f"() => !{T.TABS}.tabs.some((t) => t.id === 'console:w-c')", timeout=8000)
            except Exception:
                pass
            check("T9 closing the console closes its tab", "console:w-c" not in page.evaluate(TAB_IDS),
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
