// The note CARD: a note's view on the consoles stage (ADR-0064 §§2, 7–14).
//
// The split this file rests on: `wb-console.ts` owns the desk — the `notes`
// records, the flush, the fold against other pages — and this file owns the
// card: its DOM, its gestures' bindings, its editor, its autosave. Nothing here
// touches `/api/desk`; every placement change goes through
// the console's `saveNotes`, and every byte through `note.read`/`note.write`.
//
// The editor is Milkdown Crepe, vendored lean (ADR-0064 §6) and reached through
// the ONE surface `vendor-build/crepe/entry.js` exports — `window.CrepeLean`.
// Nothing below knows what a ProseMirror plugin is, which is what makes the
// engine replaceable.
//
// Loaded in the shell AND in the detached-fence popup, so: no module-scope DOM
// read, no `fetch`, no timer at load (the ui-tests evaluate this file under a
// stub document), and no browser store of its own — `wb-client-view.ts` holds the one
// there is (#339).
import { WBGeometry } from "./wb-geometry.ts";
import { addNote, removeNote, NOTE_MAX } from "./wb-desk-folds.ts";
import { DEFAULT_FILL, DEFAULT_INK, DEFAULT_TONE, NEW_NOTE_STYLE, NOTE_DEFAULT, applyLook, lockedBy, spawnRect, NOTE_FOLDS } from "./wb-notes-folds.ts";
import { createNoteChrome } from "./wb-notes-chrome.ts";
import { createNoteDormancy } from "./wb-notes-dormancy.ts";
import { createNoteDrafts } from "./wb-notes-drafts.ts";
import { createNoteEditor } from "./wb-notes-editor.ts";
import { createNoteFile } from "./wb-notes-file.ts";
import { installLinks } from "./wb-notes-links.ts";
import { createNoteMenus } from "./wb-notes-menus.ts";
import { createNoteOnTop } from "./wb-notes-on-top.ts";
import { createNoteVeil } from "./wb-notes-veil.ts";
import { createNoteMap } from "./wb-notes-map.ts";
import { createNotePalette } from "./wb-notes-palette.ts";
import { createNoteStage } from "./wb-notes-stage.ts";
import type { CrepeLean } from "./wb-notes-editor.ts";
import type { Messages } from "./wb-messages.ts";
import type { Gestures, Stack } from "./wb-stage-stack.ts";
import type { CardHost, DeskFence, DeskNote, NoteCard, NoteSource, Offset, Size } from "./wb-types.d.ts";

type NotesWindow = Window & {
  CrepeLean?: CrepeLean;
  IntersectionObserver?: typeof IntersectionObserver;
  ResizeObserver?: typeof ResizeObserver;
};

