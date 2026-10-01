"""A card on top (ADR-0064, 2026-09-26 amendment) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7481.

The fixture: one live shell `w-a`, maximized, a LOCKED fence `f-lock` and an
unlocked fence `f-open`. Two notes are created through `WBNotes.create`; note A
is moved inside `f-lock` and note B inside `f-open`.

T1  the Note menu row keeps A on top: it floats in the top-right corner of the
    viewport, over the maximized console, the menu closes, and the row is lit
T2  a shadow holds A's place in the fence, under every window; the desk rect
    does not change; a window resize while the tab is hidden keeps the box
T3  the card is editable while it floats (the fence is locked) and autosaves
T4  the floating card moves and resizes; the desk rect still does not change
T5  a click on the console behind keeps the card in view
T6  while floating, Put back is shown and close and lock are not; Put back
    returns the card to its desk rect, removes the shadow, and puts the
    maximized console back in front of it
T7  one card at a time: keeping B on top puts A back
T8  a click on the shadow puts the card back
T9  a reload finds nothing on top, and the card reads the text saved in T3
T10 detaching the fence while A is on top puts A back first, writes A's unsaved
    text, and keeps A's desk rect; A's row is refused
T11 a 390 px viewport shows the band across the top, half the viewport high
T12 an unlocked card (B) on top moves without touching its desk rect
T13 panning the stage does not move the card on top
T14 moving an unlocked fence while its card floats moves the desk rect and the
    shadow, and Put back lands the card at the new place
T15 a record another client closes takes the floating card away, after its
    unsaved text is written
T16 a tap on the title of a floating card opens the rename field, and it stays
    open
T17 a card on top never falls asleep: the dormancy observer cannot see a fixed
    card, so an idle floating card kept its editor only by the fold's rule

"The desk rect" is read twice: from this tab's cache and from the daemon's
`/api/desk`, so a write that only one of them saw fails the check.

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/notes/wb_note_on_top.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7481
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SH = T.SH
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "note-on-top-2026-09-26.png")
SHOT_BAND = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "note-on-top-band-2026-09-26.png")
VIEW = {"width": 1600, "height": 1000}
F_LOCK = {"left": 700, "top": 40, "width": 600, "height": 500}
F_OPEN = {"left": 40, "top": 40, "width": 600, "height": 500}
A_RECT = {"left": 760, "top": 120, "width": 240, "height": 180}
B_RECT = {"left": 100, "top": 120, "width": 240, "height": 180}

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
        "\n[[fences]]\n"
        'id = "f-open"\n'
        'name = "open"\n'
        "ts = 201\n" + T.rect_toml(F_OPEN) + "\n"
    )
    with open(os.path.join(daemon_dir, "desk.toml"), "wb") as f:
        f.write(out.encode("utf-8"))


HELPERS = """() => {
  window.__card = (id) => WBNotes.cardEl(id);
  window.__box = (el) => { const ws = document.getElementById('workspace').getBoundingClientRect();
    const r = el.getBoundingClientRect();
    return { left: r.left - ws.left, top: r.top - ws.top, width: r.width, height: r.height,
             clientWidth: document.getElementById('workspace').clientWidth,
             clientHeight: document.getElementById('workspace').clientHeight }; };
  window.__hit = (el) => { const r = el.getBoundingClientRect();
    const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!at && el.contains(at); };
  window.__rect = (id) => WBConsole.notes().find((n) => n.id === id)?.rect ?? null;
}"""


def daemon_rect(nid, want=None, timeout=4):
    """The note's rect as the DAEMON holds it. With `want`, poll until it
    matches (a desk write is debounced); without, wait out one debounce."""
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        try:
            got = next((n.get("rect") for n in T.desk_raw().get("notes", []) if n.get("id") == nid), None)
        except Exception:
            got = None
        if want is not None and got == want:
            return got
        time.sleep(0.3)
    return got


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
            move_note(page, bnote, B_RECT)
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
                abs(box["left"] + box["width"] - (box["clientWidth"] - 12)) <= 2 and abs(box["top"] - 44) <= 2,
                str(box),
            )
            check("T1 over the maximized console", page.evaluate("(id) => __hit(__card(id))", a))
            check("T1 the menu closed", page.evaluate(f"() => {SH}.noteMenu") is False)
            page.screenshot(path=SHOT)
            row_top(page, a)
            check(
                "T1 the row is lit while the card floats",
                page.locator(".note-menu .note-top.on").count() == 1
                and page.evaluate("(id) => WBNotes.list().find((n) => n.id === id).onTop", a) is True,
            )
            page.evaluate(f"() => {{ {SH}.noteMenu = false; }}")
            page.wait_for_timeout(100)

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
            check(
                "T2 the shadow has the desk size, the title, and no editor",
                shadow is not None
                and shadow["w"] == A_RECT["width"]
                and shadow["h"] == A_RECT["height"]
                and page.evaluate(
                    "(id) => { const s = __card(id)._noteShadow;"
                    " return s.querySelector('.note-shadow-title').textContent === 'Untitled note'"
                    " && !s.querySelector('.ProseMirror, .milkdown'); }",
                    a,
                ),
            )
            check("T2 the desk rect is unchanged", page.evaluate("(id) => __rect(id)", a) == A_RECT)
            check("T2 the daemon's desk rect is unchanged", daemon_rect(a) == A_RECT)
            zs = page.evaluate(
                "(id) => { const el = __card(id); WBConsole.focusWin(el); WBNotes.render();"
                " return getComputedStyle(el._noteShadow).zIndex; }",
                a,
            )
            check("T2 the shadow stays under every window after a render", zs == "60", zs)
            refit = page.evaluate(
                """(id) => { const tab = document.querySelector('.consoles-tab'); tab.style.display = 'none';
                  window.dispatchEvent(new Event('resize')); tab.style.display = '';
                  window.dispatchEvent(new Event('scroll'));
                  const el = __card(id); return { band: el.classList.contains('band'), box: !!el._noteOnTop }; }""",
                a,
            )
            check(
                "T2 a window resize while the tab is hidden keeps the floating box",
                refit == {"band": False, "box": True},
                str(refit),
            )

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
            check(
                "T4 the floating card moves on both axes",
                abs(moved["left"] - (box["left"] - 200)) <= 3 and abs(moved["top"] - (box["top"] + 100)) <= 3,
                str(moved),
            )
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
            check(
                "T4 the floating card resizes on both axes",
                abs(grown["width"] - (moved["width"] + 60)) <= 3 and abs(grown["height"] - (moved["height"] + 40)) <= 3,
                str(grown),
            )
            check("T4 the desk rect is still unchanged", page.evaluate("(id) => __rect(id)", a) == A_RECT)
            check("T4 the daemon's desk rect is still unchanged", daemon_rect(a) == A_RECT)

            # T5
            term = page.evaluate(
                "() => { const r = document.querySelector('#stage .session-window .xterm').getBoundingClientRect();"
                " return { x: r.left + 120, y: r.bottom - 120 }; }"
            )
            page.mouse.click(term["x"], term["y"])
            page.wait_for_timeout(200)
            check(
                "T5 a click on the console keeps the card in view",
                page.evaluate("(id) => __card(id).classList.contains('on-top') && __hit(__card(id))", a),
            )
            check(
                "T5 and the console has the focus",
                page.evaluate("() => !!document.activeElement?.closest?.('.session-window')"),
            )

            # T6
            shown = page.evaluate(
                "(id) => ['.note-putback', '.note-close', '.note-lock'].map((s) =>"
                " getComputedStyle(__card(id).querySelector(s)).display)",
                a,
            )
            check(
                "T6 Put back is shown, close and lock are not",
                shown[0] != "none" and shown[1] == "none" and shown[2] == "none",
                str(shown),
            )
            # Typed into last, as an operator does before putting it back: the
            # card then holds the top of the focus ladder.
            page.locator(".note-card.on-top .note-body").click()
            page.wait_for_timeout(150)
            page.locator(".note-card.on-top .note-putback").click()
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
            # Compared by z-index, not by a hit test: this fixture's card is
            # above the scrolled viewport once it is back on the plane.
            z = page.evaluate(
                "(id) => ({ card: +getComputedStyle(__card(id)).zIndex,"
                " max: +getComputedStyle(document.querySelector('#stage .session-window.maximized')).zIndex })",
                a,
            )
            check("T6 after Put back the maximized console is in front of the card again", z["max"] > z["card"], str(z))

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
            page.wait_for_function("(id) => !!__card(id)._noteEditor", arg=a, timeout=15000)
            check(
                "T9 the text saved while floating is in the file",
                "edited on top" in page.evaluate("(id) => __card(id)._noteMarkdown", a),
            )

            # T12
            page.evaluate("(id) => WBNotes.keepOnTop(id)", bnote)
            page.wait_for_timeout(200)
            b0 = page.evaluate("(id) => __box(__card(id))", bnote)
            head = page.evaluate(
                "(id) => { const r = __card(id).querySelector('.note-grab').getBoundingClientRect();"
                " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }",
                bnote,
            )
            page.mouse.move(head["x"], head["y"])
            page.mouse.down()
            page.mouse.move(head["x"] - 150, head["y"] + 80, steps=8)
            page.mouse.up()
            page.wait_for_timeout(300)
            b1 = page.evaluate("(id) => __box(__card(id))", bnote)
            check("T12 the unlocked floating card moves", abs(b1["left"] - (b0["left"] - 150)) <= 3, str(b1))
            check("T12 its desk rect is unchanged", page.evaluate("(id) => __rect(id)", bnote) == B_RECT)
            check("T12 the daemon's desk rect is unchanged", daemon_rect(bnote) == B_RECT)

            # T13
            scrolled = page.evaluate(
                "() => { const ws = document.getElementById('workspace'); ws.scrollTop = 150; ws.scrollLeft = 120;"
                " return [ws.scrollLeft, ws.scrollTop]; }"
            )
            page.wait_for_timeout(300)
            b2 = page.evaluate("(id) => __box(__card(id))", bnote)
            check(
                "T13 panning does not move the card on top",
                (scrolled[0] > 0 or scrolled[1] > 0)
                and abs(b2["left"] - b1["left"]) <= 1
                and abs(b2["top"] - b1["top"]) <= 1,
                f"scroll={scrolled} {b2}",
            )
            page.evaluate("() => { const ws = document.getElementById('workspace'); ws.scrollTop = 0; ws.scrollLeft = 0; }")
            page.wait_for_timeout(200)

            # T14
            page.dblclick(".session-window .session-titlebar", position={"x": 120, "y": 12})
            page.wait_for_function(
                "() => !document.querySelector('#stage .session-window').classList.contains('maximized')",
                timeout=5000,
            )
            page.wait_for_timeout(300)
            grab = page.evaluate(
                "() => { const f = [...document.querySelectorAll('.fence')].find((x) => x.dataset.fenceId === 'f-open');"
                " const r = f.querySelector('.fence-grab').getBoundingClientRect();"
                " return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; }"
            )
            page.mouse.move(grab["x"], grab["y"])
            page.mouse.down()
            page.mouse.move(grab["x"] + 60, grab["y"] + 40, steps=10)
            page.mouse.up()
            page.wait_for_timeout(600)
            want = dict(B_RECT, left=B_RECT["left"] + 60, top=B_RECT["top"] + 40)
            desk_b = page.evaluate("(id) => __rect(id)", bnote)
            check("T14 the fence move moves the desk rect, not the floating box", desk_b == want, str(desk_b))
            got = daemon_rect(bnote, want)
            check("T14 the daemon holds the moved rect", got == want, str(got))
            sh = page.evaluate(
                "(id) => { const s = __card(id)._noteShadow; return s ? [s.offsetLeft, s.offsetTop] : null; }", bnote
            )
            check("T14 the shadow moved with the fence", sh == [want["left"], want["top"]], str(sh))
            page.evaluate("() => WBNotes.putBack()")
            page.wait_for_timeout(200)
            landed = page.evaluate("(id) => [__card(id).offsetLeft, __card(id).offsetTop]", bnote)
            check("T14 Put back lands the card at the new place", landed == [want["left"], want["top"]], str(landed))

            # T10
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", a)
            page.keyboard.press("End")
            page.keyboard.type(" typed before detach")
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
            check("T10 A's desk rect is its place", page.evaluate("(id) => __rect(id)", a) == A_RECT)
            popup.close()
            page.wait_for_timeout(800)
            page.evaluate("() => WBConsole.reattachFence('f-lock')")
            page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=a, timeout=15000)
            check(
                "T10 the unsaved text was written before the card left",
                "typed before detach" in page.evaluate("(id) => __card(id)._noteMarkdown", a),
            )

            # T15
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", a)
            page.keyboard.press("End")
            page.keyboard.type(" typed before a close elsewhere")
            kept = page.evaluate(
                """(id) => { const rec = WBConsole.notes().find((n) => n.id === id);
                  WBConsole.saveNotes(WBConsole.notes().filter((n) => n.id !== id)); WBNotes.render();
                  return rec; }""",
                a,
            )
            check(
                "T15 a record closed elsewhere takes the card off the top and the stage",
                page.evaluate("(id) => !__card(id) && WBNotes.onTopNow() === null", a)
                and page.evaluate("() => document.querySelectorAll('.note-shadow').length === 0"),
            )
            page.wait_for_timeout(1500)
            page.evaluate(
                "(rec) => { WBConsole.saveNotes(WBConsole.notes().concat([{ ...rec, ts: Date.now() }])); WBNotes.render(); }",
                kept,
            )
            page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=a, timeout=15000)
            check(
                "T15 its unsaved text was written before it left",
                "typed before a close elsewhere" in page.evaluate("(id) => __card(id)._noteMarkdown", a),
            )

            # T16
            page.evaluate("(id) => WBNotes.keepOnTop(id)", a)
            page.wait_for_timeout(200)
            page.locator(".note-card.on-top .note-title").click()
            page.wait_for_timeout(400)
            renamed = page.evaluate(
                "(id) => { const f = __card(id).querySelector('.note-title-edit');"
                " return { open: !f.hidden, focused: document.activeElement === f }; }",
                a,
            )
            check("T16 a tap on the floating title opens the rename field", renamed == {"open": True, "focused": True}, str(renamed))
            page.keyboard.press("Escape")

            # T17
            page.evaluate("(id) => __card(id).querySelector('.ProseMirror')?.blur()", a)
            page.evaluate("() => { WBConsole.DORMANT_AFTER_MS = 0; }")
            page.wait_for_timeout(6500)  # one sweep (SWEEP_MS 5 s) past the timeout
            awake = page.evaluate(
                "(id) => ({ onTop: WBNotes.onTopNow() === id, asleep: !!__card(id)._noteAsleep,"
                " editor: !!__card(id)._noteEditor })",
                a,
            )
            page.evaluate("() => { WBConsole.DORMANT_AFTER_MS = 15000; }")
            check("T17 an idle card on top keeps its editor", awake == {"onTop": True, "asleep": False, "editor": True},
                  str(awake))
            page.evaluate("() => WBNotes.putBack()")

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
                and abs(band["width"] - (band["clientWidth"] - 16)) <= 2
                and abs(band["height"] - band["clientHeight"] * 0.5) <= 3,
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
