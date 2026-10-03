"""An empty note line offers a title (ADR-0064, 2026-09-27 amendment) browser
acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7482. The fixture and
the note helpers are `wb_note_on_top.py`'s.

A1  the empty line that holds the caret shows the placeholder and "Add title"
    side by side, and Crepe's own placeholder is hidden on that line
A2  "Add title" makes the line an empty `##`: the caret stays, the line reads
    "Write a title" and offers "Remove title"
A3  typing on the title removes the control; the saved note has `## <text>`
A4  Enter after a title gives a plain line, which offers "Add title" again
A5  "Remove title" turns an empty title back into a plain line
A6  Ctrl+Z after "Add title" gives the plain line back
A7  a line with text never shows the control; without the focus it is hidden
    and the placeholder shows as before
A8  an empty list item shows no control

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/notes/wb_note_title.py   (exit 0 = all pass)
"""

import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "notes"))
import wb_note_on_top as N  # noqa: E402  the fixture and note helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

T = N.T
PORT = 7482
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SHOT = os.path.join(T.REPO_ROOT, ".ralphy", "screenshots", "note-add-title-2026-09-27.png")
VIEW = {"width": 1600, "height": 1000}
FLOOR = 19  # every check above the floor check; pinned after the first green run

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(f"[{'PASS' if ok else 'FAIL'}] {name} {detail}", flush=True)


# The block that holds the caret, and what the title control shows on it.
STATE = """(id) => {
  const card = __card(id);
  const pm = card._noteEditor.dom();
  const sel = getSelection();
  let block = sel.anchorNode;
  while (block && block.parentElement !== pm) block = block.parentElement;
  const toggle = card.querySelector('.note-title-toggle');
  const shown = !!toggle && getComputedStyle(toggle).display !== 'none';
  const hint = toggle?.querySelector('.note-title-hint');
  const act = toggle?.querySelector('.note-title-act');
  const overlap = (a, b) => { if (!a || !b) return null;
    const r = a.getBoundingClientRect(), s = b.getBoundingClientRect();
    return r.right > s.left && s.right > r.left && r.bottom > s.top && s.bottom > r.top; };
  return {
    tag: block?.tagName || null,
    empty: !!block && block.textContent.replace(hint?.textContent || '', '').replace(act?.textContent || '', '') === '',
    focused: document.activeElement === pm,
    shown,
    inBlock: !!toggle && !!block && block.contains(toggle),
    hint: hint?.textContent || null,
    act: act?.textContent || null,
    overlap: overlap(hint, act),
    before: block ? getComputedStyle(block, '::before').content : null,
  };
}"""


def state(page, nid):
    return page.evaluate(STATE, nid)


def click_act(page, nid):
    page.locator(f".note-card[data-note-id='{nid}'] .note-title-act").click()
    page.wait_for_timeout(150)


