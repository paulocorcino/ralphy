// The note cards on the stage: how `render` builds a card for a record and
// how it takes a card off (ADR-0064 §§7, 8, 14). `createNotes` builds it with
// the card's closure names as typed deps, after the card on top, whose
// `onTopNow` and `putBack` it calls.
import type { NoteCard, NoteSource } from "./wb-types.d.ts";

/** A draft that came home with a re-attach: the text and the name it chose. */
export type Home = { draft: string; claim: string | null };

/** What `createNotes` hands the stage: the card's closure names. */
export type NoteStageDeps = {
  /** The card's chrome for one record, already on the stage. */
  buildCard: (record: NoteSource) => NoteCard;
  /** Drafts that came home with a re-attach, by note id. */
  drafts: Map<string, Home>;
  /** Build a card's text from a draft that came home. */
  mountHome: (el: NoteCard, record: NoteSource, home: Home) => unknown;
  /** Read the note's file into the card. */
  loadInto: (el: NoteCard, record: NoteSource) => unknown;
  /** Watch the card for dormancy. */
  trackDormancy: (el: NoteCard) => void;
  /** The id of the card on top, or null. */
  onTopNow: () => string | null;
  /** Put the card on top back on the desk. */
  putBack: () => void;
  /** Write what the card holds now. */
  flush: (el: NoteCard) => Promise<void>;
  /** Take a card off the stage, with its editor. */
  tearDownCard: (el: NoteCard) => void;
  /** True in a detached fence's popup, where the document holds a fragment of the plane. */
  isFragment: () => boolean;
};

export function createNoteStage(deps: NoteStageDeps) {
  const {
    buildCard,
    drafts,
    mountHome,
    loadInto,
    trackDormancy,
    onTopNow,
    putBack,
    flush,
    tearDownCard,
    isFragment,
  } = deps;

  // A new card for a record: from a draft that came home when the note has
  // no file yet, else from a read of its file.
  function mountCard(record: NoteSource) {
    const el = buildCard(record);
    const home = record.path ? undefined : drafts.get(record.id);
    drafts.delete(record.id);
    if (home) mountHome(el, record, home);
    else loadInto(el, record);
    trackDormancy(el);
    return el;
  }

  // A card whose record is not on this stage any more.
  function dropCard(id: string, el: NoteCard) {
    // A popup card whose record this document's desk has never held (a
    // note created a moment before the detach) is not gone: it is the
    // snapshot's. Tearing it down would drop its text unsaved.
    if (isFragment() && el._noteOrphan && !el._noteListed) return;
    // A card leaving the stage takes its editor with it; the record it was
    // built from has already gone (closed) or moved (detached). It is put
    // back first, so a detach carries it from its place and not from the
    // corner, and what was typed is written before the editor goes: a
    // detach, or a close from another client, tears the card down with no
    // other flush on the way. A NAMED note only: an unnamed one would be
    // named here while a detach popup opens the same record with no path,
    // and the two would write two files.
    if (id === onTopNow()) putBack();
    if (el._noteRecord?.path) {
      el._noteOrphan = el._noteRecord;
      flush(el).catch(() => {});
    }
    tearDownCard(el);
  }

  return { mountCard, dropCard };
}
