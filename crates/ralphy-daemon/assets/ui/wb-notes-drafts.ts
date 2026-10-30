// A draft that crosses a detach (ADR-0064 §8, amended for #475): the text of
// a note with no file yet travels in the detach snapshot, and comes home with
// a re-attach. `createNotes` builds it with the card's closure names as typed
// deps; the text of each function is the one it had in `wb-notes.ts`.
import { WBFail } from "./wb-fail.ts";
import type { Home } from "./wb-notes-stage.ts";
import type { DeskNote, NoteCard, NoteEditor, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the drafts: the card's closure names. */
export type NoteDraftsDeps = {
  window: Window;
  /** The card of a note, by id. */
  cardEl: (id: string | null | undefined) => NoteCard | null;
  /** The desk record of a note, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write one card's record back, keeping the rest of the collection. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** Take what the editor holds into the card. */
  syncFromEditor: (el: NoteCard) => void;
  /** Mark the card's text as not written yet. */
  markDirty: (el: NoteCard) => void;
  /** Build the card's editor over a text. */
  mountEditor: (el: NoteCard, markdown: string) => Promise<NoteEditor | null>;
  /** Paint the card's file path in its footer. */
  paintPath: (el: NoteCard, path: string | null | undefined) => void;
};

export function createNoteDrafts(deps: NoteDraftsDeps) {
  const { window, cardEl, recordOf, patch, syncFromEditor, markDirty, mountEditor, paintPath } =
    deps;

  // The text of a note that has NO FILE yet, for the detach snapshot. The
  // popup must open immediately (`window.open` in the same click), and the first save
  // is asynchronous, so the text travels in the snapshot instead of on
  // disk. The card is marked handed off: from here the popup's card is the
  // only writer, and `writeNow` refuses to name the note on this side.
  // `claim` is a name this card already chose, so a write already sent and
  // the popup's own write go to the same file.
  function draftOf(id: string | null | undefined) {
    const el = cardEl(id);
    const record = recordOf(id);
    if (!el || !record || record.path) return null;
    syncFromEditor(el);
    if (!el._noteDirty) return null;
    el._noteHandedOff = true;
    return { draft: el._noteMarkdown, claim: el._noteClaim || null };
  }

  // Drafts that came home with a re-attach, for a popup closed before its
  // first save. `render` builds the card from one of them in place of a read,
  // once, and only while the record still has no path. `claim` is the name
  // the popup chose before its write: that write may have landed with no
  // report after it.
  const drafts = new Map<string, Home>();
  function adoptDraft(
    id: string | null | undefined,
    draft: string | null | undefined,
    claim: string | null | undefined,
  ) {
    if (id && typeof draft === "string") drafts.set(id, { draft, claim: claim || null });
  }

  // A card built from a draft that came home. When the popup had chosen a
  // name, the file under that name is the newer text if it exists: it is
  // read, and the name is recorded. Otherwise the draft is mounted and
  // written under that name, so a re-attach never makes a second file.
  function mountHome(el: NoteCard, record: NoteSource, home: Home) {
    const claim = home.claim;
    if (!claim) return mountDraft(el, home.draft);
    el._noteClaim = claim;
    paintPath(el, claim);
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: claim }, record.checkout),
    )
      .then((reply): Promise<NoteEditor | null | void> | null => {
        if (el._noteGone) return null;
        if (!WBFail.isError(reply)) {
          el._noteClaim = null;
          patch(record.id, { path: claim });
          el._noteSavedAt = Number(reply.modified) || null;
          return mountEditor(el, reply.markdown || "");
        }
        // "not a note" is someone else's file under that name: step past it.
        if (WBFail.message(reply, "") !== "not found") {
          el._noteClaim = null;
          paintPath(el, "");
        }
        return mountDraft(el, home.draft);
      })
      .catch(() => mountDraft(el, home.draft));
  }

  // An editor over unsaved text: dirty from the start, so the ordinary
  // autosave names the note and writes it (ADR-0064 §7). Dirty BEFORE the
  // editor mounts, so a detach in that gap still carries the draft. A veiled
  // draft mounts no editor and `mountEditor` cuts the document down to its
  // header; the whole draft is put back, because the header alone is what
  // the autosave would write.
  function mountDraft(el: NoteCard, draft: string) {
    el._noteDirty = true;
    return mountEditor(el, draft).then(() => {
      if (el._noteGone) return;
      if (!el._noteEditor) el._noteMarkdown = draft;
      markDirty(el);
    });
  }

  return { draftOf, drafts, adoptDraft, mountHome, mountDraft };
}