/** Where a new card lands, and which file it opens (`create`, `openFromExplorer`). */
type NewNote = {
  repo?: string | null;
  checkout?: string | null;
  viewport?: Partial<Size> | null;
  offset?: Partial<Offset> | null;
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
      if (el.dataset.noteId === id) return el;
    }
    return null;
  }

  function recordOf(id: string | null | undefined) {
    return (consoleHost?.notes?.() || []).find((n) => n.id === id) || null;
  }

  // Write one card's record back, keeping the rest of the collection.
  // `saveNotes` sends only the fields that differ (ADR-0050 amendment
  // 2026-10-04, changes, not the desk).
  function patch(id: string | null | undefined, fields: Partial<DeskNote>) {
    const next = (consoleHost?.notes?.() || []).map((n) =>
      n.id === id ? { ...n, ...fields } : n,
    );
    consoleHost?.saveNotes(next);
  }

  // The rect as the DOM holds it — the shape the desk record wants. A card on
  // top paints its floating box over the inline rect, which is still the desk
  // rect, so a fence move persists the place and not the box.
  function rectOf(el: NoteCard) {
    if (el.classList?.contains("on-top")) return consoleHost?.restoreRect(el);
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
    const moved = new Map(list.filter(Boolean).map((el) => [el.dataset.noteId, rectOf(el)]));
    if (!moved.size) return;
    const next = (consoleHost?.notes?.() || []).map((n) => {
      const rect = moved.get(n.id);
      return rect ? { ...n, rect } : n;
    });
    consoleHost?.saveNotes(next);
  }

  // Paint the lock. A locked card cannot be MOVED or RESIZED, and that is the
  // whole of it (the operator's words, 2026-09-22: "o cadeado no notes não é
  // impedir de editar é impedir de mover"). The note is still written, renamed,
  // recoloured and hidden — a pin through the card, not a read-only file. The
  // JS guards in `makeDraggable`/`startResize` are the truth for the gestures;
  // this is the glyph.
  function applyLock(el: NoteCard, locked: boolean) {
    el._noteLocked = !!locked;
    el.classList.toggle("locked", !!locked);
    const btn = el.querySelector<HTMLElement>(".note-lock");
    if (btn) {
      // The console's own two glyphs, verbatim (`wb-console.ts`'s `applyLock`):
      // one lock on the plane, not one per surface.
      btn.innerHTML = locked ? '<i class="bi bi-lock-fill"></i>' : '<i class="bi bi-unlock"></i>';
      btn.title = locked ? "Unlock this note" : "Lock this note in place";
    }
  }

  // The card's chrome for one record. Mirrors `buildFence`'s construction
  // order, including its hit-test rule.
  function buildCard(record: NoteSource) {
    const el = document.createElement("div") as HTMLElement as NoteCard;
    el.className = "note-card";
    el.dataset.noteId = record.id;
    // A card with NO FILE YET is a new note and is born in the operator's
    // colours; one with a path is dressed from what it is about to read, and
    // starting it anywhere but the reading default would flash a colour the
    // file does not hold for the tick the read takes.
    const born = record.path
      ? { tone: DEFAULT_TONE, fill: DEFAULT_FILL, ink: DEFAULT_INK }
      : NEW_NOTE_STYLE;
    applyLook(el, born);
    // Everything the card knows that is not in the desk record: the text, what
    // has been written, and what is in flight. Deliberately NOT desk state —
    // the file is the note (ADR-0064 §2).
    el._noteMarkdown = "";
    el._noteDirty = false;
    el._noteInFlight = false;
    el._noteTimer = null;
    el._noteSavedAt = null;
    // The SESSION half of the veil: the file says whether a note opens veiled,
    // this says whether it has been shown since. Never persisted — that is the
    // whole point of the pair.
    el._noteRevealed = false;

    const { handles, head, body, foot, tools, menu, anchors } = buildCardParts(el);

    // ORDER IS THE HIT TEST (see `buildFence`): the bands overlap the head and
    // the tools, later siblings win, so the interactive clusters go last.
    const palette = buildPalette(el);
    el.append(...handles, head, body, foot, tools, menu, anchors, palette);
    el.addEventListener("pointerdown", () => stack?.focusWin(el), true);
    // The plane's gestures write the inline rect, which is the DESK rect; a
    // card on top moves its floating box instead, through `floatGesture`.
    consoleHost?.makeDraggable(el, head, {
      locked: () => !!el._noteLocked || onTop(el),
      onDrop: () => persistCards(el),
    });
    head.addEventListener("pointerdown", floatGesture(el, null));
    // A press on the body that misses the editor still means "type here".
    // MEASURED: the editor fills the body but not its padding, and a short
    // note leaves most of a card below the last line — a click there focused
    // nothing, and the operator's next keystroke went wherever the focus
    // happened to be. The card is a place to write; every pixel of its body
    // says so.
    //
    // ANYTHING THAT IS NOT THE EDITOR, not "is the body": measured again
    // 2026-09-22 in a browser, the blank space under the last line belongs to
    // Crepe's own `.milkdown` wrapper, which sits between the two — so the
    // `ev.target !== body` this guard used to be let every one of those
    // presses through to the default, which BLURRED the editor. The keystrokes
    // after it went to `document.body` and were lost with no sign on the card.
    body.addEventListener("mousedown", (ev) => {
      const dom = el._noteEditor?.dom?.();
      if (!dom) return;
      if (dom === ev.target || dom.contains(ev.target as Node | null)) return;
      ev.preventDefault();
      try {
        dom.focus();
      } catch {}
    });
    installKeys(el);
    linkCard(el);
    // Leaving the card writes what was typed: `focusout` fires before whatever
    // the operator clicked next does anything (ADR-0064 §7).
    el.addEventListener("focusout", () => {
      // Unconditional: `flush` asks the editor what it holds and returns at
      // once when there is nothing to write, so the last character typed
      // before the pointer left the body is carried even though the editor's
      // change listener has not fired yet.
      void flush(el);
    });
    stage()?.append(el);
    // In the WINDOW tier from the moment it exists. A card built by a restore
    // is never focused, so without this it kept `z-index: auto` and every
    // console painted over it — see the stack's `stackWin`.
    stack?.stackWin(el);
    return el;
  }

  // ---- the veil (in `wb-notes-veil.ts`) ----------------------------------------

  // The editor is built after the veil and the veil acts through it, so the
  // editor's names reach the veil as arrows.
  const { veiledNow, veilPanel, paintVeil, toggleReveal, setMarked } = createNoteVeil({
    document,
    recordOf,
    loadInto: (el, record) => loadInto(el, record),
    syncFromEditor: (el) => syncFromEditor(el),
    markDirty: (el) => markDirty(el),
    flush: (el) => flush(el),
  });

  // ---- the links (in `wb-notes-links.ts`) --------------------------------------

  const linkCard = installLinks({ window, document, recordOf });

  // ---- the editor (in `wb-notes-editor.ts`) ------------------------------------

  const { mountEditor, loadInto, paintTitle, paintState, syncFromEditor, markDirty, flush, paintPath } =
    createNoteEditor({ window, document, consoleHost, recordOf, patch, render, paintIndex: (el) => paintIndex(el), veilPanel, paintVeil, fragment: () => fragment });

  // ---- the palette (in `wb-notes-palette.ts`) ----------------------------------

  // The palette and the menus close each other, so the palette reaches the
  // menus' `closeMenu` through an arrow.
  const { togglePalette, closePalette, buildPalette } = createNotePalette({
    document, veiledNow, closeMenu: () => closeMenu(), syncFromEditor, markDirty,
  });

  // ---- the menus and the cheat sheet (in `wb-notes-menus.ts`) ------------------

  const { toggleMenu, toggleIndex, paintIndex, closeMenu, markdownHelp, MARKDOWN_HELP } = createNoteMenus({
    window, document, recordOf, syncFromEditor, closePalette,
  });

  // ---- the file acts (in `wb-notes-file.ts`) -----------------------------------

  const { pointElsewhere, renameNote, endRename, commitRename, deleteNote } = createNoteFile({
    window, consoleHost, messages, recordOf, patch, render, paintPath, paintState, loadInto,
  });

  // ---- dormancy (in `wb-notes-dormancy.ts`) ------------------------------------

  // A card on top is never put to sleep, and `onTopNow` is built after this, so
  // it reaches the dormancy through an arrow.
  const { trackDormancy, untrackDormancy, wakeCard } = createNoteDormancy({
    window, document, recordOf, loadInto, onTopNow: () => onTopNow(), veiledNow,
  });

  // ---- the card's chrome and the title (in `wb-notes-chrome.ts`) ---------------

  // A card on top is built after the chrome, so its names reach the chrome
  // through arrows.
  const { buildCardParts } = createNoteChrome({
    document, consoleHost, recordOf, patch, render, persistCards, closeCard, veiledNow, toggleReveal, setMarked,
    mountEditor, syncFromEditor, markDirty, paintTitle, togglePalette, closePalette, toggleMenu, toggleIndex,
    closeMenu, markdownHelp, pointElsewhere, renameNote, deleteNote, commitRename, endRename,
    onTop: (el) => onTop(el), putBack: () => putBack(), floatGesture: (el, dir) => floatGesture(el, dir),
  });

  // ---- keys ----------------------------------------------------------

  // The card's own keys (ADR-0064 §13). `consoleShortcutsBlocked()` already
  // stands down inside a `contentEditable`, so the plane's accelerators do not
  // reach a note being typed into and nothing here has to fight them.
  function installKeys(el: NoteCard) {
    el.addEventListener("keydown", (ev) => {
      if ((ev.ctrlKey || ev.metaKey) && (ev.key === "s" || ev.key === "S")) {
        // The browser's Save-page dialog, over a note the daemon already has,
        // is the wrong answer to this key.
        ev.preventDefault();
        ev.stopPropagation();
        void flush(el);
        return;
      }
      if (ev.key !== "Escape") return;
      // Two stages: out of the editor, then off the card. Held at the card so
      // an Escape meant for this edit never also reaches the plane.
      ev.stopPropagation();
      const inEditor = el.querySelector<HTMLElement>(".note-body")?.contains(document.activeElement);
      (document.activeElement as HTMLElement | null)?.blur?.();
      if (!inEditor) el.classList.remove("focused");
    });
  }

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

  // ---- creating, closing, rendering ---------------------------------------------

  // The desk's note records and fence records; none where a test has no host.
  function deskNotes() {
    return consoleHost?.notes?.() || [];
  }
  function deskFences() {
    return consoleHost?.fenceRecords?.() || [];
  }

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

  // A card whose fence is detached is not on THIS stage — it is in the popup.
  // Derived, never stored: the record stays in the desk (the popup is a view,
  // not an owner), so a reload works this out again from the fence registry
  // instead of restoring a set that died with the document.
  function isAway(record: Pick<NoteSource, "rect"> | null | undefined, fences: DeskFence[] | null | undefined) {
    const held = WBGeometry?.fenceOf?.(fences || [], record?.rect);
    return !!held && !!consoleHost?.isDetached?.(held.id);
  }

  // This document holds a FRAGMENT of the plane — one detached fence's members
  // — not the desk. Latched by `mountDetached`, because the popup reads the
  // whole desk (every document runs `reloadDesk`) while its detach registry is
  // deliberately inert: without this, the first `render()` a lock or a close
  // triggers there would build a card, an editor and a `note.read` for every
  // note on the shell's plane.
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
      const el = nodes.get(record.id) || mountCard(record);
      el._noteListed = true;
      paint(el, record, fences);
    }
    dropUnseen(nodes, seen);
  }

  // The cards on the stage, by id. `buildCard` sets every card's `data-note-id`.
  function cardNodes(st: HTMLElement): CardNodes {
    const nodes: CardNodes = new Map();
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) nodes.set(el.dataset.noteId as string, el);
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
    const r = record.rect || {};
    if (!gestures?.active(el)) {
      el.style.left = (r.left || 0) + "px";
      el.style.top = (r.top || 0) + "px";
      el.style.width = (r.width || NOTE_DEFAULT.width) + "px";
      el.style.height = (r.height || NOTE_DEFAULT.height) + "px";
    }
    paintTitle(el);
    paintPath(el, record.path);
    applyLock(el, !!lockedBy(record, fences));
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
    if (typeof record.draft === "string" && !record.path) {
      if (record.claim) {
        el._noteClaim = record.claim;
        paintPath(el, record.claim);
      }
      void mountDraft(el, record.draft);
    } else {
      void loadInto(el, record);
    }
    paint(el, record, []);
    if (el._noteClaim) paintPath(el, el._noteClaim);
    return el;
  }

  // ---- a draft that crosses a detach (in `wb-notes-drafts.ts`) ----------------

  const { draftOf, drafts, adoptDraft, mountHome, mountDraft } = createNoteDrafts({
    window, cardEl, recordOf, patch, syncFromEditor, markDirty, mountEditor, paintPath,
  });

  // ---- a card on top (in `wb-notes-on-top.ts`) ---------------------------------

  const { onTop, keepOnTop, putBack, onTopNow, paintShadow, floatGesture } = createNoteOnTop({
    window, document, consoleHost, stack, stage, cardEl, recordOf, isAway, wakeCard, fragment: () => fragment,
  });

  // ---- the cards on the stage (in `wb-notes-stage.ts`) -------------------------

  const { mountCard, dropCard } = createNoteStage({
    buildCard, drafts, mountHome, loadInto, trackDormancy, onTopNow, putBack, flush, tearDownCard, isFragment: () => fragment,
  });

  // ---- opening from the explorer (ADR-0064 §11) ---------------------------------

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

  // ---- the map (in `wb-notes-map.ts`) ------------------------------------------

  const { list, jump } = createNoteMap({ consoleHost, cardEl, isAway, onTopNow });

  // Every card writes what it holds before the page goes. A best effort by
  // definition — the socket may not finish — but an 800 ms debounce loses a
  // sentence without it, and this is the same bargain `wb-console.ts` takes
  // for the desk on `pagehide`.
  function flushAll() {
    const st = stage();
    if (!st) return;
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) {
      // `flush` re-reads the editor itself, so an unannounced last keystroke
      // is carried here too.
      void flush(el);
    }
  }

  // Whether any note card holds an edit not yet saved (ADR-0070 D6). The
  // editor is asked first, like every flush point: its change notification
  // can land after the last keystroke.
  function anyDirty() {
    const st = stage();
    if (!st) return false;
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) {
      syncFromEditor(el);
      if (el._noteDirty) return true;
    }
    return false;
  }

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
