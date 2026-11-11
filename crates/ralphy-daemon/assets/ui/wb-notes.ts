// The note CARD: a note's view on the consoles stage (ADR-0064 §§2, 7–14).
//
// `wb-console.ts` owns the desk; this file owns the card: its DOM, its editor, its
// autosave. Nothing here touches `/api/desk`: a placement change goes through the
// console's `saveNotes`, and a byte through `note.read`/`note.write`.
//
// The editor is Milkdown Crepe (ADR-0064 §6), reached only through
// `window.CrepeLean`, the one surface `vendor-build/crepe/entry.js` exports.
//
// Loaded in the shell AND in the detached-fence popup: no module-scope DOM read,
// no `fetch`, no timer at load (the ui-tests evaluate this file under a stub
// document), and no browser store of its own (`wb-client-view.ts`, #339).
import { WBGeometry } from "./wb-geometry.ts";
import { NOTE_DEFAULT, lockedBy, NOTE_FOLDS } from "./wb-notes-folds.ts";
import { createNoteBuild } from "./wb-notes-build.ts";
import { createNoteChrome } from "./wb-notes-chrome.ts";
import { createNoteDormancy } from "./wb-notes-dormancy.ts";
import { createNoteDrafts } from "./wb-notes-drafts.ts";
import { createNoteEditor } from "./wb-notes-editor.ts";
import { createNoteFile } from "./wb-notes-file.ts";
import { installLinks } from "./wb-notes-links.ts";
import { createNoteMenus } from "./wb-notes-menus.ts";
import { createNoteOpen } from "./wb-notes-open.ts";
import { createNoteOnTop } from "./wb-notes-on-top.ts";
import { createNoteVeil } from "./wb-notes-veil.ts";
import { createNoteMap } from "./wb-notes-map.ts";
import { createNotePalette } from "./wb-notes-palette.ts";
import { createNoteStage } from "./wb-notes-stage.ts";
import type { CrepeLean } from "./wb-notes-editor.ts";
import type { Messages } from "./wb-messages.ts";
import type { Gestures, Stack } from "./wb-stage-stack.ts";
import type { CardHost, DeskFence, DeskNote, NoteCard, NoteSource } from "./wb-types.d.ts";

type NotesWindow = Window & {
  CrepeLean?: CrepeLean;
  IntersectionObserver?: typeof IntersectionObserver;
  ResizeObserver?: typeof ResizeObserver;
};

/** The cards of the stage, by note id. */
type CardNodes = Map<string, NoteCard>;

/** What the entry module hands the cards. */
export type NotesDeps = {
  /** The page's consoles, built before the cards; null where a test has none. */
  console: CardHost | null;
  /** The page's door for operator messages; null where a test has none. */
  messages: Messages | null;
  /** The document's z stack, shared with the consoles; null where a test has none. */
  stack: Stack | null;
  /** The document's gestures, shared with the consoles; null where a test has none. */
  gestures: Gestures | null;
};

