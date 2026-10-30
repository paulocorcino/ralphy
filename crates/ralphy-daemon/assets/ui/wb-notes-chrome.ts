// The note card's chrome: its head with the title field, the card's tools,
// the file menu, the body, the foot and the resize edges, and the title as a
// rename (ADR-0064 §4). `createNotes` builds it with the card's closure names
// as typed deps; the text of each part is the one it had in `wb-notes.ts`.
import { DEFAULT_DIR, NOTE_MIN, noCredential, titleOf, veiledOf, withTitle } from "./wb-notes-folds.ts";
import type { CardHost, DeskNote, NoteCard, NoteSource, Point } from "./wb-types.d.ts";

/** What `createNotes` hands the card's chrome: the card's closure names. */
export type NoteChromeDeps = {
  /** The page's document. */
  document: Document;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The desk record of a card, by id. */
  recordOf: (id: string | null | undefined) => NoteSource | null;
  /** Write fields of one card's record back to the desk. */
  patch: (id: string | null | undefined, fields: Partial<DeskNote>) => void;
  /** Put the desk's cards on the stage. */
  render: () => void;
  /** Persist the placement of a card after a gesture. */
  persistCards: (els: NoteCard | NoteCard[]) => void;
  /** Close a card and keep its file. */
  closeCard: (id: string | null | undefined) => void;
  veiledNow: (el: NoteCard) => boolean;
  toggleReveal: (el: NoteCard) => void;
  setMarked: (el: NoteCard, marked: boolean) => unknown;
  mountEditor: (el: NoteCard, markdown: string) => Promise<unknown>;
  syncFromEditor: (el: NoteCard) => void;
  markDirty: (el: NoteCard) => void;
  paintTitle: (el: NoteCard) => void;
  togglePalette: (el: NoteCard) => void;
  closePalette: () => void;
  toggleMenu: (el: NoteCard) => void;
  toggleIndex: (el: NoteCard) => void;
  closeMenu: () => void;
  markdownHelp: () => unknown;
  pointElsewhere: (el: NoteCard) => unknown;
  renameNote: (el: NoteCard) => void;
  deleteNote: (el: NoteCard) => unknown;
  commitRename: (el: NoteCard) => unknown;
  endRename: (el: NoteCard) => void;
  onTop: (el: NoteCard) => boolean;
  putBack: () => void;
  floatGesture: (el: NoteCard, dir: string | null) => (e: PointerEvent) => void;
};

