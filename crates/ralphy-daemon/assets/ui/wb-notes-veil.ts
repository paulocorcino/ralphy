// The note card's veil: a note marked as hidden opens with a glyph and no text
// until the operator shows it (ADR-0064 §8 as amended). `createNotes` builds it
// with the editor's names as typed deps; the text of each function is the one
// it had in `wb-notes.ts`.
import { veiledOf, withVeil } from "./wb-notes-folds.ts";
import type { NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the veil: the card's and the editor's names. */
export type NoteVeilDeps = {
  /** The page's document. */
  document: Document;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Read the file again into the card. */
  loadInto: (el: NoteCard, record: NoteSource) => Promise<unknown>;
  /** Pull the editor's text into the card's document. */
  syncFromEditor: (el: NoteCard) => void;
  /** Mark the card's document as changed. */
  markDirty: (el: NoteCard) => void;
  /** Write the card's document now. */
  flush: (el: NoteCard) => Promise<unknown>;
};

export function createNoteVeil(deps: NoteVeilDeps) {
  const { document, recordOf, loadInto, syncFromEditor, markDirty, flush } = deps;

  // Is this card showing nothing RIGHT NOW? The mark is the file's and the
  // reveal is the session's; a card is veiled when it carries the first and
  // has not been given the second.
  function veiledNow(el: NoteCard) {
    return veiledOf(el._noteMarkdown) && !el._noteRevealed;
  }

  // What the body holds instead of an editor. Says what it is and what opens
  // it — a card that is simply blank reads as one that failed to load.
  // A VEILED CARD SHOWS ONE GLYPH AND NO SENTENCE (asked 2026-09-22). The line
  // it replaces named the control that opens it — "the eye above shows it" —
  // which is instruction for a gesture the operator has already learnt, printed
  // on every hidden note forever. The point of the veil is that nothing about
  // the note is on screen; a caption is the one thing still talking.
  //
  // The glyph keeps its `title`, so the answer is still a hover away.
  function veilPanel() {
    const p = document.createElement("p");
    p.className = "note-veiled";
    p.title = "This note is hidden. Click the eye button to show it.";
    p.innerHTML = '<i class="bi bi-eye-slash"></i>';
    return p;
  }

  // The eye: on EVERY card, and its glyph is the ACT it offers, not the state
  // it is in. Three states, two acts — hide this note (a write), show it, put
  // it away again (both session-only). Unmarking stays in the `⋯` menu, which
  // is where the other writes to the file live.
  function paintVeil(el: NoteCard) {
    const btn = el.querySelector<HTMLElement>(".note-veil");
    if (!btn) return;
    const marked = veiledOf(el._noteMarkdown);
    const shown = marked && !!el._noteRevealed;
    const hides = !marked || shown;
    btn.innerHTML = hides ? '<i class="bi bi-eye-slash"></i>' : '<i class="bi bi-eye"></i>';
    btn.title = !marked ? "Hide this note" : shown ? "Hide this note again" : "Show this note";
    el.classList.toggle("veiled", veiledNow(el));
  }

  // Show it, or put it away again. Writes NOTHING: the mark stays whatever the
  // file says, so a note shown once is veiled again on the next open.
  function toggleReveal(el: NoteCard) {
    if (!veiledOf(el._noteMarkdown)) return;
    const record = recordOf(el.dataset.noteId);
    if (!record) return;
    if (el._noteRevealed) {
      // Away first, THEN the teardown: `loadInto` reads the file again, and
      // the card must not be left holding a body it is no longer showing.
      el._noteRevealed = false;
      loadInto(el, record);
      return;
    }
    el._noteRevealed = true;
    loadInto(el, record);
  }

  // Mark the note as one that opens veiled, or stop. A DOCUMENT write, so it
  // goes through the same autosave every other edit does — and marking puts
  // the card away in the same gesture, because marking a note you are looking
  // at and leaving it on screen is half an act.
  function setMarked(el: NoteCard, marked: boolean) {
    // A VEILED CARD HOLDS ONLY ITS HEADER — `mountEditor` cuts the body out
    // rather than put it in a document nobody is looking at — so unmarking
    // straight from `_noteMarkdown` would write that bare header OVER the note
    // and the text would be gone. MEASURED against the code path, not guessed:
    // `withVeil` rebuilds from `bodyOf`, and the body it would find is `""`.
    // So the file is read back FIRST and the unmark rides the load out, the
    // same door `toggleReveal` opens.
    if (!marked && veiledNow(el)) {
      const record = recordOf(el.dataset.noteId);
      if (!record) return;
      el._noteRevealed = true;
      loadInto(el, record).then(() => setMarked(el, false));
      return;
    }
    // Through the editor first: a character typed a frame ago is not in
    // `_noteMarkdown` yet, and this rewrites the whole document.
    syncFromEditor(el);
    el._noteMarkdown = withVeil(el._noteMarkdown, marked);
    el._noteRevealed = !marked;
    markDirty(el);
    paintVeil(el);
    // The write lands first; only then does the body go, or the flush would
    // find an editor that had already been taken away from under it.
    flush(el).then(() => {
      const record = recordOf(el.dataset.noteId);
      if (record) loadInto(el, record);
    });
  }

  return { veiledNow, veilPanel, paintVeil, toggleReveal, setMarked };
}
