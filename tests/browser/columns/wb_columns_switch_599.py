"""Switch the maximized console on a phone (#599, ADR-0051 §5) browser
acceptance.

A phone paints one console, so the title bar's list button is "Switch" there:
a row of the list puts its console in the maximize in place of the current
one. Wider, the same button is "Slice", a row still swaps, and the icon
beside a row opens a column.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7599. The fixture and
the daemon helpers are `wb_columns_473.py`'s:
  w-a  loose, live shell, maximized
  w-b  loose, live shell
  w-c  loose, placeholder
  w-f  placeholder inside fence f-one
  w-l  placeholder inside fence f-lock

P1  phone width: the maximized w-a shows the button as "Switch"
P2  the list hides Right | Down and the open icons; a loose row is enabled
P3  a tap on w-b: w-b is maximized, w-a is not, no column opens, the list
    closes, and the desk records the new maximize
P4  after f-one is detached, its w-f is in the list but disabled; the popup
    shows no list button
W1  wide: the button is "Slice", the list shows Right | Down and the open
    icons, and an open icon opens a column

The list is closed through its component: Escape closes it only while the
focus is in its filter, and the list button toggles an open list closed.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: python tests/browser/columns/wb_columns_switch_599.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7599
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "599-switch-2026-10-12.png")
PHONE = {"width": 400, "height": 800}
WIDE = {"width": 2400, "height": 1000}
FLOOR = 17  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


BUTTON = """(id) => { const b = __colBtn(id);
  return b && { hidden: b.hidden, title: b.title, label: b.getAttribute('aria-label'),
                swapIcon: !!b.querySelector('.bi-arrow-down-up') }; }"""

LIST = """() => { const m = document.querySelector('.column-menu');
  const shown = (el) => !!el && el.getClientRects().length > 0;
  return {
    dir: shown(m.querySelector('.column-dir')),
    opens: [...m.querySelectorAll('.column-open')].filter(shown).length,
    rows: Object.fromEntries([...m.querySelectorAll('.column-item')].map((b) => [b.dataset.id, !b.disabled])),
  }; }"""

MAX = "() => [...document.querySelectorAll('#stage .session-window.maximized')].map((w) => w._deskId)"


def close_list(page):
    page.evaluate(f"() => {{ {T.COLS}.columnMenu = false; }}")
    page.wait_for_timeout(150)


def main():
    T.build()
    daemon_dir = tempfile.mkdtemp(prefix="wbswitch_daemon_")
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
            ctx = browser.new_context(viewport=dict(PHONE))
            page = ctx.new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            T.boot(page)

            # P1 -------------------------------------------------------------
            cap = page.evaluate(f"() => {T.COLS}.columnCap()")
            check("P1 setup: a phone width has a cap of 1", cap == 1, f"cap={cap}")
            check("P1 setup: w-a is maximized", page.evaluate(MAX) == ["w-a"], str(page.evaluate(MAX)))
            btn = page.evaluate(BUTTON, "w-a")
            check("P1 the button shows", btn and not btn["hidden"], str(btn))
            check("P1 …named Switch, with the swap icon",
                  btn and btn["title"] == "Switch" and btn["label"] == "Switch" and btn["swapIcon"], str(btn))

            # P2 -------------------------------------------------------------
            T.open_menu(page, "w-a")
            lst = page.evaluate(LIST)
            check("P2 Right | Down is hidden", not lst["dir"], str(lst))
            check("P2 the open icons are hidden", lst["opens"] == 0, str(lst))
            check("P2 a loose console's row is enabled", lst["rows"].get("w-b") is True, str(lst))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.screenshot(path=SHOT)

            # P3 -------------------------------------------------------------
            page.locator(".column-menu .column-item[data-id='w-b']").click()
            page.wait_for_timeout(500)
            check("P3 w-b took the maximize from w-a", page.evaluate(MAX) == ["w-b"], str(page.evaluate(MAX)))
            check("P3 no column opened",
                  T.shell_cols(page) == [] and page.evaluate("() => !document.querySelector('.session-window.column')"),
                  str(T.shell_cols(page)))
            check("P3 the list closed", page.evaluate(f"() => {T.COLS}.columnMenu") is False)
            desk = T.poll_desk(lambda d: d["w-b"].get("max") and not d["w-a"].get("max"))
            check("P3 the desk records the new maximize",
                  desk and desk["w-b"].get("max") and not desk["w-a"].get("max"),
                  str({k: v.get("max") for k, v in (desk or {}).items()}))

            # P4 -------------------------------------------------------------
            with page.expect_popup(timeout=15000) as info:
                page.evaluate("() => WBConsole.detachFence('f-one')")
            popup = info.value
            popup.on("pageerror", lambda e: errors.append("popup: " + str(e)))
            popup.wait_for_load_state()
            try:
                popup.wait_for_function(
                    "() => [...document.querySelectorAll('.session-window')].some((w) => w._deskId === 'w-f')",
                    timeout=15000,
                )
            except Exception:
                pass
            popup.wait_for_timeout(500)
            T.open_menu(page, "w-b")
            lst = page.evaluate(LIST)
            check("P4 the detached w-f is listed but disabled", lst["rows"].get("w-f") is False, str(lst))
            buttons = popup.evaluate("() => [...document.querySelectorAll('.session-column')].map((b) => b.hidden)")
            check("P4 the popup shows no list button", buttons and all(buttons), str(buttons))
            close_list(page)
            popup.close()
            page.wait_for_timeout(500)

            # W1 -------------------------------------------------------------
            page.set_viewport_size(dict(WIDE))
            page.wait_for_timeout(800)
            btn = page.evaluate(BUTTON, "w-b")
            check("W1 wide, the button is Slice",
                  btn and not btn["hidden"] and btn["title"] == "Slice" and not btn["swapIcon"], str(btn))
            T.open_menu(page, "w-b")
            lst = page.evaluate(LIST)
            check("W1 Right | Down and the open icons show", lst["dir"] and lst["opens"] > 0, str(lst))
            page.locator(".column-menu .column-open[data-id='w-c']").click()
            page.wait_for_timeout(500)
            check("W1 the open icon opens a column", T.shell_cols(page) == ["w-b", "w-c"], str(T.shell_cols(page)))

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
