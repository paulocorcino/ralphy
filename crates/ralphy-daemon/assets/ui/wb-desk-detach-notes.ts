/* ---------------------------------------------------------------------------
   The opener's record of what a popup says about its note cards: the name a
   card gave its file, the name it claimed before its first write, and the
   names a reload made the opener miss. `createNoteReports(deps)` is built by
   `createDetach` (`wb-desk-detach.ts`).
   --------------------------------------------------------------------------- */
import * as WBDeskFolds from "./wb-desk-folds.ts";
import type { PopupRegistry } from "./wb-desk-popups.ts";
import type { NoteReport } from "./wb-desk-popup-message.ts";
import type { DetachMessage } from "./wb-detach-link.ts";
import type { DeskNote } from "./wb-types.d.ts";

const { noteNameOk } = WBDeskFolds;

// What the reports read from the console, and nothing else.
export type NoteReportDeps = {
  popups: PopupRegistry;
  // The note cards; the console reassigns the list, so it is read at each use.
  notes: () => DeskNote[];
  // Writes the note cards to the desk.
  saveNotes: (next: DeskNote[]) => void;
  // Resolves once the boot desk load has settled.
  whenDeskLoaded: () => Promise<unknown>;
};

export function createNoteReports(deps: NoteReportDeps) {
  const { popups, notes, saveNotes, whenDeskLoaded } = deps;
  const { isDetached } = popups;

  // A name report sent while this tab was reloading reached nobody. The
  // popup's members carry the name it gave, so it is recorded from here, once
  // the desk has loaded. `recordNoteName` refuses a record that already has
  // its path, so this is safe to repeat.
  function replayNoteNames(id: string, members: NonNullable<DetachMessage["members"]>) {
    void whenDeskLoaded().then(() => {
      for (const x of members) {
        if (x.kind === "note" && typeof x.path === "string") {
          recordNoteName(id, { noteId: x.id, path: x.path });
        }
      }
      return null;
    });
  }

  // The popup's card gave a never-saved note its file. The popup cannot write
  // the desk (ADR-0051 §8), so it reports the name and this tab records it,
  // after checking the report. The report comes twice: over `postMessage`,
  // which arrives before the popup's own `wb-fence-reattach`, and over the
  // channel, which still reaches this tab after a reload. The second one is
  // refused because the record already has its path.
  function recordNoteName(id: string, m: NoteReport) {
    const entry = popups.entry(id);
    const record = notes().find((n) => n.id === m.noteId);
    if (!entry || !isDetached(id) || !noteNameOk(entry, record, m)) return;
    saveNotes(notes().map((n) => (n.id === m.noteId ? { ...n, path: m.path } : n)));
    entry.members = entry.members.map((x) => {
      if (x.kind !== "note" || x.id !== m.noteId) return x;
      const { draft, claim, ...rest } = x;
      return { ...rest, path: m.path };
    });
  }

  // The name the popup's card chose BEFORE its first write, kept on the member
  // and never on the desk (a path on the desk says a file is there). If the
  // popup closes with that write in flight, no name report follows, and a
  // re-attach reads or writes this name instead of choosing a second one.
  function recordNoteClaim(id: string, m: NoteReport) {
    const entry = popups.entry(id);
    const record = notes().find((n) => n.id === m.noteId);
    if (
      !entry ||
      !isDetached(id) ||
      !noteNameOk(entry, record, { noteId: m.noteId, path: m.claim })
    ) {
      return;
    }
    entry.members = entry.members.map((x) =>
      x.kind === "note" && x.id === m.noteId ? { ...x, claim: m.claim } : x,
    );
  }

  return { replayNoteNames, recordNoteName, recordNoteClaim };
}