def main():
    d = tempfile.mkdtemp(prefix="wbtitle_")
    fx = T.make_fixture_repo()
    slug = T.register_fixture(d, fx)
    N.write_desk(d, slug)
    proc = T.launch(d)
    errors = []
    try:
        if not T.wait_listening(BASE):
            check("daemon listening", False)
            return
        with sync_playwright() as p:
            b = p.chromium.launch()
            page = b.new_context(viewport=dict(VIEW)).new_page()
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(BASE)
            N.boot(page, slug)
            nid = N.create_note(page, slug, "Intro")
            # A card of its own size, off the maximized console.
            page.evaluate(f"() => {{ {T.SH}.activate('consoles'); }}")
            page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", nid)
            page.keyboard.press("Control+End")
            page.keyboard.press("Enter")
            page.wait_for_timeout(200)

            # A1 --------------------------------------------------------------
            s = state(page, nid)
            check("A1 an empty plain line shows the control on that line",
                  s["tag"] == "P" and s["shown"] and s["inBlock"], str(s))
            check("A1 it reads the placeholder and 'Add title'",
                  s["hint"] == "Write a note…" and s["act"] == "Add title", str(s))
            check("A1 the two do not overlap, and Crepe's placeholder is hidden there",
                  s["overlap"] is False and s["before"] in ("none", "normal"), str(s))
            # A touch on the hint reaches the empty line, so a press-and-hold
            # there opens the browser's Paste menu (reported on an iPhone).
            hit = page.evaluate(
                "(id) => { const t = __card(id).querySelector('.note-title-toggle');"
                " const at = (el) => { const r = el.getBoundingClientRect();"
                "  return document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); };"
                " const onHint = at(t.querySelector('.note-title-hint'));"
                " const act = t.querySelector('.note-title-act');"
                " return { hint: onHint?.tagName, act: at(act) === act }; }",
                nid,
            )
            check("A1 a touch on the hint reaches the line; 'Add title' still takes one",
                  hit["hint"] == "P" and hit["act"], str(hit))
            os.makedirs(os.path.dirname(SHOT), exist_ok=True)
            page.locator(f".note-card[data-note-id='{nid}']").screenshot(path=SHOT)

            # A2 --------------------------------------------------------------
            click_act(page, nid)
            s = state(page, nid)
            check("A2 'Add title' makes the line an empty title", s["tag"] == "H2" and s["empty"], str(s))
            check("A2 the editor keeps the focus", s["focused"], str(s))
            check("A2 the title reads 'Write a title' and offers 'Remove title'",
                  s["shown"] and s["hint"] == "Write a title" and s["act"] == "Remove title", str(s))

            # A3 --------------------------------------------------------------
            page.keyboard.type("Plan")
            page.wait_for_timeout(150)
            s = state(page, nid)
            check("A3 typing on the title removes the control", not s["shown"] and s["tag"] == "H2", str(s))
            page.keyboard.press("Control+s")
            page.wait_for_function("(id) => !__card(id)._noteDirty", arg=nid, timeout=10000)
            md = page.evaluate("(id) => __card(id)._noteMarkdown", nid)
            check("A3 the note holds `## Plan`", "\n## Plan" in md, repr(md))

            # A4 --------------------------------------------------------------
            page.keyboard.press("Enter")
            page.wait_for_timeout(150)
            s = state(page, nid)
            check("A4 Enter after a title gives a plain line that offers 'Add title'",
                  s["tag"] == "P" and s["shown"] and s["act"] == "Add title", str(s))

            # A5 --------------------------------------------------------------
            click_act(page, nid)
            check("A5 setup: a title", state(page, nid)["tag"] == "H2")
            click_act(page, nid)
            s = state(page, nid)
            check("A5 'Remove title' gives the plain line back",
                  s["tag"] == "P" and s["act"] == "Add title" and s["focused"], str(s))

            # A6 --------------------------------------------------------------
            click_act(page, nid)
            page.keyboard.press("Control+z")
            page.wait_for_timeout(150)
            s = state(page, nid)
            check("A6 Ctrl+Z after 'Add title' gives the plain line back", s["tag"] == "P", str(s))

            # A7 --------------------------------------------------------------
            page.keyboard.type("text")
            page.wait_for_timeout(100)
            check("A7 a line with text shows no control", not state(page, nid)["shown"])
            page.keyboard.press("Enter")
            page.wait_for_timeout(100)
            check("A7 setup: an empty line shows it", state(page, nid)["shown"])
            page.evaluate("(id) => __card(id)._noteEditor.dom().blur()", nid)
            page.wait_for_timeout(150)
            blurred = page.evaluate(
                "(id) => { const t = __card(id).querySelector('.note-title-toggle');"
                " const p = t?.closest('p');"
                " return { shown: !!t && getComputedStyle(t).display !== 'none',"
                "  before: p ? getComputedStyle(p, '::before').content : null }; }",
                nid,
            )
            check("A7 without the focus the control is hidden and the placeholder shows",
                  not blurred["shown"] and blurred["before"] not in (None, "none", "normal"), str(blurred))

            # A8 --------------------------------------------------------------
            page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", nid)
            page.keyboard.type("- item")
            page.keyboard.press("Enter")
            page.wait_for_timeout(150)
            li = page.evaluate(
                "() => { let n = getSelection().anchorNode; while (n && n.nodeType !== 1) n = n.parentNode;"
                " return !!n?.closest('li'); }"
            )
            check("A8 setup: the caret is in an empty list item", li)
            check("A8 an empty list item shows no control", not state(page, nid)["shown"])

            check("no page errors", not errors, str(errors[:3]))
            b.close()
    finally:
        T.stop(proc)


if __name__ == "__main__":
    main()
    passed = sum(results)
    print(f"{passed}/{len(results)} passed", flush=True)
    if FLOOR is not None:
        check("check floor", len(results) == FLOOR, f"{len(results)} != {FLOOR}")
    sys.exit(0 if results and all(results) else 1)
