// The note card's editor: Crepe mounted over the body, the dress that puts the
// card's look back on what the editor answers, and the autosave that writes it
// (ADR-0064 §§6, 7). `createNotes` builds it with the card's closure names as
// typed deps; the text of each function is the one it had in `wb-notes.ts`.
import { WBFail } from "./wb-fail.ts";
import { removeNote } from "./wb-desk-folds.ts";
import { ADD_TITLE, DEFAULT_DIR, EMPTY_TITLE_HINT, REMOVE_TITLE, SAVE_AFTER_MS, applyLook, bodyOf, noteSlug, savedLabel, stampName, styleOf, titleFieldOf, titleOf, veiledOf, withHeader, withVeil } from "./wb-notes-folds.ts";
import { sendDocument } from "./wb-events.ts";
import type { CardHost, DeskNote, NoteCard, NoteEditor, NoteSource } from "./wb-types.d.ts";

// The one surface `vendor-build/crepe/entry.js` exports. It is set by a script
// the shell and the detached fence page load, so it may not be there.
export type CrepeLean = {
  create(options: {
    root: HTMLElement;
    value: string;
    readonly: boolean;
    placeholder: string;
    titleLabels: { add: string; remove: string; heading: string };
    onChange: (markdown: string) => void;
  }): Promise<NoteEditor>;
};

/** What `createNotes` hands the editor: the card's closure names. */
export type NoteEditorDeps = {
  /** The page's window: `CrepeLean` and the daemon's `observe` and `write`. */
  window: Window & { CrepeLean?: CrepeLean };
  /** The page's document. */
  document: Document;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write one card's record back, keeping the rest of the collection. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** Put the desk's cards on the stage. */
  render: () => void;
  /** Repaint the card's index from its document. */
  paintIndex: (el: NoteCard) => void;
  /** The veil's panel, shown in place of an editor. */
  veilPanel: () => HTMLElement;
  /** Paint the veil on one card. */
  paintVeil: (el: NoteCard) => void;
  /** True in a detached fence's popup, where the document holds a fragment of the plane. */
  fragment: () => boolean;
};

