"""Columns and a note on top in a detached fence window, browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7484.

The fixture: two consoles `w-a` and `w-b` inside the fence `f-det`, then a note
in the same fence, which is then detached.

C1 the columns button is in the bottom-right corner, a press at its centre
   reaches it, and it covers no control of any console
C2 the button opens both consoles as columns that fill the window, side by
   side, with no overlap
C3 a column's restore ends the columns
C4 the popup wrote nothing: the daemon's desk is the same as before
N1 the note button keeps the note on top, above every column
N2 the card's Put back takes it off the top
N3 a window with no note does not show the note button
P1 on a phone the window shows neither button

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/fence/wb_fence_detached_columns.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "notes"))
import wb_note_detach_draft as D  # noqa: E402  the fixture and page helpers
from playwright.sync_api import sync_playwright  # noqa: E402

T = D.T
PORT = 7484
T.PORT = PORT
T.BASE = D.BASE = BASE = f"http://127.0.0.1:{PORT}/"

W_B = {"left": 440, "top": 80, "width": 280, "height": 240}

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    out = ""
    for wid, rect, ts in (("w-a", D.W_A, 100), ("w-b", W_B, 101)):
        out += (
            "[[windows]]\n"
            f'id = "{wid}"\n'
            f'repo = "{slug}"\n'
            'agent = "console"\n'
            'kind = "console"\n'
            f"ts = {ts}\n" + T.rect_toml(rect) + "\n\n"
        )
    out += '[[fences]]\nid = "f-det"\nname = "det"\nts = 200\n' + T.rect_toml(D.F_DET) + "\n"
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


# Every console in the popup, in viewport pixels, with its stacking order.
CONSOLES = """() => [...document.querySelectorAll('#stage .session-window')].map((el) => {
  const r = el.getBoundingClientRect();
  return { column: el.classList.contains('column'), left: r.left, top: r.top,
           right: r.right, bottom: r.bottom, z: parseInt(el.style.zIndex, 10) || 0 };
})"""

# Whether a press at the button's centre reaches it, and whether its box
# meets any visible control of a console.
REACH = """(sel) => { const b = document.querySelector(sel); const r = b.getBoundingClientRect();
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  const meets = [...document.querySelectorAll('#stage .session-actions button')].filter((x) => {
    const q = x.getBoundingClientRect();
    return q.width > 0 && q.left < r.right && r.left < q.right && q.top < r.bottom && r.top < q.bottom; });
  return { reached: !!hit && b.contains(hit), meets: meets.length,
           right: innerWidth - r.right, bottom: innerHeight - r.bottom }; }"""

VIEWPORT = """() => { const ws = document.getElementById('workspace'); const r = ws.getBoundingClientRect();
  return { left: r.left, top: r.top, right: r.left + ws.clientWidth, bottom: r.top + ws.clientHeight }; }"""


def overlap(a, b):
    return a["left"] < b["right"] - 1 and b["left"] < a["right"] - 1 and a["top"] < b["bottom"] - 1 and b["top"] < a["bottom"] - 1


def open_popup(ctx, page):
    popup = D.detach(ctx, page, "f-det")
    popup.wait_for_function("() => document.querySelectorAll('#stage .session-window').length === 2", timeout=15000)
    popup.wait_for_selector(".detached-tools", state="attached", timeout=5000)
    popup.wait_for_timeout(800)
    return popup


def visible(popup, sel):
    return popup.locator(sel).is_visible()


def main():
    d = tempfile.mkdtemp(prefix="wbdetcol_")
    fx = T.make_fixture_repo()
    slug = T.register_fixture(d, fx)
    write_desk(d, slug)
    proc = T.launch(d)
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            b = p.chromium.launch()
            ctx = b.new_context(viewport=dict(D.VIEW))
            page = ctx.new_page()
            page.goto(BASE)
            D.boot(page)

            # N3, before the note exists
            popup = open_popup(ctx, page)
            check("N3 a window with no note does not show the note button", not visible(popup, ".detached-note"))
            popup.close()
            page.wait_for_function("() => !WBConsole.isDetached('f-det')", timeout=8000)
            page.wait_for_timeout(800)

            nid = D.new_note(page, slug, D.N_IN_DET)
            page.wait_for_timeout(1500)
            popup = open_popup(ctx, page)
            popup.wait_for_function("(id) => !!window.WBNotes?.cardEl(id)", arg=nid, timeout=15000)

            # C1
            reach = popup.evaluate(REACH, ".detached-columns")
            check(
                "C1 the columns button is in the bottom-right corner, reachable, over no console control",
                reach["reached"] and reach["meets"] == 0 and reach["right"] < 60 and reach["bottom"] < 40,
                f"{reach}",
            )

            # C2
            desk_before = T.desk_raw()
            popup.locator(".detached-columns").click()
            popup.wait_for_timeout(800)
            vp = popup.evaluate(VIEWPORT)
            cols = popup.evaluate(CONSOLES)
            fills = (
                all(c["column"] for c in cols)
                and abs(min(c["left"] for c in cols) - vp["left"]) < 2
                and abs(max(c["right"] for c in cols) - vp["right"]) < 2
                and all(abs(c["top"] - vp["top"]) < 2 and abs(c["bottom"] - vp["bottom"]) < 2 for c in cols)
            )
            check("C2 both consoles are columns that fill the window", len(cols) == 2 and fills, f"{cols} vp={vp}")
            check("C2 the columns do not overlap", len(cols) == 2 and not overlap(cols[0], cols[1]), f"{cols}")
            reach = popup.evaluate(REACH, ".detached-columns")
            check("C1 ... and still covers no control with the columns open", reach["meets"] == 0, f"{reach}")

            # N1
            popup.locator(".detached-note").click()
            popup.wait_for_timeout(500)
            on_top = popup.evaluate("() => WBNotes.onTopNow()")
            note_z = popup.evaluate("(id) => parseInt(getComputedStyle(WBNotes.cardEl(id)).zIndex, 10)", nid)
            cols = popup.evaluate(CONSOLES)
            check(
                "N1 the note button keeps the note on top, above every column",
                on_top == nid and all(note_z > c["z"] for c in cols),
                f"onTop={on_top} note={note_z} consoles={[c['z'] for c in cols]}",
            )

            # N2
            popup.evaluate("(id) => WBNotes.cardEl(id).querySelector('.note-putback').click()", nid)
            popup.wait_for_timeout(300)
            check("N2 Put back takes the note off the top", popup.evaluate("() => WBNotes.onTopNow()") is None)

            # C3
            popup.locator("#stage .session-window.column .session-max").first.click()
            popup.wait_for_timeout(600)
            check(
                "C3 a column's restore ends the columns",
                popup.evaluate("() => document.querySelectorAll('#stage .session-window.column').length") == 0,
            )

            # C4
            popup.wait_for_timeout(1500)
            check("C4 the popup wrote nothing to the desk", T.desk_raw() == desk_before)

            # P1: Playwright gives the popup the size `window.open` asks for.
            # A phone opens it as a tab with the phone's own width.
            popup.set_viewport_size(p.devices["iPhone 13"]["viewport"])
            popup.wait_for_timeout(300)
            check(
                "P1 a phone shows neither button",
                not visible(popup, ".detached-columns") and not visible(popup, ".detached-note"),
            )
            b.close()
    except BaseException:
        # A crash is a failure, and its traceback is printed: the exit below
        # would otherwise replace it with the verdict of the checks so far.
        traceback.print_exc()
        results.append(False)
    finally:
        T.stop(proc)
        # In `finally`, so an early return still reports and still fails.
        passed = sum(results)
        print(f"\n{passed}/{len(results)} passed")
        sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
