"""A detach carries a never-saved note's text (issue #475) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7482.

The fixture: one console `w-a` inside the fence `f-det`, and an empty fence
`f-two`. Notes are created through `WBNotes.create` and moved into a fence.

D7 a console closed in the popup does not stop a note's name reaching the desk
D1 a new note typed into and detached at once shows its text in the popup
D2 the popup's first save writes exactly one `.note` file, and the shell
   records its path on the desk
D3 after re-attach the card on the stage shows the same text
D4 a popup closed before its first save: after re-attach the stage card shows
   the text and saves it to one file
D6 a detach while the shell is naming the note still gives one file
D5 no `draft` field ever reaches `/api/desk`

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python crates/ralphy-daemon/tests/wb_note_detach_draft.py   (exit 0 = all pass)
"""

import json
import os
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import wb_columns_473 as T  # noqa: E402  the daemon and fixture helpers
from playwright.sync_api import sync_playwright  # noqa: E402

sys.stdout.reconfigure(encoding="utf-8")

PORT = 7482
T.PORT = PORT
T.BASE = BASE = f"http://127.0.0.1:{PORT}/"
SH = T.SH
VIEW = {"width": 1600, "height": 1000}
F_DET = {"left": 40, "top": 40, "width": 700, "height": 500}
F_TWO = {"left": 800, "top": 40, "width": 600, "height": 500}
W_A = {"left": 60, "top": 80, "width": 360, "height": 240}
N_IN_DET = {"left": 460, "top": 120, "width": 240, "height": 180}
N_IN_TWO = {"left": 860, "top": 120, "width": 240, "height": 180}

results = []
drafts_seen = []


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
        "ts = 100\n"
        + T.rect_toml(W_A)
        + "\n\n[[fences]]\n"
        'id = "f-det"\n'
        'name = "det"\n'
        "ts = 200\n" + T.rect_toml(F_DET) + "\n"
        "\n[[fences]]\n"
        'id = "f-two"\n'
        'name = "two"\n'
        "ts = 201\n" + T.rect_toml(F_TWO) + "\n"
    )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def desk_notes():
    raw = T.desk_raw()
    if "draft" in json.dumps(raw) or "claim" in json.dumps(raw.get("notes", [])):
        drafts_seen.append(True)
    return {n["id"]: n for n in raw.get("notes", [])}


def poll_note(nid, predicate, timeout=8):
    deadline = time.time() + timeout
    got = None
    while time.time() < deadline:
        try:
            got = desk_notes().get(nid)
        except Exception:
            got = None
        if predicate(got):
            return got
        time.sleep(0.3)
    return got


def note_files(page, fx, slug, marker):
    """Every `.note` file in the fixture repo whose text holds `marker`. The
    file is encoded, so the text is read back through the daemon's
    `note.read`."""
    out = []
    for p in Path(fx).rglob("*.note"):
        rel = p.relative_to(fx).as_posix()
        text = page.evaluate(
            """([repo, path]) => WBDaemon.observe('note.read', { repo, path })
                 .then((r) => (r && typeof r.markdown === 'string' ? r.markdown : ''))""",
            [slug, rel],
        )
        if marker in text:
            out.append(rel)
    return sorted(out)


def boot(page):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    page.wait_for_function(
        "() => { const w = document.querySelector('#stage .session-window');"
        " return !!w && w.clientWidth > 0; }",
        timeout=20000,
    )
    page.evaluate("() => { window.__card = (id) => WBNotes.cardEl(id); }")
    page.wait_for_timeout(600)


def new_note(page, slug, rect, text=None):
    """A note placed in `rect`. With `text`, typed and NOT saved."""
    nid = page.evaluate(
        """([repo, rect]) => { const ws = document.getElementById('workspace');
          const id = WBNotes.create({ repo, viewport: { width: ws.clientWidth, height: ws.clientHeight },
                                      offset: { left: ws.scrollLeft, top: ws.scrollTop } });
          WBConsole.saveNotes(WBConsole.notes().map((n) => n.id === id ? { ...n, rect, ts: Date.now() } : n));
          WBNotes.render(); return id; }""",
        [slug, rect],
    )
    page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=nid, timeout=15000)
    if text is not None:
        page.evaluate("(id) => __card(id)._noteEditor.dom().focus()", nid)
        page.keyboard.type(text)
    return nid


def popup_card_text(popup, nid, timeout=15000):
    popup.wait_for_function("(id) => !!window.WBNotes?.cardEl(id)?._noteEditor", arg=nid, timeout=timeout)
    return popup.evaluate("(id) => WBNotes.cardEl(id)._noteEditor.getMarkdown()", nid)


def stage_text(page, nid):
    page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=nid, timeout=15000)
    page.wait_for_timeout(300)
    return page.evaluate("(id) => __card(id)._noteEditor.getMarkdown()", nid)


def detach(ctx, page, fence):
    with ctx.expect_page(timeout=10000) as info:
        page.evaluate("(id) => WBConsole.detachFence(id)", fence)
    return info.value