export function createNotes(window: NotesWindow, document: Document, deps: NotesDeps) {
  const consoleHost = deps.console;
  const messages = deps.messages;
  const stack = deps.stack;
  const gestures = deps.gestures;
  // ---- the card ----------------------------------------------------------------

  function stage() {
    return document.getElementById("stage");
  }

  function cardEl(id: string | null | undefined) {
    const st = stage();
    if (!st) return null;
    // Indexed, not selected: an id is daemon data and one quote in it would
    // throw a SyntaxError out of the whole render (`renderFences`' lesson).
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) {
      if (el.dataset["noteId"] === id) return el;
    }
    return null;
  }

  function recordOf(id: string | null | undefined) {
    return (consoleHost?.notes() ?? []).find((n) => n.id === id) ?? null;
  }

  // Write one card's record back, keeping the rest of the collection.
  // `saveNotes` sends only the fields that differ (ADR-0050 amendment
  // 2026-10-04, changes, not the desk).
  function patch(id: string | null | undefined, fields: Partial<DeskNote>) {
    const next = (consoleHost?.notes() ?? []).map((n) => (n.id === id ? { ...n, ...fields } : n));
    consoleHost?.saveNotes(next);
  }

  // The rect as the DOM holds it — the shape the desk record wants. A card on
  // top paints its floating box over the inline rect, which is still the desk
  // rect, so a fence move persists the place and not the box.
  function rectOf(el: NoteCard) {
    if (el.classList.contains("on-top")) return consoleHost?.restoreRect(el);
    return {
      left: el.offsetLeft,
      top: el.offsetTop,
      width: el.offsetWidth,
      height: el.offsetHeight,
    };
  }

  // Persist the placement of one or more cards after a gesture: ONE write for
  // the whole set, so a fence carrying six cards uploads once.
  function persistCards(els: NoteCard | NoteCard[]) {
    const list = Array.isArray(els) ? els : [els];
    const moved = new Map(list.filter(Boolean).map((el) => [el.dataset["noteId"], rectOf(el)]));
    if (moved.size === 0) return;
    const next = (consoleHost?.notes() ?? []).map((n) => {
      const rect = moved.get(n.id);
      return rect ? { ...n, rect } : n;
    });
    consoleHost?.saveNotes(next);
  }

  // The card's chrome for one record. Mirrors `buildFence`'s construction
  // order, including its hit-test rule; `assembleCard` does the rest.
  function buildCard(record: NoteSource) {
    const el = document.createElement("div") as HTMLElement as NoteCard;
    return assembleCard(el, record);
  }

  // ---- the veil (in `wb-notes-veil.ts`) ----------------------------------------

  // The editor is built after the veil, so its names reach the veil as arrows.
  const { veiledNow, veilPanel, paintVeil, toggleReveal, setMarked } = createNoteVeil({
    document,
    recordOf,
    loadInto: (el, record) => loadInto(el, record),
    syncFromEditor: (el) => {
      syncFromEditor(el);
    },
    markDirty: (el) => {
      markDirty(el);
    },
    flush: (el) => flush(el),
  });

  // ---- the links (in `wb-notes-links.ts`) --------------------------------------

  const linkCard = installLinks({ window, document, recordOf });

  // ---- the editor (in `wb-notes-editor.ts`) ------------------------------------

  const {
    mountEditor,
    loadInto,
    paintTitle,
    paintState,
    syncFromEditor,
    markDirty,
    flush,
    paintPath,
  } = createNoteEditor({
    window,
    document,
    consoleHost,
    recordOf,
    patch,
    render,
    paintIndex: (el) => {
      paintIndex(el);
    },
    veilPanel,
    paintVeil,
    fragment: () => fragment,
  });

  // ---- the palette (in `wb-notes-palette.ts`) ----------------------------------

  // The palette and the menus close each other, so the palette reaches the
  // menus' `closeMenu` through an arrow.
  const { togglePalette, closePalette, buildPalette } = createNotePalette({
    document,
    veiledNow,
    closeMenu: () => {
      closeMenu();
    },
    syncFromEditor,
    markDirty,
  });

  // ---- the menus and the cheat sheet (in `wb-notes-menus.ts`) ------------------

  const { toggleMenu, toggleIndex, paintIndex, closeMenu, markdownHelp, MARKDOWN_HELP } =
    createNoteMenus({
      window,
      document,
      recordOf,
      syncFromEditor,
      closePalette,
    });

  // ---- the file acts (in `wb-notes-file.ts`) -----------------------------------

  const { pointElsewhere, renameNote, endRename, commitRename, deleteNote } = createNoteFile({
    window,
    consoleHost,
    messages,
    recordOf,
    patch,
    render,
    paintPath,
    paintState,
    loadInto,
  });

  // ---- dormancy (in `wb-notes-dormancy.ts`) ------------------------------------

  // A card on top is never put to sleep, and `onTopNow` is built after this, so
  // it reaches the dormancy through an arrow.
  const { trackDormancy, untrackDormancy, wakeCard } = createNoteDormancy({
    window,
    document,
    recordOf,
    loadInto,
    onTopNow: () => onTopNow(),
    veiledNow,
  });

  // ---- creating, opening and closing a card (in `wb-notes-open.ts`) ----------

  const { create, closeCard, openFromExplorer } = createNoteOpen({
    window,
    consoleHost,
    messages,
    stack,
    cardEl,
    recordOf,
    deskNotes,
    deskFences,
    render,
    syncFromEditor,
    flush,
  });

  // ---- the card's chrome and the title (in `wb-notes-chrome.ts`) ---------------

  // A card on top is built after the chrome, so its names reach the chrome
  // through arrows.
  const { buildCardParts } = createNoteChrome({
    document,
    consoleHost,
    recordOf,
    patch,
    render,
    persistCards,
    closeCard,
    veiledNow,
    toggleReveal,
    setMarked,
    mountEditor,
    syncFromEditor,
    markDirty,
    paintTitle,
    togglePalette,
    closePalette,
    toggleMenu,
    toggleIndex,
    closeMenu,
    markdownHelp,
    pointElsewhere,
    renameNote,
    deleteNote,
    commitRename,
    endRename,
    onTop: (el) => onTop(el),
    putBack: () => {
      putBack();
    },
    floatGesture: (el, dir) => floatGesture(el, dir),
  });

  // The ONE place a card is taken off the stage: the editor goes with it,
  // whether it had finished mounting or not.
  function tearDownCard(el: NoteCard) {
    el._noteGone = true;
    clearTimeout(el._noteTimer);
    el._noteTimer = null;
    untrackDormancy(el);
    const editor = el._noteEditor;
    el._noteEditor = null;
    try {
      editor?.destroy();
    } catch {}
    el.remove();
  }

  // ---- rendering ---------------------------------------------------------------

  // The desk's note records and fence records; none where a test has no host.
  function deskNotes() {
    return consoleHost?.notes() ?? [];
  }
  function deskFences() {
    return consoleHost?.fenceRecords() ?? [];
  }

  // A card whose fence is detached is not on THIS stage — it is in the popup. Derived, never
  // stored: the record stays in the desk (the popup is a view, not an owner), so a reload works
  // this out again from the fence registry, not restoring a set that died with the document.
  function isAway(
    record: Pick<NoteSource, "rect"> | null | undefined,
    fences: DeskFence[] | null | undefined,
  ) {
    const held = WBGeometry.fenceOf(fences ?? [], record?.rect);
    // oxlint-disable-next-line typescript/no-unnecessary-condition -- a test host has no isDetached
    return held !== null && consoleHost?.isDetached?.(held.id) === true;
  }

  // This document holds a FRAGMENT of the plane (one detached fence's members),
  // not the desk. Latched by `mountDetached`: the popup reads the whole desk while
  // its detach registry is inert, so the first `render()` there would build a
  // card and a `note.read` for every note on the shell's plane.
  let fragment = false;

  // Put the desk's cards on the stage: build what is missing, move what moved,
  // drop what is gone. Idempotent — the same shape as `renderFences`.
  function render() {
    const st = stage();
    if (!st) return;
    const records = deskNotes();
    const fences = deskFences();
    const nodes = cardNodes(st);
    const seen = new Set<string>();
    for (const record of records) {
      if (!showsHere(record, fences, nodes)) continue;
      seen.add(record.id);
      const el = nodes.get(record.id) ?? mountCard(record);
      el._noteListed = true;
      paint(el, record, fences);
    }
    dropUnseen(nodes, seen);
  }

  // The cards on the stage, by id. `buildCard` sets every card's `data-note-id`.
  function cardNodes(st: HTMLElement): CardNodes {
    const nodes: CardNodes = new Map();
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) {
      const id = el.dataset["noteId"];
      if (id !== undefined) nodes.set(id, el);
    }
    return nodes;
  }

  // Is this record's card this window's to show? Not when its fence is in a
  // popup. In a fragment, an unknown record is not this window's either.
  function showsHere(record: NoteSource, fences: DeskFence[], nodes: CardNodes) {
    if (isAway(record, fences)) return false;
    return !fragment || nodes.has(record.id);
  }

  // A card no record names any more leaves the stage.
  function dropUnseen(nodes: CardNodes, seen: Set<string>) {
    for (const [id, el] of nodes) {
      if (!seen.has(id)) dropCard(id, el);
    }
  }

  // One record onto one card: rect, title, path, lock. A card under a
  // gesture keeps the place the operator's hand gives it.
  function paint(el: NoteCard, record: NoteSource, fences: DeskFence[]) {
    el._noteRecord = record;
    const r = record.rect;
    if (gestures?.active(el) !== true) {
      el.style.left = (r.left || 0) + "px";
      el.style.top = (r.top || 0) + "px";
      el.style.width = (r.width || NOTE_DEFAULT.width) + "px";
      el.style.height = (r.height || NOTE_DEFAULT.height) + "px";
    }
    paintTitle(el);
    paintPath(el, record.path);
    applyLock(el, lockedBy(record, fences) !== null);
    if (el._noteShadow) paintShadow(el);
  }

  // The popup's side of a detached fence: the same card, from the snapshot the
  // opener handed over.
  function mountDetached(record: NoteSource) {
    fragment = true;
    const el = buildCard(record);
    // This document's desk may not hold the record yet (a note created a
    // moment before the detach), and `writeNow` needs one to save at all.
    el._noteOrphan = record;
    if (typeof record.draft === "string" && (record.path ?? "") === "") {
      if (record.claim != null && record.claim !== "") {
        el._noteClaim = record.claim;
        paintPath(el, record.claim);
      }
      void mountDraft(el, record.draft);
    } else {
      void loadInto(el, record);
    }
    paint(el, record, []);
    if (el._noteClaim != null && el._noteClaim !== "") paintPath(el, el._noteClaim);
    return el;
  }

  // ---- a draft that crosses a detach (in `wb-notes-drafts.ts`) ----------------

  const { draftOf, drafts, adoptDraft, mountHome, mountDraft } = createNoteDrafts({
    window,
    cardEl,
    recordOf,
    patch,
    syncFromEditor,
    markDirty,
    mountEditor,
    paintPath,
  });

  // ---- a card on top (in `wb-notes-on-top.ts`) ---------------------------------

  const { onTop, keepOnTop, putBack, onTopNow, paintShadow, floatGesture } = createNoteOnTop({
    window,
    document,
    consoleHost,
    stack,
    stage,
    cardEl,
    recordOf,
    isAway,
    wakeCard,
    fragment: () => fragment,
  });

  // ---- the card's assembly (in `wb-notes-build.ts`) ----------------------------

  const { assembleCard, applyLock } = createNoteBuild({
    document,
    consoleHost,
    stack,
    stage,
    buildCardParts,
    buildPalette,
    linkCard,
    persistCards,
    flush,
    onTop,
    floatGesture,
  });

  // ---- the cards on the stage (in `wb-notes-stage.ts`) -------------------------

  const { mountCard, dropCard, flushAll, anyDirty } = createNoteStage({
    stage,
    syncFromEditor,
    buildCard,
    drafts,
    mountHome,
    loadInto,
    trackDormancy,
    onTopNow,
    putBack,
    flush,
    tearDownCard,
    isFragment: () => fragment,
  });

  // ---- the map (in `wb-notes-map.ts`) ------------------------------------------

  const { list, jump } = createNoteMap({ consoleHost, cardEl, isAway, onTopNow });

  return {
    ...NOTE_FOLDS,
    MARKDOWN_HELP,
    // the card
    render,
    create,
    buildCard,
    mountDetached,
    draftOf,
    adoptDraft,
    applyLock,
    persistCards,
    closeCard,
    isAway,
    cardEl,
    flushAll,
    anyDirty,
    list,
    keepOnTop,
    putBack,
    onTopNow,
    jump,
    markdownHelp,
    openFromExplorer,
  };
}
