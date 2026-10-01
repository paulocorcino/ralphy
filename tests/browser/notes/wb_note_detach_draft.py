"""A detach carries a never-saved note's text (issue #475) browser acceptance.

One Playwright pass over a REAL daemon on a scratch `RALPHY_DAEMON_DIR`, so the
operator's own desk and login policy are untouched. PORT 7482.

The fixture: one console `w-a` inside the fence `f-det`, and an empty fence
`f-two`. Notes are created through `WBNotes.create` and moved into a fence.

D7  a console closed in the popup does not stop a note's name reaching the desk
D1  a new note typed into and detached at once shows its text in the popup
D2  the popup's first save writes exactly one `.note` file, and the shell
    records its path on the desk
D10 a second save in the popup writes the same file
D3  after re-attach the card on the stage shows the same text
D4  a popup closed before its first save: after re-attach the stage card shows
    the text and saves it to one file
D6  a detach while the shell is naming the note still gives one file
D6b a detach while the shell's first write is in flight: the popup writes the
    same name, and there is one file
D11 a popup closed while its first write is in flight: the re-attach reads the
    file the popup chose, and there is one file
D12 a popup card whose record is not in the popup's desk survives a render
    there, and its save still records the name
D13 a veiled draft is written whole, not as its header only
D14 a card built from a draft that came home carries that draft before its
    editor mounts
D8  after the opener reloads, the name report reaches it over the channel
D9  a name report the opener missed during a reload is recorded when the popup
    is adopted again
D5  no `draft` or `claim` field is ever sent to or served by `/api/desk`

The daemon is stopped by its own subprocess handle, NEVER by name (`ralphy.exe`
doubles as the orchestrator on this host).

Run: cargo build -p ralphy-cli --bin ralphy
     python tests/browser/notes/wb_note_detach_draft.py   (exit 0 = all pass)
"""

import json
import os
import sys
import tempfile
import time
import traceback
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "columns"))
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
leaks = []


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
        'id = "f-two'
        '"\n'
        'name = "two"\n'
        "ts = 201\n" + T.rect_toml(F_TWO) + "\n"
    )
    Path(daemon_dir, "desk.toml").write_bytes(out.encode("utf-8"))


def note_leaks(notes, where):
    for n in notes or []:
        if isinstance(n, dict) and ("draft" in n or "claim" in n):
            leaks.append(f"{where}: {sorted(n)}")


def watch_desk_writes(request):
    """Every desk PUT any page sends, checked for a snapshot-only field. The
    daemon drops unknown fields, so reading `/api/desk` alone cannot see one."""
    if request.method != "PUT" or not request.url.endswith("/api/desk"):
        return
    try:
        body = json.loads(request.post_data or "{}")
    except ValueError:
        return
    note_leaks(body.get("notes"), "PUT /api/desk")


def desk_notes():
    notes = T.desk_raw().get("notes", [])
    note_leaks(notes, "GET /api/desk")
    return {n["id"]: n for n in notes}


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


def read_note(page, slug, path):
    return page.evaluate(
        """([repo, path]) => WBDaemon.observe('note.read', { repo, path })
             .then((r) => (r && typeof r.markdown === 'string' ? r.markdown : ''))""",
        [slug, path],
    )


def wait_files(page, fx, slug, marker, want=1, timeout=8):
    deadline = time.time() + timeout
    files = []
    while time.time() < deadline:
        files = note_files(page, fx, slug, marker)
        if len(files) >= want:
            return files
        time.sleep(0.3)
    return files


def boot(page, console=True):
    page.wait_for_selector("[x-data]", timeout=8000)
    page.evaluate(f"() => {{ {SH}.activate('consoles'); }}")
    if console:
        page.wait_for_function(
            "() => { const w = document.querySelector('#stage .session-window');"
            " return !!w && w.clientWidth > 0; }",
            timeout=20000,
        )
    else:
        page.wait_for_function("() => !!window.WBNotes && !!window.WBConsole", timeout=20000)
    page.evaluate("() => { window.__card = (id) => WBNotes.cardEl(id); }")
    page.wait_for_timeout(600)


def reload_opener(page, fence):
    """Reload the shell and wait until it has adopted the popup that holds
    `fence` again (the `popup-here` answer to its boot broadcast)."""
    page.reload()
    boot(page, console=False)
    page.wait_for_function("(id) => WBConsole.isDetached(id)", arg=fence, timeout=10000)
    # Ask the popup, and wait for its answer: the shell hears the same
    # `popup-here` on its own channel object, in the same turn.
    answered = page.evaluate(
        """(id) => new Promise((done) => {
          const tab = JSON.parse(sessionStorage.getItem('wb.detach.v1') || '{}').tab;
          const c = new BroadcastChannel('wb.detach.v1');
          const t = setTimeout(() => { c.close(); done(false); }, 8000);
          c.onmessage = (e) => { if (e.data?.type === 'popup-here' && e.data?.fenceId === id) {
            clearTimeout(t); c.close(); done(true); } };
          c.postMessage({ type: 'origin-ping', tab, fenceId: id }); })""",
        fence,
    )
    assert answered, "the popup did not answer the reloaded opener"
    page.wait_for_timeout(300)


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