def main():
    d = tempfile.mkdtemp(prefix="wbdraft_")
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
            boot(page)

            # D7: a note with no file in the console's fence. The popup's
            # timer is held, the console is closed there first, and only then
            # does the card save: the name must still reach the desk.
            z = new_note(page, slug, N_IN_DET, "Draft zero")
            popup = detach(ctx, page, "f-det")
            popup.wait_for_selector("#stage .session-window", timeout=15000)
            popup_card_text(popup, z)
            popup.evaluate("(id) => clearTimeout(WBNotes.cardEl(id)._noteTimer)", z)
            popup.locator("#stage .session-window .session-close").first.click()
            T.confirm(popup)
            popup.wait_for_function("() => !document.querySelector('#stage .session-window')", timeout=5000)
            popup.wait_for_timeout(500)
            popup.evaluate("() => WBNotes.flushAll()")
            files = []
            deadline = time.time() + 8
            while time.time() < deadline and not files:
                files = note_files(page, fx, slug, "Draft zero")
                time.sleep(0.3)
            rec = poll_note(z, lambda n: bool(n and n.get("path")))
            check("D7 the note is still on the desk after a console closed in the popup", rec is not None)
            check(
                "D7 its name reached the desk",
                bool(rec) and len(files) == 1 and rec.get("path") == files[0],
                f"{rec and rec.get('path')} {files}",
            )
            popup.close()
            page.wait_for_timeout(800)
            page.evaluate("() => WBConsole.reattachFence('f-det')")
            check("D7 the note is back on the stage with its text", "Draft zero" in stage_text(page, z))

            # D1: typed, never saved, detached at once.
            u = new_note(page, slug, N_IN_TWO, "Draft one")
            popup = detach(ctx, page, "f-two")
            text = popup_card_text(popup, u)
            check("D1 the popup's card shows the unsaved text", "Draft one" in text, repr(text))

            # D2
            popup.wait_for_function(
                "(id) => { const el = window.WBNotes?.cardEl(id); return !!el && !el._noteDirty && !el._noteInFlight; }",
                arg=u,
                timeout=10000,
            )
            rec = poll_note(u, lambda n: bool(n and n.get("path")))
            files = note_files(page, fx, slug, "Draft one")
            check("D2 exactly one .note file holds the text", len(files) == 1, str(files))
            check(
                "D2 the shell recorded that path on the desk",
                bool(rec) and files and rec.get("path") == files[0],
                str(rec and rec.get("path")),
            )

            # D3
            popup.close()
            page.wait_for_timeout(800)
            page.evaluate("() => WBConsole.reattachFence('f-two')")
            check("D3 after re-attach the stage card shows the text", "Draft one" in stage_text(page, u))
            page.wait_for_timeout(1500)
            check("D3 still one file", len(note_files(page, fx, slug, "Draft one")) == 1, str(note_files(page, fx, slug, "Draft one")))

            # D4: the popup closes before its first save.
            v = new_note(page, slug, dict(N_IN_TWO, top=340), "Draft two")
            popup = detach(ctx, page, "f-two")
            popup.close()
            page.wait_for_timeout(1200)
            page.evaluate("() => WBConsole.reattachFence('f-two')")
            check("D4 the stage card shows the text", "Draft two" in stage_text(page, v))
            page.wait_for_function(
                "(id) => { const el = __card(id); return !!el && !el._noteDirty && !el._noteInFlight"
                " && !!WBConsole.notes().find((n) => n.id === id)?.path; }",
                arg=v,
                timeout=10000,
            )
            files = note_files(page, fx, slug, "Draft two")
            check("D4 the text is saved to one file", len(files) == 1, str(files))

            # D6: the shell has started naming when the fence leaves.
            w = new_note(page, slug, dict(N_IN_TWO, left=1120), "Draft three")
            popup = None
            with ctx.expect_page(timeout=10000) as info:
                naming = page.evaluate(
                    """async (id) => { const el = __card(id); WBNotes.flushAll();
                      for (let i = 0; i < 50 && !el._noteNaming && !el._noteClaim; i++) await Promise.resolve();
                      const busy = !!el._noteNaming || !!el._noteClaim || !!el._noteInFlight;
                      WBConsole.detachFence('f-two'); return busy; }""",
                    w,
                )
            popup = info.value
            check("D6 the shell was naming the note at the detach", naming)
            popup_card_text(popup, w)
            popup.wait_for_function(
                "(id) => { const el = window.WBNotes?.cardEl(id); return !!el && !el._noteDirty && !el._noteInFlight; }",
                arg=w,
                timeout=10000,
            )
            page.wait_for_timeout(1500)
            files = note_files(page, fx, slug, "Draft three")
            check("D6 exactly one .note file holds the text", len(files) == 1, str(files))
            rec = poll_note(w, lambda n: bool(n and n.get("path")))
            check("D6 the desk path is that file", bool(rec) and files and rec.get("path") == files[0], str(rec))
            popup.close()
            page.wait_for_timeout(800)

            # D5
            desk_notes()
            check("D5 no draft field ever reached /api/desk", not drafts_seen)
            b.close()
    finally:
        T.stop(proc)
    passed = sum(results)
    print(f"\n{passed}/{len(results)} passed")
    sys.exit(0 if results and all(results) else 1)


if __name__ == "__main__":
    main()