export function createNoteChrome(deps: NoteChromeDeps) {
  const { document, consoleHost, recordOf, patch, render, persistCards, closeCard } = deps;
  const { veiledNow, toggleReveal, setMarked, mountEditor, syncFromEditor, markDirty, paintTitle } = deps;
  const { togglePalette, closePalette, toggleMenu, toggleIndex, closeMenu, markdownHelp } = deps;
  const { pointElsewhere, renameNote, deleteNote, commitRename, endRename, onTop, putBack, floatGesture } = deps;

  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

  // The parts of one card, built in no order: `buildCard` appends them in the
  // order of the hit test.
  function buildCardParts(el: NoteCard) {
    const head = document.createElement("div");
    head.className = "note-head";
    const grab = document.createElement("span");
    grab.className = "note-grab";
    grab.title = "Move this note";
    // Bootstrap Icons, like every other control on the plane: the card's
    // chrome was the one surface drawing its controls as text characters, and
    // a braille-dots grip beside a console's `bi-grip-vertical` reads as a
    // different application. The console's titlebar is the reference
    // (`buildChrome`), and a test pins the characters back OUT.
    grab.innerHTML = '<i class="bi bi-grip-vertical"></i>';
    const title = document.createElement("span");
    title.className = "note-title";
    title.title = "Rename this note";
    // Renaming IS editing the first `#` heading (ADR-0064 §4) — the title has
    // no storage of its own, so this field writes the document. An INPUT and
    // not `contenteditable`: the head is a drag handle, and a caret inside a
    // handle is two gestures on one pixel.
    const titleEdit = document.createElement("input");
    titleEdit.className = "note-title-edit";
    titleEdit.setAttribute("aria-label", "Note title");
    noCredential(titleEdit);
    titleEdit.hidden = true;
    titleEdit.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") commitTitle(el);
      else if (ev.key === "Escape") endTitle(el);
    });
    titleEdit.addEventListener("blur", () => commitTitle(el));
    // A PRESS THAT DID NOT MOVE opens the field, and that shape is the whole
    // of it. The title is `flex: 1` and therefore most of the head, which is
    // the drag handle: swallowing its `pointerdown` (the obvious way to stop a
    // rename from arming a drag) left the card draggable only by the 10 px of
    // grip beside it — MEASURED, a drag across the title moved nothing. So
    // nothing is stopped on the way down, and the decision is taken on the way
    // up, against the same 3 px the gesture calls a drag.
    //
    // The `mousedown` default IS cancelled, and that is load-bearing for the
    // other half — MEASURED: it moves focus to the nearest focusable ancestor,
    // which blurred the field this press had just opened, and `blur` commits,
    // so the field closed inside the same click and the rename never happened.
    let press: Point | null = null;
    title.addEventListener("pointerdown", (ev) => {
      press = { x: ev.clientX, y: ev.clientY };
    });
    title.addEventListener("mousedown", (ev) => ev.preventDefault());
    title.addEventListener("pointerup", (ev) => {
      const from = press;
      press = null;
      if (!from) return;
      if (Math.abs(ev.clientX - from.x) > 3 || Math.abs(ev.clientY - from.y) > 3) return;
      beginTitle(el);
    });
    head.append(grab, title, titleEdit);

    const tools = document.createElement("div");
    tools.className = "note-tools";
    // Colour is the FILE's (front matter), so a close and a reopen keep it —
    // which is why this writes the document and not the record.
    const tone = document.createElement("button");
    tone.className = "note-tone";
    tone.type = "button";
    tone.title = "Change the colors and the font";
    tone.innerHTML = '<i class="bi bi-palette"></i>';
    tone.addEventListener("click", (ev) => {
      ev.stopPropagation();
      togglePalette(el);
    });
    const lock = document.createElement("button");
    lock.className = "note-lock";
    lock.type = "button";
    lock.addEventListener("click", () => {
      const r = recordOf(el.dataset.noteId);
      patch(el.dataset.noteId, { locked: !r?.locked });
      render();
    });
    // The card's own index (ADR-0064 §10). The `Note` menu already maps every
    // card's `##`; this is the same fold read from ONE card, in its head,
    // where an operator scrolling a long note is already looking. It does not
    // move the card — the note is already in front of them.
    const index = document.createElement("button");
    index.className = "note-index";
    index.type = "button";
    index.title = "Go to a heading in this note";
    index.hidden = true;
    index.innerHTML = '<i class="bi bi-list-ul"></i>';
    index.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleIndex(el);
    });
    // The eye (ADR-0064 §8 as amended). Shown ONLY on a note that carries
    // `hidden: true`, because on any other card it would be a control for a
    // state that card is not in — the same rule `Use another file…` follows.
    // It shows and re-hides for THIS SESSION and writes nothing: the mark is
    // what the file carries, and looking at a note is not a change to it.
    const veil = document.createElement("button");
    veil.className = "note-veil";
    veil.type = "button";
    veil.addEventListener("click", (ev) => {
      ev.stopPropagation();
      // ALWAYS THERE, and the act depends on the card's state: a note nobody
      // has hidden is hidden by this press (the file write the `⋯` menu also
      // offers); a hidden one is shown and put away again, which writes
      // nothing. The operator asked for the eye to be available on every card
      // (2026-09-22) — a control that appears only once the state is already
      // reached is not a door into it.
      if (!veiledOf(el._noteMarkdown)) setMarked(el, true);
      else toggleReveal(el);
    });
    // THE FILE'S OWN CONTROL, and it lives with the file: a gear in the
    // footer's left corner, beside the path it acts on (asked 2026-09-22).
    // It was a `⋯` in the head, which put the delete two pixels from the
    // close button and read as "more of the head's controls" — but nothing in
    // this menu is about the card. The footer already names the file; this is
    // the button that changes it.
    const more = document.createElement("button");
    more.className = "note-more";
    more.type = "button";
    more.title = "File and Markdown help";
    more.innerHTML = '<i class="bi bi-gear"></i>';
    more.addEventListener("click", (ev) => {
      ev.stopPropagation();
      toggleMenu(el);
    });
    const close = document.createElement("button");
    close.className = "note-close";
    close.type = "button";
    close.title = "Close this note. The file is kept.";
    close.innerHTML = '<i class="bi bi-x-lg"></i>';
    close.addEventListener("click", () => closeCard(el.dataset.noteId));
    // Shown only while the card is on top, in the place of `✕` (CSS): the
    // close removes the card from the desk, which is not what the operator
    // means when they want the card out of the way.
    const back = document.createElement("button");
    back.className = "note-putback";
    back.type = "button";
    back.title = "Put back";
    back.innerHTML = '<i class="bi bi-box-arrow-in-down-left"></i>';
    back.addEventListener("click", (ev) => {
      ev.stopPropagation();
      putBack();
    });
    tools.append(tone, index, veil, lock, close, back);

    // The file actions (ADR-0064 §11), in a menu rather than in the head: they
    // act on the FILE, not on the card, and two more glyphs beside the close
    // button is where a slip becomes a deletion.
    const menu = document.createElement("div");
    menu.className = "note-menu-card";
    menu.hidden = true;
    const renameItem = document.createElement("button");
    renameItem.className = "note-menu-item note-menu-file";
    renameItem.type = "button";
    renameItem.textContent = "Rename file…";
    renameItem.addEventListener("click", () => {
      closeMenu();
      renameNote(el);
    });
    const drop = document.createElement("button");
    drop.className = "note-menu-item note-menu-file danger";
    drop.type = "button";
    drop.textContent = "Delete file…";
    drop.addEventListener("click", () => {
      closeMenu();
      deleteNote(el);
    });
    // ONLY the missing card's verb, and hidden otherwise (ADR-0064 §11: a
    // missing card's tools shrink to *remove* and *point elsewhere*). Offered
    // beside `Rename file…` on a healthy note it read as a second, cryptic
    // rename — "I don't know what this point… is" (operator, 2026-09-22) —
    // for a state that card is not in.
    const point = document.createElement("button");
    point.className = "note-menu-item note-menu-point";
    point.type = "button";
    point.textContent = "Use another file…";
    point.title = "The file of this note is gone. Choose another file.";
    point.hidden = true;
    point.addEventListener("click", () => {
      closeMenu();
      pointElsewhere(el);
    });
    // UNMARKING ONLY, and hidden on a note that is not marked — the rule
    // `Use another file…` follows, for the same reason. The operator asked for
    // `Hide this note` to go (2026-09-22): the eye in the head hides any card,
    // so the entry was a second door to a place that already had one. The
    // OTHER direction is not a duplicate of anything — the eye can mark and
    // reveal but never unmark, so deleting the item outright would strand a
    // veiled note with no way back — and it is what stays here.
    const mark = document.createElement("button");
    mark.className = "note-menu-item note-menu-mark";
    mark.type = "button";
    mark.textContent = "Stop hiding this note";
    mark.hidden = true;
    mark.addEventListener("click", () => {
      closeMenu();
      setMarked(el, false);
    });
    // Not a file action, which is why it is under a rule: the two above write
    // the disk and this one only says how to write the note (the sheet itself
    // is `markdownHelp`).
    const help = document.createElement("button");
    help.className = "note-menu-item note-menu-help";
    help.type = "button";
    help.textContent = "Markdown help";
    help.addEventListener("click", () => {
      closeMenu();
      markdownHelp();
    });
    menu.append(renameItem, point, drop, mark, help);

    // The index's own dropdown, filled on each open from the LIVE document —
    // the headings change with every keystroke and a list built once would
    // name sections that are no longer there.
    const anchors = document.createElement("div");
    anchors.className = "note-menu-card note-anchors";
    anchors.hidden = true;

    const body = document.createElement("div");
    body.className = "note-body";

    const foot = document.createElement("div");
    foot.className = "note-foot";
    const path = document.createElement("span");
    path.className = "note-path";
    // Where an unsaved note will land (ADR-0064 §9: creation has no dialog, the
    // directory is a field in the footer). Gone once the file exists — moving a
    // note is a rename, not a re-aim.
    const dir = document.createElement("input");
    dir.className = "note-dir";
    dir.setAttribute("aria-label", "Folder for this note");
    noCredential(dir);
    dir.value = DEFAULT_DIR;
    dir.title = "Where this note is saved";
    // The plane's accelerators must not fire on a directory being typed.
    dir.addEventListener("keydown", (e) => e.stopPropagation());
    // The rename field (ADR-0064 §11), in the footer beside the path it
    // replaces while an edit is open. An INPUT and not `window.prompt`: the
    // native dialog is dismissed by default in an automated browser, which is
    // exactly the objection `wb-messages.ts` records against `window.confirm`.
    const rename = document.createElement("input");
    rename.className = "note-rename";
    rename.setAttribute("aria-label", "New file name for this note");
    noCredential(rename);
    rename.hidden = true;
    rename.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter") commitRename(el);
      else if (ev.key === "Escape") endRename(el);
    });
    rename.addEventListener("blur", () => endRename(el));
    const state = document.createElement("span");
    state.className = "note-state";
    foot.append(more, path, dir, rename, state);

    // Every edge and corner resizes; only the SE grip is visible, as a fence's.
    const handles = DIRS.map((d) => {
      const h = document.createElement("div");
      h.className = d === "se" ? "note-edge note-grip" : "note-edge";
      h.dataset.dir = d;
      h.addEventListener(
        "pointerdown",
        consoleHost?.startResize(el, d, {
          locked: () => !!el._noteLocked || onTop(el),
          onDrop: () => persistCards(el),
          min: NOTE_MIN,
        }) ?? (() => {}),
      );
      h.addEventListener("pointerdown", floatGesture(el, d));
      return h;
    });
    return { handles, head, body, foot, tools, menu, anchors };
  }

  // ---- the title, as a rename (ADR-0064 §4) -------------------------------------

  function beginTitle(el: NoteCard) {
    // Veiled for `togglePalette`'s reason: the card is holding a header, not
    // the note, and a retitle writes the whole document. A locked card renames
    // like any other.
    if (veiledNow(el)) return;
    closePalette();
    closeMenu();
    const label = el.querySelector<HTMLElement>(".note-title");
    const field = el.querySelector<HTMLInputElement>(".note-title-edit");
    if (!label || !field) return;
    field.value = titleOf(el._noteMarkdown, "");
    label.hidden = true;
    field.hidden = false;
    field.focus();
    field.select();
  }
  function endTitle(el: NoteCard) {
    const label = el.querySelector<HTMLElement>(".note-title");
    const field = el.querySelector<HTMLInputElement>(".note-title-edit");
    if (!label || !field) return;
    field.hidden = true;
    label.hidden = false;
  }
  function commitTitle(el: NoteCard) {
    const field = el.querySelector<HTMLInputElement>(".note-title-edit");
    // `blur` is also what a teardown fires: a card removed with the field open
    // must not remount an editor onto a node that has left the document.
    if (!field || field.hidden || !el.isConnected) return;
    const next = field.value;
    endTitle(el);
    if (next === titleOf(el._noteMarkdown, "")) return;
    syncFromEditor(el);
    el._noteMarkdown = withTitle(el._noteMarkdown, next);
    // The editor holds the document, so a heading it did not write must be
    // handed back to it — a remount is the whole of `CrepeLean`'s surface for
    // "here is different text", and a retitle is rare enough to afford one.
    // The FILE keeps its name: `⋯ → Rename file…` is the other verb.
    void mountEditor(el, el._noteMarkdown).then(() => markDirty(el));
    paintTitle(el);
  }

  return { buildCardParts };
}
