"""The Tile button of a detached fence window browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7484.

The fixture: two consoles `w-a` and `w-b` and one note inside the fence
`f-det`, which is then detached.

D1 the popup shows the Tile button
D2 after the button and its question, every console is inside the window's
   visible part, below the button, and no two consoles overlap
D3 the note did not move, and it is above every console
D4 the popup wrote nothing: the daemon's desk is the same as before the tile

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/fence/wb_fence_detached_tile.py   (exit 0 = all pass)
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

W_B = {"left": 120, "top": 140, "width": 360, "height": 240}

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


# Every box in the popup, in viewport pixels, with its stacking order.
BOXES = """() => [...document.querySelectorAll('#stage .session-window, #stage .note-card')].map((el) => {
  const r = el.getBoundingClientRect();
  return { note: el.classList.contains('note-card'), left: r.left, top: r.top,
           right: r.right, bottom: r.bottom, z: parseInt(el.style.zIndex, 10) || 0 };
})"""


def overlap(a, b):
    return a["left"] < b["right"] and b["left"] < a["right"] and a["top"] < b["bottom"] and b["top"] < a["bottom"]


def main():
    d = tempfile.mkdtemp(prefix="wbdettile_")
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
            nid = D.new_note(page, slug, D.N_IN_DET)
            page.wait_for_timeout(1500)

            popup = D.detach(ctx, page, "f-det")
            popup.wait_for_function("() => document.querySelectorAll('#stage .session-window').length === 2",
                                    timeout=15000)
            popup.wait_for_function("(id) => !!window.WBNotes?.cardEl(id)", arg=nid, timeout=15000)
            popup.wait_for_timeout(800)

            # D1
            button = popup.locator(".detached-tile")
            check("D1 the popup shows the Tile button", button.count() == 1 and button.is_visible())

            note_before = next(x for x in popup.evaluate(BOXES) if x["note"])
            desk_before = T.desk_raw()
            button.click()
            T.confirm(popup)
            popup.wait_for_timeout(800)

            # D2
            vp = popup.evaluate(
                "() => { const ws = document.getElementById('workspace'); const r = ws.getBoundingClientRect();"
                " return { left: r.left, top: r.top, right: r.left + ws.clientWidth, bottom: r.top + ws.clientHeight }; }"
            )
            band = popup.evaluate("() => document.querySelector('.detached-tile').getBoundingClientRect().bottom")
            boxes = popup.evaluate(BOXES)
            consoles = [x for x in boxes if not x["note"]]
            inside = all(
                c["left"] >= vp["left"] - 1
                and c["right"] <= vp["right"] + 1
                and c["top"] >= band - 1
                and c["bottom"] <= vp["bottom"] + 1
                for c in consoles
            )
            check("D2 every console is in the window, below the button", inside, f"{consoles} vp={vp} band={band}")
            check(
                "D2 no two consoles overlap",
                len(consoles) == 2 and not overlap(consoles[0], consoles[1]),
                f"{consoles}",
            )

            # D3
            note = next(x for x in boxes if x["note"])
            same = all(abs(note[k] - note_before[k]) < 1 for k in ("left", "top", "right", "bottom"))
            check("D3 the note did not move", same, f"before={note_before} after={note}")
            check(
                "D3 the note is above every console",
                all(note["z"] > c["z"] for c in consoles),
                f"note={note['z']} consoles={[c['z'] for c in consoles]}",
            )

            # D4
            popup.wait_for_timeout(1500)
            check("D4 the popup wrote nothing to the desk", T.desk_raw() == desk_before)
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