def unsaved(page, nid):
    return page.evaluate(
        """(id) => { const el = __card(id);
          return !WBConsole.notes().find((n) => n.id === id)?.path && !el._noteClaim && !el._noteNaming
            && el._noteEditor.getMarkdown().trim().length > 0; }""", nid
    )


def popup_card_text(popup, nid, timeout=15000):
    popup.wait_for_function("(id) => !!window.WBNotes?.cardEl(id)?._noteEditor", arg=nid, timeout=timeout)
    return popup.evaluate("(id) => WBNotes.cardEl(id)._noteEditor.getMarkdown()", nid)


def hold_popup_save(popup, nid):
    """Stop the popup card's first autosave, so the test decides when it runs."""
    popup_card_text(popup, nid)
    popup.evaluate("(id) => clearTimeout(WBNotes.cardEl(id)._noteTimer)", nid)


def popup_saved(popup, nid, timeout=10000):
    popup.wait_for_function(
        "(id) => { const el = window.WBNotes?.cardEl(id); return !!el && !el._noteDirty && !el._noteInFlight; }",
        arg=nid,
        timeout=timeout,
    )


def stage_text(page, nid):
    page.wait_for_function("(id) => !!__card(id)?._noteEditor", arg=nid, timeout=15000)
    page.wait_for_timeout(300)
    return page.evaluate("(id) => __card(id)._noteEditor.getMarkdown()", nid)


def detach(ctx, page, fence):
    with ctx.expect_page(timeout=10000) as info:
        page.evaluate("(id) => WBConsole.detachFence(id)", fence)
    return info.value


