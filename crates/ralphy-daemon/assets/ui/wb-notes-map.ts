// The note map: the notes on the plane, in desk order, for the `Note` menu
// (ADR-0064 §10). `createNotes` builds it with the card's closure names as
// typed deps; the text of each function is the one it had in `wb-notes.ts`.
import { WBGeometry } from "./wb-geometry.ts";
import { titleOf, toneOf } from "./wb-notes-folds.ts";
import type { CardHost, DeskFence, NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the map: the card's closure names. */
export type NoteMapDeps = {
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The card of a note, by id. */
  cardEl: (id: string | null | undefined) => NoteCard | null;
  /** True for a card whose fence is detached, so it is in the popup. */
  isAway: (record: Pick<NoteSource, "rect"> | null | undefined, fences: DeskFence[] | null | undefined) => boolean;
  /** The id of the card on top, or null. */
  onTopNow: () => string | null;
};

export function createNoteMap(deps: NoteMapDeps) {
  const { consoleHost, cardEl, isAway, onTopNow } = deps;

  // The notes on the plane, in desk order: what the `Note` menu draws. The
  // title comes from the LIVE card (the text is the card's, not
  // the desk's), so a note edited since it was opened lists what it says now.
  // A card that is away in a detached fence is still listed — the row jumps
  // this window's viewport to where the fence is, which is where it will be
  // when it comes home.
  function list() {
    const fences = consoleHost?.fenceRecords?.() || [];
    return (consoleHost?.notes?.() || []).map((record) => {
      const el = cardEl(record.id);
      const markdown = el?._noteMarkdown || "";
      const fence = WBGeometry?.fenceOf?.(fences, record.rect || {});
      return {
        id: record.id,
        title: titleOf(markdown, "Untitled note"),
        tone: toneOf(el?._noteTone),
        path: record.path || "",
        fence: fence?.name || "",
        onTop: record.id === onTopNow(),
        // The row's `Keep on top` is refused for a card in the popup.
        away: isAway(record, fences),
      };
    });
  }

  // Jump to a card: the plane moves to it and it takes the focus.
  function jump(id: string) {
    return consoleHost?.jumpToNote?.(id) ?? null;
  }

  return { list, jump };
}
