// The note card's file acts: rename the file, delete it, or point the card at
// another one (ADR-0064 §§4, 11). `createNotes` builds it with the card's names
// as typed deps; the text of each function is the one it had in `wb-notes.ts`.
import { WBFail } from "./wb-fail.ts";
import { removeNote } from "./wb-desk-folds.ts";
import { baseName, dirName } from "./wb-notes-folds.ts";
import type { Messages } from "./wb-messages.ts";
import type { CardHost, DeskNote, NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the file acts: the card's and the editor's names. */
export type NoteFileDeps = {
  /** The page's window: the daemon's `write`. */
  window: Window;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The page's door for operator messages; null where a test has none. */
  messages: Messages | null;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write one card's record back, keeping the rest of the collection. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** Put the desk's cards on the stage. */
  render: () => void;
  /** Show the file path on the card. */
  paintPath: (el: NoteCard, path: string | null | undefined) => void;
  /** Say something in the card's state line. */
  paintState: (el: NoteCard, text: string) => void;
  /** Read the note's file into the card. */
  loadInto: (el: NoteCard, record: NoteSource) => unknown;
};

export function createNoteFile(deps: NoteFileDeps) {
  const { window, consoleHost, messages, recordOf, patch, render, paintPath, paintState, loadInto } = deps;

  // Point a card at another file (ADR-0064 §11's missing-file state). The
  // record is re-aimed and the file is re-read; nothing on disk is touched,
  // because there is nothing there to touch — which is why this is a separate
  // verb from the rename beside it.
  function pointElsewhere(el: NoteCard) {
    const record = recordOf(el.dataset.noteId);
    if (!record) return;
    const field = el.querySelector<HTMLInputElement>(".note-rename");
    if (!field) return;
    el._notePointing = true;
    field.value = record.path || "";
    field.hidden = false;
    const path = el.querySelector<HTMLElement>(".note-path");
    if (path) path.hidden = true;
    field.focus();
    field.select();
  }

  // Rename the FILE and follow it with the record (ADR-0064 §4: a title change
  // never renames, so this is the only way a note's name moves). The daemon's
  // `file.rename` is what reaches it, through the denylist's one carve-out.
  function renameNote(el: NoteCard) {
    const record = recordOf(el.dataset.noteId);
    if (!record?.path || record.checkout) return;
    const field = el.querySelector<HTMLInputElement>(".note-rename");
    const path = el.querySelector<HTMLElement>(".note-path");
    if (!field) return;
    field.value = baseName(record.path);
    field.hidden = false;
    if (path) path.hidden = true;
    field.focus();
    field.select();
  }
  function endRename(el: NoteCard) {
    const field = el.querySelector<HTMLInputElement>(".note-rename");
    if (!field || field.hidden) return;
    field.hidden = true;
    el._notePointing = false;
    const path = el.querySelector<HTMLElement>(".note-path");
    if (path) path.hidden = false;
  }
  function commitRename(el: NoteCard) {
    const record = recordOf(el.dataset.noteId);
    const field = el.querySelector<HTMLInputElement>(".note-rename");
    const next = String(field?.value || "").trim();
    const pointing = !!el._notePointing;
    endRename(el);
    if (!record || !next) return;
    if (pointing) pointAt(el, record, next);
    else renameFile(el, record, next);
  }

  // A whole rel path, not a file name: the card is being aimed somewhere
  // else in the checkout, and the read decides whether there is a note
  // there. Identity is `(repo, checkout, path)` (ADR-0064 §4) and this is
  // the one gesture that could break it: two cards over one file would be
  // two editors autosaving it, each overwriting the other with stale text
  // and neither told.
  function pointAt(el: NoteCard, record: NoteSource, next: string) {
    const taken = (consoleHost?.notes?.() || []).find(
      (n) =>
        n.id !== record.id &&
        n.repo === record.repo &&
        (n.checkout ?? null) === (record.checkout ?? null) &&
        n.path === next,
    );
    if (taken) {
      paintState(el, "Another note already uses that file.");
      return;
    }
    patch(record.id, { path: next });
    paintPath(el, next);
    el.classList.remove("missing");
    loadInto(el, { ...record, path: next });
  }

  // A new file name for the note, in the same directory.
  function renameFile(el: NoteCard, record: NoteSource, next: string) {
    if (!record.path || next === baseName(record.path)) return;
    // Always a `.note`: the verbs refuse anything else, and a note renamed out
    // of its extension would be a file nothing can open (ADR-0064 §5).
    const name = next.endsWith(".note") ? next : next + ".note";
    const dir = dirName(record.path);
    const to = (dir ? dir + "/" : "") + name;
    window.WBDaemon.write("file.rename", { repo: record.repo, path: record.path, to })
      .then((reply) => {
        if (WBFail.isError(reply)) {
          paintState(el, WBFail.failed(reply, "Could not rename: the daemon gave no reason."));
          return;
        }
        patch(record.id, { path: to });
        paintPath(el, to);
        paintState(el, "Renamed");
      })
      .catch((err: Error) => paintState(el, WBFail.cause({ message: err?.message }, "The daemon did not answer.")));
  }

  // Delete the file — a SEPARATE act from closing the card (§11), confirmed,
  // and it takes the card with it because there is nothing left to show.
  function deleteNote(el: NoteCard) {
    const record = recordOf(el.dataset.noteId);
    if (!record?.path || record.checkout) return;
    messages?.askConfirm({
      title: "Delete this note's file?",
      message: `${record.path} is deleted from the checkout. This cannot be undone.`,
      confirmLabel: "Delete",
      danger: true,
    }).then((ok) => {
      if (!ok) return;
      window.WBDaemon.write("file.delete", { repo: record.repo, path: record.path })
        .then((reply) => {
          if (WBFail.isError(reply)) {
            paintState(el, WBFail.failed(reply, "Could not delete: the daemon gave no reason."));
            return;
          }
          // The record goes without a toast: an undo that cannot put the file
          // back would be a lie.
          el._noteDirty = false;
          consoleHost?.saveNotes(removeNote(consoleHost?.notes() || [], record.id));
          render();
        })
        .catch((err: Error) => paintState(el, WBFail.cause({ message: err?.message }, "The daemon did not answer.")));
    });
  }

  return { pointElsewhere, renameNote, endRename, commitRename, deleteNote };
}