export function createNoteEditor(deps: NoteEditorDeps) {
  const { window, document, consoleHost, recordOf, patch, render, paintIndex, veilPanel, paintVeil } = deps;

  // The editor answers the BODY — it never sees the front matter — so every
  // value that comes back out of it is dressed in the card's look before it
  // becomes the document. Reading the look off the card and not off `next` is
  // the point: `styleOf("")` is the default, and dressing an editor's answer
  // with that would silently reset a restyled note on the first keystroke.
  function dress(el: NoteCard, body: string) {
    // The NAME comes off the card's document, not off `body`: the editor never
    // sees the header, so `withStyle` here would read the title out of the
    // body it was handed and find nothing — and rewrite the field away on the
    // first keystroke. The FIELD and not `titleOf`, for the mirror reason: a
    // legacy note's heading is still in the body being typed into, and copying
    // it up would print the name twice until the next retitle.
    const title = titleFieldOf(el._noteMarkdown) ?? "";
    return withHeader(
      body,
      title,
      { tone: el._noteTone, fill: el._noteFill, ink: el._noteInk },
      veiledOf(el._noteMarkdown),
    );
  }

  // Mount Crepe over the card's body with `markdown` (front matter stripped —
  // the look is chrome, not text the operator edits). Re-entrant: a retitle
  // hands the editor a document it did not write, so the live view is given
  // back FIRST — two ProseMirror views over one body is the leak dormancy
  // exists to prevent, arriving by another door.
  function mountEditor(el: NoteCard, markdown: string): Promise<NoteEditor | null> {
    const body = el.querySelector<HTMLElement>(".note-body");
    if (!body || !window.CrepeLean) return Promise.resolve(null);
    const previous = el._noteEditor;
    el._noteEditor = null;
    try {
      previous?.destroy();
    } catch {}
    el._noteMarkdown = markdown;
    // A card with an editor is AWAKE by definition — `wakeCard` is not the
    // only door any more, a retitle remounts too, and a card left flagged
    // asleep with a live view would never be given back again.
    el._noteAsleep = false;
    const style = styleOf(markdown);
    applyLook(el, style);
    body.textContent = "";
    // A VEILED CARD MOUNTS NO EDITOR (ADR-0064 §8 as amended). Not a blur and
    // not `display: none`: the text is never put into the document at all, and
    // the markdown held here is cut down to its header, so the tab does not
    // carry the body either. Revealing RE-READS the file — which is also what
    // waking from dormancy does, and for the same reason.
    if (veiledOf(markdown) && !el._noteRevealed) {
      el._noteMarkdown = withVeil(withHeader("", titleFieldOf(markdown) ?? "", style), true);
      body.append(veilPanel());
      paintVeil(el);
      paintTitle(el);
      paintState(el, "");
      return Promise.resolve(null);
    }
    paintVeil(el);
    // Marked here and cleared on teardown: `create()` resolves a tick or two
    // later, and a card closed, detached or evicted in between would otherwise
    // leave a live ProseMirror view with its listeners on a detached node —
    // the exact leak dormancy exists to prevent.
    el._noteGone = false;
    return window.CrepeLean.create({
      root: body,
      value: bodyOf(markdown),
      // Never read-only from the lock: a locked card is pinned, not frozen.
      readonly: false,
      placeholder: "Write a note…",
      titleLabels: { add: ADD_TITLE, remove: REMOVE_TITLE, heading: EMPTY_TITLE_HINT },
      onChange: (next) => {
        // Milkdown reports its own value back on mount too; a change that is
        // not a change must not mark the card dirty, or every card would
        // autosave itself once per reload.
        if (next === bodyOf(el._noteMarkdown)) return;
        el._noteMarkdown = dress(el, next);
        markDirty(el);
        paintTitle(el);
      },
    })
      .then((editor) => {
        if (el._noteGone) {
          try {
            editor?.destroy();
          } catch {}
          return null;
        }
        el._noteEditor = editor;
        paintTitle(el);
        // The footer says when this note last landed, from the moment it
        // opens — not only after the card has saved it once itself.
        if (!el._noteDirty) paintState(el, "");
        if (el._noteFocusOnMount) {
          el._noteFocusOnMount = false;
          try {
            editor?.dom()?.focus();
          } catch {}
        }
        return editor;
      })
      .catch((err: Error) => {
        paintState(el, "Could not start the editor: " + String(err?.message || err));
        return null;
      });
  }

  // Read the note's file into the card. A missing file is a STATE, not a
  // disappearance: the card stays, says so, and — §11 as amended 2026-09-22 —
  // keeps an editor, because the way back from that state is the operator
  // writing in it.
  function loadInto(el: NoteCard, record: NoteSource) {
    if (!record.path) return mountEditor(el, dress(el, ""));
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: record.path }, record.checkout),
    )
      .then((reply) => {
        if (WBFail.isError(reply)) {
          const reason = WBFail.message(reply, "Could not read the file.");
          // A file that is not ours is not a missing note — it is someone
          // else's file under our extension (ADR-0064 §11). The card goes and
          // the shell says so; keeping it would put an editor over bytes and
          // the first autosave would overwrite them.
          if (reason === "not a note") {
            consoleHost?.saveNotes(removeNote(consoleHost?.notes() || [], record.id));
            render();
            sendDocument(document, "workbench:open-request", {
              project: record.repo!,
              path: record.path!,
              checkout: record.checkout ?? null,
              as: "bytes",
            });
            return null;
          }
          // The reason in the footer, beside the path that footer already
          // shows — and the whole of it on hover, because `.note-state` is a
          // narrow box and "not found" alone is the half that matters.
          const said = WBFail.why(reply, "the file could not be read");
          paintMissing(el, said, `${record.path} — ${said}`);
          // An editor over what the card still holds — the unsaved text if
          // there is any, and an empty document if the read is all this card
          // ever had. Typing in it makes the card dirty, and a dirty card
          // writes its file back. Without this the missing state is a dead
          // end: a red line where the note was and no way to put it back.
          if (el._noteEditor) return null;
          // The line is repainted AFTER the mount: mounting an editor ends by
          // clearing the footer, and the card would go translucent while
          // saying nothing about why.
          return mountEditor(el, el._noteMarkdown || dress(el, "")).then((mounted) => {
            paintMissing(el, said, `${record.path} — ${said}`);
            return mounted;
          });
        }
        el.classList.remove("missing");
        // After a reload the card remembers no save of its own, and this is
        // the only place the answer exists (`note.read` carries it).
        el._noteSavedAt = Number(reply.modified) || null;
        return mountEditor(el, reply.markdown || "");
      })
      .catch((err: Error) => {
        paintMissing(el, WBFail.cause({ message: err?.message }, "The daemon did not answer."));
        return null;
      });
  }

  // The card says its file is gone — in the FOOTER, and in the dashed border
  // the `missing` class draws. The body is left alone: a read that fails must
  // not take the text with it, and emptying it is the one irreversible thing
  // this state can do. Measured 2026-09-22 from the operator's screenshot — a
  // card in a fence went translucent and blank while pointing at a path no
  // file was ever written to, and what it blanked had never reached the disk.
  function paintMissing(el: NoteCard, reason: string, full?: string) {
    el.classList.add("missing");
    paintState(el, reason);
    const state = el.querySelector<HTMLElement>(".note-state");
    if (state) state.title = full || reason;
  }

  // The head, repainted from the document: both the name and whether there is
  // an index are folds of `_noteMarkdown`, so every caller that has one has
  // the other.
  function paintTitle(el: NoteCard) {
    const title = el.querySelector<HTMLElement>(".note-title");
    if (title) title.textContent = titleOf(el._noteMarkdown, "Untitled note");
    paintIndex(el);
  }

  // `text` is the momentary word — "…", "Saved", a refusal. EMPTY means idle,
  // and idle is where the last-save stamp lives: the footer is the one place
  // that can answer "when did this land" without opening the file's
  // properties (asked 2026-09-22).
  function paintState(el: NoteCard, text?: string) {
    const state = el.querySelector<HTMLElement>(".note-state");
    if (!state) return;
    if (text) {
      state.textContent = text;
      state.title = "";
      return;
    }
    state.textContent = savedLabel(el._noteSavedAt);
    state.title = el._noteSavedAt ? new Date(el._noteSavedAt).toLocaleString() : "";
  }

  // Ask the EDITOR what it holds, rather than trusting that its change
  // notification has arrived. MEASURED: Milkdown's `markdownUpdated` lands a
  // tick or more after the keystroke, so a flush that sampled `_noteDirty`
  // right after the last character — a close, a `Ctrl+S`, a `pagehide` — saw a
  // clean card and wrote nothing, and the card was torn down before the
  // listener could fire. Every flush point goes through here first, which makes
  // the editor the source of truth and the listener only the thing that starts
  // the 800 ms clock.
  function syncFromEditor(el: NoteCard) {
    if (!el._noteEditor) return;
    let next: string;
    try {
      next = el._noteEditor.getMarkdown();
    } catch {
      return;
    }
    if (typeof next !== "string" || next === bodyOf(el._noteMarkdown)) return;
    el._noteMarkdown = dress(el, next);
    el._noteDirty = true;
    paintTitle(el);
  }

  function markDirty(el: NoteCard) {
    el._noteDirty = true;
    paintState(el, "…");
    clearTimeout(el._noteTimer);
    el._noteTimer = setTimeout(() => {
      el._noteTimer = null;
      flush(el);
    }, SAVE_AFTER_MS);
  }

  // Write now, whatever the timer was going to do. Last-writer-wins by design
  // (ADR-0064 §7): there is one operator and no live sync, so a second page
  // editing the same note resolves by overwriting — and the card that lost
  // re-reads the file on its next wake.
  //
  // SERIALISED PER CARD, and that is load-bearing: `WBDaemon.observe` opens a
  // socket per call and orders nothing, and this card has four callers (the
  // debounce, `focusout`, `Ctrl+S`, `flushAll`). Two overlapping writes can
  // land oldest-last, and the newer reply has already cleared the dirty flag —
  // the card would then show text the file does not hold, with nothing
  // scheduled to fix it. `wb-desk-sink.ts` chains its writes for exactly
  // this reason and this is the same shape.
  function flush(el: NoteCard): Promise<void> {
    clearTimeout(el._noteTimer);
    el._noteTimer = null;
    syncFromEditor(el);
    // DIRTY IS THE WHOLE TEST, and `missing` deliberately is not (ADR-0064 §11
    // as amended 2026-09-22, the operator's call): a card whose file is gone
    // but which holds an edit the operator typed writes it back and recreates
    // the file. That is not "behind their back" — it is the text they are
    // looking at. A missing card that is CLEAN holds only a stale copy of a
    // file somebody deleted, and `!dirty` already refuses it.
    if (!el._noteDirty) return Promise.resolve();
    el._noteWrite = (el._noteWrite || Promise.resolve()).catch(() => {}).then(() => writeNow(el));
    return el._noteWrite;
  }

  function writeNow(el: NoteCard): Promise<void> {
    // Re-read EVERYTHING here: this runs at the tail of the chain, and the card
    // may have been saved, closed or emptied while it waited.
    if (!el._noteDirty) return Promise.resolve();
    // Handed to a detach popup (`draftOf`): the popup's card is now the only
    // writer of this unnamed note. A write already sent still lands, at the
    // name the popup reuses (`claim`); nothing queued after it does.
    if (el._noteHandedOff) return Promise.resolve();
    // `_noteOrphan` is the record of a card whose record left the desk (see
    // `render`), or, in a popup, the snapshot record: the file is still named,
    // so the text can still land. Its path wins over a desk record that has
    // none, because this document's desk can lag behind the name it chose.
    const found = recordOf(el.dataset.noteId);
    const orphan = el._noteOrphan;
    const record =
      found && !found.path && orphan?.path ? { ...found, path: orphan.path } : found || orphan;
    if (!record) return Promise.resolve();
    const markdown = el._noteMarkdown;
    return namePath(el, record, markdown).then((path) => {
      if (!path) return;
      // Handed off while the name probe was running: see above.
      if (el._noteHandedOff) return;
      // A popup tells the opener the name BEFORE the write. If the popup
      // closes with this write in flight, the file exists but no name report
      // follows, and the opener must write to this same name on re-attach.
      if (deps.fragment() && !record.path && el._noteClaimSent !== path) {
        el._noteClaimSent = path;
        sendDocument(document, "workbench:note-claimed", { id: record.id, claim: path });
      }
      el._noteInFlight = true;
      return window.WBDaemon.write(
        "note.write",
        window.WBDaemon.withCheckout({ repo: record.repo, path, markdown }, record.checkout),
      )
        .then((reply) => {
          el._noteInFlight = false;
          if (WBFail.isError(reply)) {
            // The text stays in the editor and the card stays dirty: the next
            // keystroke schedules another attempt, and nothing was lost.
            el.classList.add("danger");
            paintState(el, WBFail.failed(reply, "Could not save: the daemon gave no reason."));
            return;
          }
          el.classList.remove("danger");
          // The file exists NOW — including the case where it had gone and
          // this write is what brought it back, which is why the missing state
          // is cleared here and not only by a read.
          el.classList.remove("missing");
          // The record may carry its path: this is the moment a claim becomes
          // a name (see `namePath`).
          if (!record.path) {
            patch(record.id, { path });
            el._noteClaim = null;
            // With no record in this document's desk, the orphan is the one
            // that must learn the name, or the next save would name again.
            if (el._noteOrphan?.id === record.id) el._noteOrphan = { ...el._noteOrphan, path };
            // A popup's desk sink is null (ADR-0051 §8), so that patch never
            // leaves this document. The popup reports the name instead, and
            // the shell records it (ADR-0064 §8, amended for #475).
            if (deps.fragment()) {
              sendDocument(document, "workbench:note-named", { id: record.id, path });
            }
          }
          // Only for the bytes that landed: a keystroke during the write leaves
          // the card dirty, and the next debounce carries it.
          if (el._noteMarkdown === markdown) el._noteDirty = false;
          // The stamp is OUR clock and not the file's: `note.write` replies
          // with no mtime, and a second round trip to ask for one would be a
          // read per keystroke-pause. The two agree to within the write.
          el._noteSavedAt = Date.now();
          paintState(el, "Saved");
          setTimeout(() => {
            if (!el._noteDirty) paintState(el, "");
          }, 1200);
        })
        .catch((err: Error) => {
          el._noteInFlight = false;
          el.classList.add("danger");
          paintState(el, WBFail.cause({ message: err?.message }, "The daemon did not answer."));
        });
    });
  }

  // The path a save writes to. An already-named note keeps its name for good
  // (ADR-0064 §4): retitling never renames, because a silent rename breaks a
  // link, a backup and a desk record at once. An unnamed one is named HERE,
  // from the title at this moment, stepping past a name already taken.
  function namePath(el: NoteCard, record: NoteSource, markdown: string): Promise<string | null> {
    if (record.path) return Promise.resolve(record.path);
    // A name this card CLAIMED but has not written yet. The desk record is
    // patched only once bytes have landed (`writeNow`), so without this a
    // second flush would probe again and — the first write having created the
    // file — step to `-2`, orphaning it under a name nobody chose.
    if (el._noteClaim) return Promise.resolve(el._noteClaim);
    // ONE naming per card, memoised synchronously: the probe below is a round
    // trip, and a second flush entering it would race the first.
    if (el._noteNaming) return el._noteNaming;
    const dirField = el.querySelector<HTMLInputElement>(".note-dir");
    const dir = String(dirField?.value || DEFAULT_DIR)
      .trim()
      .replace(/^\/+|\/+$/g, "");
    const base = noteSlug(titleOf(markdown, "")) || stampName();
    el._noteNaming = firstFreeName(record, dir, base)
      .then((path) => {
        el._noteNaming = null;
        if (!path) {
          paintState(el, "Could not name this note.");
          return null;
        }
        // CLAIMED, not recorded. A path in the desk record is a promise that
        // a file is there: patch it before the write and a write that never
        // lands leaves the card pointing at a file nobody created — which is
        // exactly the translucent "not found" card the operator found on
        // 2026-09-22, holding text that had never reached the disk.
        el._noteClaim = path;
        paintPath(el, path);
        return path;
      })
      .catch((err: Error) => {
        // A dropped socket mid-probe must not leave the card unable to ever
        // name itself: clear the memo and say why.
        el._noteNaming = null;
        paintState(el, WBFail.cause({ message: err?.message }, "The daemon did not answer."));
        return null;
      });
    return el._noteNaming;
  }

  // `<dir>/<base>.note`, or `-2`, `-3`… past one that exists. The probe is a
  // `note.read`: a write would overwrite, and overwriting a note nobody asked
  // to touch is the one thing this must not do.
  const NAME_TRIES = 20;
  function firstFreeName(record: NoteSource, dir: string, base: string, n = 1): Promise<string | null> {
    if (n > NAME_TRIES) return Promise.resolve(null);
    const path = `${dir ? dir + "/" : ""}${base}${n === 1 ? "" : "-" + n}.note`;
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path }, record.checkout),
    ).then((reply) => {
      const reason = WBFail.isError(reply) ? WBFail.message(reply, "") : null;
      // Only "not found" means free: "not a note" is a file that exists and is
      // something else, and writing over it would destroy it.
      if (reason === "not found") return path;
      return firstFreeName(record, dir, base, n + 1);
    });
  }

  function paintPath(el: NoteCard, path: string | null | undefined) {
    const field = el.querySelector<HTMLElement>(".note-path");
    if (field) {
      field.textContent = path || "";
      field.title = path || "This note is not saved yet";
    }
    const dir = el.querySelector<HTMLInputElement>(".note-dir");
    if (dir) dir.hidden = !!path;
  }

  return { mountEditor, loadInto, paintTitle, paintState, syncFromEditor, markDirty, flush, paintPath };
}
