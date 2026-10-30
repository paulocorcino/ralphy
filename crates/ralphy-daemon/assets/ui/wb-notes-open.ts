// Creating, opening and closing a card (ADR-0064 §§9, 11): a new note, a
// note opened from the explorer, and a close that keeps the file.
// `createNotes` builds it with the card's closure names as typed deps; the
// text of each function is the one it had in `wb-notes.ts`.
import { addNote, removeNote, NOTE_MAX } from "./wb-desk-folds.ts";
import { spawnRect } from "./wb-notes-folds.ts";
import type { Messages } from "./wb-messages.ts";
import type { Stack } from "./wb-stage-stack.ts";
import type { CardHost, DeskFence, DeskNote, NoteCard, NoteSource, Offset, Size } from "./wb-types.d.ts";

/** Where a new card lands, and which file it opens (`create`, `openFromExplorer`). */
export type NewNote = {
  repo?: string | null;
  checkout?: string | null;
  viewport?: Partial<Size> | null;
  offset?: Partial<Offset> | null;
};

/** What `createNotes` hands the card's birth and close: the card's closure names. */
export type NoteOpenDeps = {
  window: Window;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The page's door for operator messages; null where a test has none. */
  messages: Messages | null;
  /** The document's z stack; null where a test has none. */
  stack: Stack | null;
  /** The card of a note, by id. */
  cardEl: (id: string | null | undefined) => NoteCard | null;
  /** The desk record of a note, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** The desk's note records. */
  deskNotes: () => DeskNote[];
  /** The desk's fence records. */
  deskFences: () => DeskFence[];
  /** Put the desk's cards on the stage. */
  render: () => void;
  /** Take what the editor holds into the card. */
  syncFromEditor: (el: NoteCard) => void;
  /** Write what the card holds now. */
  flush: (el: NoteCard) => Promise<void>;
};

export function createNoteOpen(deps: NoteOpenDeps) {
  const { window, consoleHost, messages, stack, cardEl, recordOf, deskNotes, deskFences, render, syncFromEditor, flush } = deps;

  // One rule for both ways a card is born: the cap refuses, it does not evict
  // (see `saveNotes`); the new record lands at the end of the desk's list.
  function addCard(fields: { repo: string; checkout: string | null; path: string }, { viewport, offset }: NewNote) {
    const records = deskNotes();
    const record = {
      id: `note-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
      ...fields,
      rect: spawnRect(
        viewport || { width: 0, height: 0 },
        offset || { left: 0, top: 0 },
        records.length,
        deskFences(),
      ),
      locked: false,
      ts: Date.now(),
    };
    const next = addNote(records, record, NOTE_MAX);
    if (!next) {
      messages?.toast({ text: `You can have at most ${NOTE_MAX} notes. Close one first.` });
      return null;
    }
    consoleHost?.saveNotes(next);
    render();
    return record;
  }

  // A new note (ADR-0064 §9): no dialog. The card lands in the middle of the
  // view with an empty editor and a default directory in its footer, and NO
  // file — the first autosave names it, so a note nobody typed into is never
  // written.
  function create(where: NewNote = {}) {
    const { repo, checkout } = where;
    if (!repo) return null;
    const record = addCard({ repo, checkout: checkout ?? null, path: "" }, where);
    if (!record) return null;
    const el = cardEl(record.id);
    if (el) {
      stack?.focusWin(el);
      // "already in edit with the cursor placed" (ADR-0064 §9) — the editor
      // mounts a tick later, so the intent is left on the card and honoured by
      // `mountEditor`. Without it the first act after "New note" is a click
      // into a card that is already focused.
      el._noteFocusOnMount = true;
    }
    return record.id;
  }

  // Close the card, keep the file (ADR-0064 §11). The undo is what makes this
  // safe to do with one click on a document nobody asked to delete.
  function closeCard(id: string | null | undefined) {
    const record = recordOf(id);
    if (!record) return;
    const el = cardEl(id);
    // Whatever was typed lands before the card goes: a close is not a discard —
    // including the character typed a frame ago, which the editor knows about
    // before its listener does.
    if (el) syncFromEditor(el);
    // `.catch` is not decoration — the naming probe rejects on a socket the
    // daemon closed without a reply, and without it the close button would
    // silently do nothing at exactly the moment the operator is trying to
    // tidy up.
    const written = el && el._noteDirty ? flush(el).catch(() => {}) : Promise.resolve();
    void written.then(() => {
      // Read the record AFTER the flush: closing a never-saved note performs
      // its first save, and the path the undo must restore is the one that
      // save just chose — the pre-flush snapshot aims at nothing.
      const saved = recordOf(id) || record;
      consoleHost?.saveNotes(removeNote(consoleHost?.notes() || [], record.id));
      render();
      messages?.toast({
        // The path IS the sentence: "note closed · <path> kept" said the same
        // thing twice, and `kept` was reassuring nobody about the file the
        // `✕` never touches (asked 2026-09-22).
        text: saved.path ? `Note ${saved.path} closed` : "Note closed",
        action: "Undo",
        onAction: () => {
          consoleHost?.saveNotes((consoleHost?.notes() || []).concat([saved]));
          render();
        },
      });
    });
  }

  // A double-click on a `.note` in the tree: jump to the card if it is already
  // on the plane (identity is `(repo, checkout, path)`), else put one there.
  // The read happens in `loadInto`, so a file that is not a note lands as the
  // missing/refused state on a card the operator can close — never silently.
  function openFromExplorer({ repo, checkout, path, viewport, offset }: NewNote & { path?: string | null }) {
    if (!repo || !path) return null;
    const tree = checkout ?? null;
    const already = (consoleHost?.notes?.() || []).find(
      (n) => n.repo === repo && (n.checkout ?? null) === tree && n.path === path,
    );
    if (already) return window.WBNotes.jump(already.id);
    const record = addCard({ repo, checkout: tree, path }, { viewport, offset });
    if (!record) return null;
    return window.WBNotes.jump(record.id);
  }

  return { create, closeCard, openFromExplorer };
}
