// The note card's save: the document dressed in the card's look, the autosave
// clock, and the write that lands it (ADR-0064 §§4, 7). `createNoteEditor`
// builds it with the paints it shares as typed deps; the text of each function
// is the one it had in `wb-notes-editor.ts`.
import { WBFail } from "./wb-fail.ts";
import {
  DEFAULT_DIR,
  SAVE_AFTER_MS,
  bodyOf,
  noteSlug,
  stampName,
  titleFieldOf,
  titleOf,
  veiledOf,
  withHeader,
} from "./wb-notes-folds.ts";
import { sendDocument } from "./wb-events.ts";
import type { DeskNote, NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNoteEditor` hands the save. */
export type NoteSaveDeps = {
  /** The page's window: the daemon's `observe` and `write`. */
  window: Window;
  /** The page's document. */
  document: Document;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write one card's record back, keeping the rest of the collection. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** True in a detached fence's popup, where the document holds a fragment of the plane. */
  fragment: () => boolean;
  /** Repaint the card's head from its document. */
  paintTitle: (el: NoteCard) => void;
  /** Say a word in the card's footer; empty means idle. */
  paintState: (el: NoteCard, text?: string) => void;
  /** Show the path of the card's file. */
  paintPath: (el: NoteCard, path: string | null | undefined) => void;
};

// The editor answers the BODY — it never sees the front matter — so every
// value that comes back out of it is dressed in the card's look before it
// becomes the document. Reading the look off the card and not off `next` is
// the point: `styleOf("")` is the default, and dressing an editor's answer
// with that would silently reset a restyled note on the first keystroke.
export function dress(el: NoteCard, body: string) {
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

export function createNoteSave(deps: NoteSaveDeps) {
  const { window, document, recordOf, patch, paintTitle, paintState, paintPath } = deps;

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
      void flush(el);
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
        .catch((err: unknown) => {
          el._noteInFlight = false;
          el.classList.add("danger");
          paintState(
            el,
            WBFail.cause(
              { message: err instanceof Error ? err.message : undefined },
              "The daemon did not answer.",
            ),
          );
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
      .catch((err: unknown) => {
        // A dropped socket mid-probe must not leave the card unable to ever
        // name itself: clear the memo and say why.
        el._noteNaming = null;
        paintState(
          el,
          WBFail.cause(
            { message: err instanceof Error ? err.message : undefined },
            "The daemon did not answer.",
          ),
        );
        return null;
      });
    return el._noteNaming;
  }

  // `<dir>/<base>.note`, or `-2`, `-3`… past one that exists. The probe is a
  // `note.read`: a write would overwrite, and overwriting a note nobody asked
  // to touch is the one thing this must not do.
  const NAME_TRIES = 20;
  function firstFreeName(
    record: NoteSource,
    dir: string,
    base: string,
    n = 1,
  ): Promise<string | null> {
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

  return { syncFromEditor, markDirty, flush };
}