def reattach(page, fence):
    page.wait_for_timeout(800)
    page.evaluate("(id) => WBConsole.reattachFence(id)", fence)


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
            ctx.on("request", watch_desk_writes)
            page = ctx.new_page()
            page.goto(BASE)
            boot(page)

            # D7: a note with no file in the console's fence. The popup's
            # timer is held, the console is closed there first, and only then
            # does the card save: the name must still reach the desk.
            z = new_note(page, slug, N_IN_DET, "Draft zero")
            popup = detach(ctx, page, "f-det")
            popup.wait_for_selector("#stage .session-window", timeout=15000)
            hold_popup_save(popup, z)
            popup.locator("#stage .session-window .session-close").first.click()
            T.confirm(popup)
            popup.wait_for_function("() => !document.querySelector('#stage .session-window')", timeout=5000)
            popup.wait_for_timeout(500)
            popup.evaluate("() => WBNotes.flushAll()")
            files = wait_files(page, fx, slug, "Draft zero")
            rec = poll_note(z, lambda n: bool(n and n.get("path")))
            check("D7 the note is still on the desk after a console closed in the popup", rec is not None)
            check(
                "D7 its name reached the desk",
                bool(rec) and len(files) == 1 and rec.get("path") == files[0],
                f"{rec and rec.get('path')} {files}",
            )
            popup.close()
            reattach(page, "f-det")
            check("D7 the note is back on the stage with its text", "Draft zero" in stage_text(page, z))

            # D1: typed, never saved, detached at once.
            u = new_note(page, slug, N_IN_TWO, "Draft one")
            check("D1 precondition: the note has no file and unsaved text", unsaved(page, u))
            popup = detach(ctx, page, "f-two")
            text = popup_card_text(popup, u)
            check("D1 the popup's card shows the unsaved text", "Draft one" in text, repr(text))

            # D2
            popup_saved(popup, u)
            rec = poll_note(u, lambda n: bool(n and n.get("path")))
            files = note_files(page, fx, slug, "Draft one")
            check("D2 exactly one .note file holds the text", len(files) == 1, str(files))
            check(
                "D2 the shell recorded that path on the desk",
                bool(rec) and files and rec.get("path") == files[0],
                str(rec and rec.get("path")),
            )

            # D10: a second save in the popup goes to the same file.
            popup.evaluate("(id) => WBNotes.cardEl(id)._noteEditor.dom().focus()", u)
            popup.keyboard.press("End")
            popup.keyboard.type(" again")
            popup.wait_for_timeout(300)
            popup.evaluate("() => WBNotes.flushAll()")
            popup_saved(popup, u)
            again = wait_files(page, fx, slug, "Draft one again")
            check(
                "D10 the second save wrote the same file",
                again == files and len(note_files(page, fx, slug, "Draft one")) == 1,
                str(again),
            )

            # D3
            popup.close()
            reattach(page, "f-two")
            check("D3 after re-attach the stage card shows the text", "Draft one again" in stage_text(page, u))
            page.wait_for_timeout(1500)
            check("D3 still one file", len(note_files(page, fx, slug, "Draft one")) == 1)

            # D4: the popup closes before its first save.
            v = new_note(page, slug, dict(N_IN_TWO, top=340), "Draft two")
            popup = detach(ctx, page, "f-two")
            popup.close()
            check(
                "D4 precondition: nothing was saved before the re-attach",
                not note_files(page, fx, slug, "Draft two")
                and not (desk_notes().get(v) or {}).get("path"),
            )
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
            with ctx.expect_page(timeout=10000) as info:
                state = page.evaluate(
                    """async (id) => { const el = __card(id); WBNotes.flushAll();
                      for (let i = 0; i < 50 && !el._noteNaming; i++) await Promise.resolve();
                      const s = { naming: !!el._noteNaming, claim: el._noteClaim || null,
                                  inFlight: !!el._noteInFlight };
                      WBConsole.detachFence('f-two'); return s; }""",
                    w,
                )
            popup = info.value
            check("D6 the shell was probing for a name, with none chosen yet", state == {
                "naming": True, "claim": None, "inFlight": False}, str(state))
            popup_card_text(popup, w)
            popup_saved(popup, w)
            page.wait_for_timeout(1500)
            files = note_files(page, fx, slug, "Draft three")
            check("D6 exactly one .note file holds the text", len(files) == 1, str(files))
            rec = poll_note(w, lambda n: bool(n and n.get("path")))
            check("D6 the desk path is that file", bool(rec) and files and rec.get("path") == files[0], str(rec))
            popup.close()
            reattach(page, "f-two")

            # D6b: the shell's first write is on its way when the fence leaves.
            x = new_note(page, slug, dict(N_IN_TWO, left=1120, top=340), "Draft four")
            with ctx.expect_page(timeout=10000) as info:
                state = page.evaluate(
                    """async (id) => { const el = __card(id); WBNotes.flushAll(); const t = Date.now();
                      while (!el._noteInFlight && Date.now() - t < 5000) await new Promise((r) => setTimeout(r, 0));
                      const s = { inFlight: !!el._noteInFlight, claim: el._noteClaim || null };
                      WBConsole.detachFence('f-two'); return s; }""",
                    x,
                )
            popup = info.value
            check("D6b the shell's write was in flight, with a chosen name", state["inFlight"] and state["claim"],
                  str(state))
            popup_card_text(popup, x)
            check(
                "D6b the popup's card took that name",
                popup.evaluate("(id) => WBNotes.cardEl(id)._noteClaim", x) in (state["claim"], None),
            )
            popup_saved(popup, x)
            page.wait_for_timeout(1500)
            files = note_files(page, fx, slug, "Draft four")
            check("D6b exactly one .note file, under the chosen name", files == [state["claim"]], str(files))
            rec = poll_note(x, lambda n: bool(n and n.get("path")))
            check("D6b the desk path is that name", bool(rec) and rec.get("path") == state["claim"], str(rec))
            popup.close()
            reattach(page, "f-two")

            # D11: the popup closes while its first write is in flight, and no
            # name report follows it.
            y = new_note(page, slug, dict(N_IN_DET, top=340), "Draft five")
            popup = detach(ctx, page, "f-det")
            hold_popup_save(popup, y)
            popup.evaluate("(id) => WBNotes.cardEl(id)._noteEditor.dom().focus()", y)
            popup.keyboard.press("End")
            popup.keyboard.type(" edited")
            popup.evaluate(
                """(id) => { clearTimeout(WBNotes.cardEl(id)._noteTimer); const w = WBDaemon.write;
                  WBDaemon.write = (...a) => { w(...a); return new Promise(() => {}); };
                  WBNotes.flushAll(); }""",
                y,
            )
            landed = wait_files(page, fx, slug, "Draft five edited")
            check(
                "D11 precondition: the file landed and the desk has no path",
                len(landed) == 1 and not (desk_notes().get(y) or {}).get("path"),
                str(landed),
            )
            popup.close()
            reattach(page, "f-det")
            check("D11 the stage card shows the popup's newer text", "Draft five edited" in stage_text(page, y))
            rec = poll_note(y, lambda n: bool(n and n.get("path")))
            page.wait_for_timeout(1500)
            files = note_files(page, fx, slug, "Draft five")
            check("D11 one file, and the desk points at it", files == landed and bool(rec)
                  and rec.get("path") == landed[0], f"{files} {rec and rec.get('path')}")

            # D12: the popup's desk does not hold the record.
            q = new_note(page, slug, dict(N_IN_DET, top=340, left=200), "Draft six")
            popup = detach(ctx, page, "f-det")
            hold_popup_save(popup, q)
            kept = popup.evaluate(
                """(id) => { WBConsole.saveNotes(WBConsole.notes().filter((n) => n.id !== id));
                  WBNotes.render(); return !!WBNotes.cardEl(id); }""",
                q,
            )
            check("D12 the card survives a render in the popup", kept)
            popup.evaluate("() => WBNotes.flushAll()")
            files = wait_files(page, fx, slug, "Draft six")
            rec = poll_note(q, lambda n: bool(n and n.get("path")))
            check("D12 its save records the name", len(files) == 1 and bool(rec) and rec.get("path") == files[0],
                  f"{files} {rec and rec.get('path')}")
            popup.close()
            reattach(page, "f-det")

            # D13: a veiled draft.
            vv = new_note(page, slug, dict(N_IN_DET, top=340, left=60), "Draft veiled")
            with ctx.expect_page(timeout=10000) as info:
                page.evaluate(
                    """(id) => { const el = __card(id); clearTimeout(el._noteTimer);
                      el._noteMarkdown = WBNotes.withVeil(el._noteMarkdown, true); el._noteDirty = true;
                      WBConsole.detachFence('f-det'); }""",
                    vv,
                )
            popup = info.value
            popup.wait_for_function("(id) => !!window.WBNotes?.cardEl(id)", arg=vv, timeout=15000)
            popup_saved(popup, vv)
            files = wait_files(page, fx, slug, "Draft veiled")
            body = read_note(page, slug, files[0]) if files else ""
            check("D13 the veiled draft is written whole", len(files) == 1 and page.evaluate(
                "(md) => WBNotes.veiledOf(md)", body), repr(body))
            popup.close()
            reattach(page, "f-det")

            # D14: a card built from a draft that came home is dirty at once,
            # so a detach before its editor mounts still carries the text. The
            # same tick, in the page: a detach from outside cannot land in
            # that gap reliably.
            got = page.evaluate(
                """([repo, rect]) => { const id = 'note-d14';
                  WBConsole.saveNotes(WBConsole.notes().concat([{ id, repo, rect, ts: Date.now() }]));
                  WBNotes.adoptDraft(id, 'Draft seven'); WBNotes.render();
                  const early = !__card(id)._noteEditor; const out = WBNotes.draftOf(id);
                  WBConsole.saveNotes(WBConsole.notes().filter((n) => n.id !== id)); WBNotes.render();
                  return { early, draft: out && out.draft }; }""",
                [slug, dict(N_IN_TWO, left=1000, top=230)],
            )
            check("D14 before its editor mounts, the card already carries its draft",
                  got["early"] and "Draft seven" in (got["draft"] or ""), str(got))

            # D8: the opener reloads; the report comes over the channel.
            e8 = new_note(page, slug, dict(N_IN_DET, top=200, left=200), "Draft eight")
            popup = detach(ctx, page, "f-det")
            hold_popup_save(popup, e8)
            reload_opener(page, "f-det")
            popup.evaluate("() => WBNotes.flushAll()")
            files = wait_files(page, fx, slug, "Draft eight")
            rec = poll_note(e8, lambda n: bool(n and n.get("path")))
            check("D8 the reloaded opener recorded the name", len(files) == 1 and bool(rec)
                  and rec.get("path") == files[0], f"{files} {rec and rec.get('path')}")

            # D9: the report is lost; the next adoption records it.
            popup.close()
            page.wait_for_timeout(1000)
            page.evaluate("() => WBConsole.reattachFence('f-det')")
            n9 = new_note(page, slug, dict(N_IN_DET, top=200, left=460), "Draft nine")
            popup = detach(ctx, page, "f-det")
            hold_popup_save(popup, n9)
            reload_opener(page, "f-det")
            popup.evaluate(
                """() => { const post = BroadcastChannel.prototype.postMessage;
                  BroadcastChannel.prototype.postMessage = function (m) {
                    if (m && m.type === 'popup-note-named') return; return post.call(this, m); };
                  WBNotes.flushAll(); }"""
            )
            files = wait_files(page, fx, slug, "Draft nine")
            page.wait_for_timeout(1500)
            check(
                "D9 precondition: the file landed and the report was lost",
                len(files) == 1 and not (desk_notes().get(n9) or {}).get("path"),
                str(files),
            )
            reload_opener(page, "f-det")
            rec = poll_note(n9, lambda n: bool(n and n.get("path")))
            check("D9 the adoption recorded the name", bool(rec) and files and rec.get("path") == files[0],
                  str(rec and rec.get("path")))
            popup.close()

            # D5
            desk_notes()
            check("D5 no draft or claim field was sent to or served by /api/desk", not leaks, str(leaks[:3]))
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
