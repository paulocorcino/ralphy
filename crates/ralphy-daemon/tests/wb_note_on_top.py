"""A card on top (ADR-0064, 2026-09-26 amendment) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7481.

The fixture: one live shell `w-a`, maximized, and a LOCKED fence `f-lock`. Two
notes are created through `WBNotes.create`; note A is moved inside the fence.

T1  the Note menu row keeps A on top: it floats in the top-right corner of the
    viewport, over the maximized console, and the menu closes
T2  a shadow holds A's place in the fence; the desk rect does not change
T3  the card is editable while it floats (the fence is locked) and autosaves
T4  the floating card moves and resizes; the desk rect still does not change
T5  a click on the console behind keeps the card in view
T6  Put back returns the card to its desk rect and removes the shadow
T7  one card at a time: keeping B on top puts A back
T8  a click on the shadow puts the card back
T9  a reload finds nothing on top
T10 detaching the fence while A is on top puts A back first; A's row is refused
T11 a 390 px viewport shows the band across the top

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python crates/ralphy-daemon/tests/wb_note_on_top.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7481
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SH = T.SH
SHOT = os.path.join(T.REPO_ROOT, "docs", "screenshots", "note-on-top-2026-09-26.png")
SHOT_BAND = os.path.join(T.REPO_ROOT, "docs", "screenshots", "note-on-top-band-2026-09-26.png")
VIEW = {"width": 1600, "height": 1000}
F_LOCK = {"left": 700, "top": 40, "width": 600, "height": 500}
A_RECT = {"left": 760, "top": 120, "width": 240, "height": 180}

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


def write_desk(daemon_dir, slug):
    out = (
        "[[windows]]\n"
        'id = "w-a"\n'
        f'repo = "{slug}"\n'
        'agent = "console"\n'
        'kind = "console"\n'
        "max = true\n"
        "ts = 100\n"
        + T.rect_toml({"left": 40, "top": 700, "width": 500, "height": 300})
        + "\n\n[[fences]]\n"
        'id = "f-lock"\n'
        'name = "held"\n'
        "locked = true\n"
        "ts = 200\n" + T.rect_toml(F_LOCK) + "\n"
    )
    with open(os.path.join(daemon_dir, "desk.toml"), "wb") as f:
        f.write(out.encode("utf-8"))


HELPERS = """() => {
  window.__card = (id) => WBNotes.cardEl(id);
  window.__box = (el) => { const ws = document.getElementById('workspace').getBoundingClientRect();
    const r = el.getBoundingClientRect();
    return { left: r.left - ws.left, top: r.top - ws.top, width: r.width, height: r.height,
             wsWidth: ws.width, wsHeight: ws.height }; };
  window.__hit = (el) => { const r = el.getBoundingClientRect();
    const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!at && el.contains(at); };
  window.__rect = (id) => WBConsole.notes().find((n) => n.id === id)?.rect ?? null;
}"""


def boot(page, slug):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_function(
        "() => { const w = document.querySelector('#stage .session-window');"
        " return !!w && w.clientWidth > 0; }",
        timeout=20000,
    )
    page.evaluate(HELPERS)
    page.wait_for_timeout(600)


def create_note(page, slug, text):
    nid = page.evaluate(
        """([repo]) => { const ws = document.getElementById('workspace');
          return WBNotes.create({ repo, viewport: { width: ws.clientWidth, height: ws.clientHeight },
                                  offset: { left: ws.scrollLeft, top: ws.scrollTop } }); }""",
        [slug],
    )
    page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=nid, timeout=15000)
    page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", nid)
    page.keyboard.type(text)
    page.keyboard.press("Control+s")
    page.wait_for_function("(id) => !__card(id)._noteDirty && !!WBConsole.notes().find((n) => n.id === id)?.path",
                           arg=nid, timeout=10000)
    return nid


def move_note(page, nid, rect):
    page.evaluate(
        """([id, rect]) => { WBConsole.saveNotes(WBConsole.notes().map((n) => n.id === id ? { ...n, rect, ts: Date.now() } : n));
          WBNotes.render(); }""",
        [nid, rect],
    )


def row_top(page, nid):
    page.evaluate(f"() => {{ {SH}.toggleNoteMenu(); }}")
    page.wait_for_function(f"() => {SH}.noteMenu === true", timeout=3000)
    page.wait_for_timeout(150)
    rows = page.locator(".note-menu .note-row")
    titles = page.evaluate(f"() => {SH}.noteItems.map((n) => n.id)")
    return rows.nth(titles.index(nid)).locator(".note-top")


def main():
    d = tempfile.mkdtemp(prefix="wbtop_")
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
            ctx = b.new_context(viewport=dict(VIEW))
            page = ctx.new_page()
            page.goto(BASE)
            boot(page, slug)
            a = create_note(page, slug, "Note A")
            move_note(page, a, A_RECT)
            bnote = create_note(page, slug, "Note B")
            page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
            page.wait_for_timeout(300)
            console_max = page.evaluate(
                "() => document.querySelector('#stage .session-window').classList.contains('maximized')"
            )
            check("fixture: the console is maximized", console_max)
            check("fixture: A is locked by its fence", page.evaluate("(id) => !!__card(id)._noteLocked", a))

            # T1
            row_top(page, a).click()
            page.wait_for_function("(id) => __card(id).classList.contains('on-top')", arg=a, timeout=3000)
            page.wait_for_timeout(200)
            box = page.evaluate("(id) => __box(__card(id))", a)
            check("T1 the card floats at the floor size", box["width"] == 420 and box["height"] == 320, str(box))
            check(
                "T1 in the top-right corner of the viewport",
                abs(box["left"] + box["width"] - (box["wsWidth"] - 12)) <= 2 and abs(box["top"] - 44) <= 2,
                str(box),
            )
            check("T1 over the maximized console", page.evaluate("(id) => __hit(__card(id))", a))
            check("T1 the menu closed", page.evaluate(f"() => {SH}.noteMenu") is False)
            page.screenshot(path=SHOT)

            # T2
            shadow = page.evaluate(
                "(id) => { const s = __card(id)._noteShadow; return s && s.isConnected ?"
                " { left: s.offsetLeft, top: s.offsetTop, w: s.offsetWidth, h: s.offsetHeight } : null; }",
                a,
            )
            check(
                "T2 a shadow holds the place",
                shadow is not None and shadow["left"] == A_RECT["left"] and shadow["top"] == A_RECT["top"],
                str(shadow),
            )
            check("T2 the desk rect is unchanged", page.evaluate("(id) => __rect(id)", a) == A_RECT)

            # T3
            page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", a)
            page.keyboard.press("End")
            page.keyboard.type(" edited on top")
            page.keyboard.press("Control+s")
            page.wait_for_function("(id) => !__card(id)._noteDirty", arg=a, timeout=10000)
            check(
                "T3 editable while floating, and saved",
                "edited on top" in page.evaluate("(id) => __card(id)._noteMarkdown", a),
            )

            # T4
            head = page.evaluate(
                "(id) => { const r = __card(id).querySelector('.note-grab').getBoundingClientRect();"
                " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
                a,
            )
            page.mouse.move(head["x"], head["y"])
            page.mouse.down()
            page.mouse.move(head["x"] - 200, head["y"] + 100, steps=8)
            page.mouse.up()
            moved = page.evaluate("(id) => __box(__card(id))", a)
            check("T4 the floating card moves", abs(moved["left"] - (box["left"] - 200)) <= 3, str(moved))
            grip = page.evaluate(
                "(id) => { const r = __card(id).querySelector('.note-grip').getBoundingClientRect();"
                " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
                a,
            )
            page.mouse.move(grip["x"], grip["y"])
            page.mouse.down()
            page.mouse.move(grip["x"] + 60, grip["y"] + 40, steps=8)
            page.mouse.up()
            grown = page.evaluate("(id) => __box(__card(id))", a)
            check("T4 the floating card resizes", grown["width"] > moved["width"] + 40, str(grown))
            check("T4 the desk rect is still unchanged", page.evaluate("(id) => __rect(id)", a) == A_RECT)

            # T5
            page.mouse.click(200, 800)
            page.wait_for_timeout(200)
            check(
                "T5 a click on the console keeps the card in view",
                page.evaluate("(id) => __card(id).classList.contains('on-top') && __hit(__card(id))", a),
            )

            # T6
            page.evaluate("(id) => __card(id).querySelector('.note-putback').click()", a)
            page.wait_for_timeout(200)
            back = page.evaluate(
                "(id) => { const el = __card(id); return { on: el.classList.contains('on-top'),"
                " left: el.offsetLeft, top: el.offsetTop, w: el.offsetWidth, h: el.offsetHeight,"
                " shadow: !!document.querySelector('.note-shadow') }; }",
                a,
            )
            check(
                "T6 Put back returns the card to its desk rect",
                not back["on"]
                and back["left"] == A_RECT["left"]
                and back["top"] == A_RECT["top"]
                and back["w"] == A_RECT["width"],
                str(back),
            )
            check("T6 the shadow is gone", not back["shadow"])

            # T7
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            row_top(page, bnote).click()
            page.wait_for_function("(id) => __card(id).classList.contains('on-top')", arg=bnote, timeout=3000)
            check(
                "T7 keeping B on top puts A back",
                page.evaluate(
                    "([a, b]) => !__card(a).classList.contains('on-top') && WBNotes.onTopNow() === b"
                    " && document.querySelectorAll('.note-shadow').length === 1",
                    [a, bnote],
                ),
            )

            # T8
            page.evaluate("(id) => __card(id)._noteShadow.click()", bnote)
            page.wait_for_timeout(150)
            check(
                "T8 a click on the shadow puts the card back",
                page.evaluate("(id) => !__card(id).classList.contains('on-top') && WBNotes.onTopNow() === null", bnote),
            )

            # T9
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            page.reload()
            boot(page, slug)
            page.wait_for_function("(id) => !!__card(id)", arg=a, timeout=15000)
            check(
                "T9 a reload finds the card in its place",
                page.evaluate("(id) => !__card(id).classList.contains('on-top') && WBNotes.onTopNow() === null", a),
            )

            # T10
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            with ctx.expect_page(timeout=10000) as popup_info:
                page.evaluate("() => WBConsole.detachFence('f-lock')")
            popup = popup_info.value
            page.wait_for_timeout(800)
            check(
                "T10 the detach put A back and took it off this stage",
                page.evaluate("(id) => WBNotes.onTopNow() === null && !__card(id)", a)
                and page.evaluate("() => document.querySelectorAll('.note-shadow').length === 0"),
            )
            refused = page.evaluate("(id) => WBNotes.list().find((n) => n.id === id)", a)
            check("T10 A's row is refused while it is in the popup", refused and refused["away"] is True)
            check("T10 keepOnTop refuses it too", page.evaluate("(id) => WBNotes.keepOnTop(id)", a) is False)
            popup.close()
            page.wait_for_timeout(800)

            # T11
            phone = b.new_context(viewport={"width": 390, "height": 800}, has_touch=True)
            pp = phone.new_page()
            pp.goto(BASE)
            boot(pp, slug)
            pp.wait_for_function("(id) => !!__card(id)", arg=bnote, timeout=15000)
            pp.evaluate("(id) => WBNotes.keepOnTop(id)", bnote)
            pp.wait_for_timeout(300)
            band = pp.evaluate("(id) => ({ band: __card(id).classList.contains('band'), ...__box(__card(id)) })", bnote)
            check(
                "T11 a phone shows the band across the top",
                band["band"] and abs(band["left"] - 8) <= 2 and abs(band["top"] - 44) <= 2
                and abs(band["width"] - (band["wsWidth"] - 16)) <= 2,
                str(band),
            )
            pp.screenshot(path=SHOT_BAND)
            phone.close()
            b.close()
    finally:
        T.stop(proc)


if __name__ == "__main__":
    main()
    print(f"{sum(results)}/{len(results)} passed")
    sys.exit(0 if results and all(results) else 1)
