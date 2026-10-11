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
import { WBFail } from "./wb-fail.ts";
import { WBGeometry } from "./wb-geometry.ts";
import { addNote, removeNote, NOTE_MAX } from "./wb-desk-folds.ts";
import { DEFAULT_DIR, DEFAULT_FILL, DEFAULT_FONT, DEFAULT_INK, DEFAULT_SIZE, DEFAULT_TONE, FILLS, FONTS, INKS, NEW_NOTE_STYLE, NOTE_DEFAULT, NOTE_MIN, ON_TOP_BAND_BELOW, ON_TOP_FLOOR, ON_TOP_TOP, SAVE_AFTER_MS, SIZES, SWATCH_NAME, TONES, anchorsOf, applyLook, baseName, bodyOf, colorOf, dirName, fillOf, fontOf, inkOf, isSafeScheme, lockedBy, lookOf, noCredential, noteDormancyDecision, noteSlug, onTopClamp, onTopRect, popupLinkTarget, savedLabel, sizeOf, spawnRect, stampName, styleOf, titleFieldOf, titleOf, toneOf, veiledOf, withStyle, withTitle, withVeil } from "./wb-notes-folds.ts";
import type { FloatBox } from "./wb-notes-folds.ts";
import { sendDocument } from "./wb-events.ts";
import { DORMANT_AFTER_MS, DORMANT_MARGIN_PX } from "./wb-console-gpu.ts";
import { dragThreshold, dragBegins } from "./wb-console-input.ts";
import { createNoteEditor } from "./wb-notes-editor.ts";
import type { CrepeLean } from "./wb-notes-editor.ts";
import type { Messages } from "./wb-messages.ts";
import type { Gestures, Stack } from "./wb-stage-stack.ts";
import type { CardHost, DeskFence, DeskNote, NoteCard, NoteEditor, NoteLook, NoteSource, Offset, Point, Size } from "./wb-types.d.ts";

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

/** A draft that came home with a re-attach: the text and the name it chose. */
type Home = { draft: string; claim: string | null };

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

  const DIRS = ["n", "s", "e", "w", "ne", "nw", "se", "sw"];

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
    const next = (consoleHost?.notes?.() || []).map((n) =>
      moved.has(n.id) ? { ...n, rect: moved.get(n.id)! } : n,
    );
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
    installLinks(el);
    // Leaving the card writes what was typed: `focusout` fires before whatever
    // the operator clicked next does anything (ADR-0064 §7).
    el.addEventListener("focusout", () => {
      // Unconditional: `flush` asks the editor what it holds and returns at
      // once when there is nothing to write, so the last character typed
      // before the pointer left the body is carried even though the editor's
      // change listener has not fired yet.
      flush(el);
    });
    stage()?.append(el);
    // In the WINDOW tier from the moment it exists. A card built by a restore
    // is never focused, so without this it kept `z-index: auto` and every
    // console painted over it — see the stack's `stackWin`.
    stack?.stackWin(el);
    return el;
  }

  // ---- the editor (in `wb-notes-editor.ts`) ------------------------------------

  const { mountEditor, loadInto, paintTitle, paintState, syncFromEditor, markDirty, flush, paintPath } =
    createNoteEditor({ window, document, consoleHost, recordOf, patch, render, paintIndex, veilPanel, paintVeil, fragment: () => fragment });

  // The tone cycles through the closed set and is written into the FILE, so a
  // close and a reopen keep it (ADR-0064 §3).
  // ---- the palette (ADR-0064 §8, amended 2026-09-22) ---------------------------

  // The ground and the ink, as two rows of swatches. The pair is the
  // operator's: the tones are desaturated by design, so a card that must READ
  // as yellow needs the solid fill, and a solid fill needs an ink chosen for
  // it. Rendered into the same popover so the two are picked together and the
  // card repaints under the pointer.
  let openPalette: HTMLElement | null = null;
  function togglePalette(el: NoteCard) {
    const pop = el.querySelector<HTMLElement>(".note-palette");
    // A VEILED card is refused: it holds only its header, so writing the
    // document from here would save that header OVER the note. A LOCKED one is
    // not — the lock pins the card, it does not freeze the note.
    if (!pop || veiledNow(el)) return;
    const wasOpen = !pop.hidden;
    closePalette();
    closeMenu();
    if (wasOpen) return;
    paintPalette(el);
    pop.hidden = false;
    openPalette = pop;
    document.addEventListener("pointerdown", closePaletteOutside, true);
  }
  function closePalette() {
    if (!openPalette) return;
    openPalette.hidden = true;
    openPalette = null;
    document.removeEventListener("pointerdown", closePaletteOutside, true);
  }
  function closePaletteOutside(ev: PointerEvent) {
    if (openPalette && !openPalette.contains(ev.target as Node | null) && !(ev.target as Element | null)?.closest?.(".note-tone")) {
      closePalette();
    }
  }

  function buildPalette(el: NoteCard) {
    const pop = document.createElement("div");
    pop.className = "note-palette";
    pop.hidden = true;
    // The press must not reach the card's drag or the plane's accelerators,
    // and the swatch must not blur the editor before it repaints.
    pop.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    pop.addEventListener("mousedown", (ev) => ev.preventDefault());
    // `text` turns the strip from swatches into CHIPS — a colour can be shown
    // as itself, a font and a size cannot, so those two rows name what they
    // offer and the font chips are drawn IN the font they name.
    const row = (label: string, cls: string, names: string[], pick: (el: NoteCard, name: string) => void, text?: (name: string) => string) => {
      const strip = document.createElement("div");
      strip.className = "note-swatches";
      const cap = document.createElement("span");
      cap.className = "note-swatch-label";
      cap.textContent = label;
      strip.append(cap);
      for (const name of names) {
        const b = document.createElement("button");
        b.className = cls;
        b.type = "button";
        b.dataset.name = name;
        b.title = SWATCH_NAME[name] || name;
        if (text) b.textContent = text(name);
        b.setAttribute("aria-label", label + ": " + (SWATCH_NAME[name] || name));
        b.addEventListener("click", (ev) => {
          ev.stopPropagation();
          pick(el, name);
        });
        strip.append(b);
      }
      pop.append(strip);
    };
    row("Color", "note-swatch note-swatch-tone", TONES, setTone);
    row("Fill", "note-swatch note-swatch-fill", FILLS, setFill);
    row("Text", "note-swatch note-swatch-ink", INKS, setInk);
    // `Aa` in each face, and the step names for the size: the chip is a sample
    // of what pressing it does, which is the only honest label a font has.
    row("Font", "note-swatch note-swatch-font", FONTS, setFont, () => "Aa");
    row("Size", "note-swatch note-swatch-size", SIZES, setSize, (n) => n.toUpperCase());
    return pop;
  }

  // Which swatch is the card's now — read off the card, so a palette opened on
  // a note whose file was hand-edited shows what the file says.
  function paintPalette(el: NoteCard) {
    const pop = el.querySelector<HTMLElement>(".note-palette");
    if (!pop) return;
    const have: NoteLook = lookOf(el);
    for (const [key, cls] of [
      ["tone", "note-swatch-tone"],
      ["fill", "note-swatch-fill"],
      ["ink", "note-swatch-ink"],
      ["font", "note-swatch-font"],
      ["size", "note-swatch-size"],
    ]) {
      for (const b of pop.querySelectorAll<HTMLElement>("." + cls)) {
        b.classList.toggle("is-on", b.dataset.name === have[key as keyof NoteLook]);
      }
    }
  }

  // One restyle: the card repaints at once and the DOCUMENT is what carries
  // it, because the look is the file's (ADR-0064 §8) — close the card and
  // reopen it and the colours come back with the text.
  function restyle(el: NoteCard, next: Partial<NoteLook>) {
    applyLook(el, { ...lookOf(el), ...next });
    // Through `syncFromEditor` first: the operator may have typed a character
    // the change listener has not reported yet, and a restyle rewrites the
    // whole document.
    syncFromEditor(el);
    el._noteMarkdown = withStyle(el._noteMarkdown, lookOf(el));
    // A mermaid fence is an SVG with its colours written into it as inline
    // fills (`entry.js` seeds them from this card), so the cascade cannot
    // reach it — without this, restyling a note left every diagram in it
    // wearing the tone the card used to be.
    try {
      el._noteEditor?.redrawDiagrams?.();
    } catch {}
    paintPalette(el);
    markDirty(el);
  }
  const setTone = (el: NoteCard, tone: string) => restyle(el, { tone });
  const setFill = (el: NoteCard, fill: string) => restyle(el, { fill });
  const setInk = (el: NoteCard, ink: string) => restyle(el, { ink });
  const setFont = (el: NoteCard, font: string) => restyle(el, { font });
  const setSize = (el: NoteCard, size: string) => restyle(el, { size });

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
    mountEditor(el, el._noteMarkdown).then(() => markDirty(el));
    paintTitle(el);
  }

  // ---- links, keys, dormancy ----------------------------------------------------

  // A link inside a note obeys the viewer's rule (ADR-0064 §12): an external
  // one opens in a new tab, a repo-relative one is an open REQUEST to the
  // shell. A raw `href` would navigate the whole workbench away.
  function installLinks(el: NoteCard) {
    el.addEventListener("click", (ev) => {
      const a = (ev.target as Element | null)?.closest?.<HTMLAnchorElement>("a[href]");
      if (!a || !el.contains(a)) return;
      const record = recordOf(el.dataset.noteId);
      // Resolved against the CHECKOUT ROOT, not the note's own directory: a
      // note lands in `.ralphy/notes/` by default, and a path written there is
      // the operator's path into their repo, not into the run-state directory.
      const href = a.getAttribute("href");
      const target = window.WBViewer?.linkTarget?.("", href) || popupLinkTarget(href);
      if (!target || target.kind === "fragment") {
        ev.preventDefault();
        return;
      }
      if (target.kind === "external") {
        // The SCHEME is an allowlist, and that is a security control, not
        // tidiness. The viewer's `linkTarget` answers "external" for ANY
        // scheme and leaves it to the browser — safe there only because the
        // viewer's markdown went through DOMPurify first (wb-file-viewer.ts), which
        // drops a `javascript:` href. A note's markdown never meets that pass:
        // Crepe renders the link straight into the document, so a `.note` file
        // — repo bytes, which an agent may have written — could otherwise run
        // script on the daemon's own origin with one click.
        if (!isSafeScheme(href)) {
          ev.preventDefault();
          return;
        }
        a.target = "_blank";
        a.rel = "noopener";
        return;
      }
      ev.preventDefault();
      sendDocument(document, "workbench:open-request", {
        project: record?.repo || null,
        path: target.path,
        fragment: target.fragment,
        checkout: record?.checkout ?? null,
      });
    });
  }

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
        flush(el);
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

  // Dormancy (ADR-0064 §14). Its OWN observer, not the consoles': a console
  // sleeps by dropping a socket, a card by destroying an editor, and the two
  // have different refusals — a card with unsaved text never sleeps.
  let observer: IntersectionObserver | null = null;
  let sweeper: ReturnType<typeof setInterval> | null = null;
  const watched = new Map<NoteCard, { visible: boolean; since: number }>();
  const SWEEP_MS = 5000;
  function trackDormancy(el: NoteCard) {
    if (typeof window.IntersectionObserver !== "function") return;
    const root = document.getElementById("workspace");
    if (!root) return;
    if (!observer) {
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const seen = watched.get(entry.target as NoteCard);
            if (!seen) continue;
            seen.visible = entry.isIntersecting;
            seen.since = Date.now();
            if (entry.isIntersecting) wakeCard(entry.target as NoteCard);
          }
        },
        { root, rootMargin: `${DORMANT_MARGIN_PX}px` },
      );
    }
    if (sweeper == null) sweeper = setInterval(sweepDormancy, SWEEP_MS);
    watched.set(el, { visible: true, since: Date.now() });
    observer.observe(el);
  }
  function untrackDormancy(el: NoteCard) {
    watched.delete(el);
    observer?.unobserve(el);
    // Nothing left to watch: a page that once showed a note must not keep a
    // 5 s timer and an observer alive for the document's life — a popup that
    // reattached its fence would carry both forever.
    if (!watched.size) {
      if (sweeper != null) clearInterval(sweeper);
      sweeper = null;
      observer?.disconnect();
      observer = null;
    }
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
  function sweepDormancy() {
    const after = DORMANT_AFTER_MS;
    for (const [el, seen] of [...watched]) {
      if (!el.isConnected) {
        untrackDormancy(el);
        continue;
      }
      const verdict = noteDormancyDecision({
        visible: seen.visible,
        onTop: el.dataset.noteId === onTopId,
        dirty: !!el._noteDirty,
        inFlight: !!el._noteInFlight,
        asleep: !!el._noteAsleep,
        elapsed: Date.now() - seen.since,
        after,
      });
      if (verdict === "sleep") sleepCard(el);
    }
  }
  function sleepCard(el: NoteCard) {
    if (el._noteAsleep || el._noteDirty || el._noteInFlight) return;
    // A veiled card has already given its editor back and is already showing
    // the one line it is allowed to show; putting it to sleep would replace
    // that with the "asleep" line and lose what the card is saying.
    if (veiledNow(el)) return;
    el._noteAsleep = true;
    const editor = el._noteEditor;
    el._noteEditor = null;
    try {
      editor?.destroy();
    } catch {}
    const body = el.querySelector<HTMLElement>(".note-body");
    if (body) {
      body.textContent = "";
      const p = document.createElement("p");
      p.className = "note-asleep";
      p.textContent = titleOf(el._noteMarkdown, "Untitled note");
      body.append(p);
    }
  }
  function wakeCard(el: NoteCard) {
    if (!el._noteAsleep) return;
    el._noteAsleep = false;
    const record = recordOf(el.dataset.noteId);
    // Re-READ rather than remount the text held here: another page may have
    // written the file while this card slept, and the file is the note.
    if (record) loadInto(el, record);
  }

  // ---- creating, closing, rendering ---------------------------------------------

  // One rule for both ways a card is born: the cap refuses, it does not evict
  // (see `saveNotes`); the new record lands at the end of the desk's list.
  function addCard(fields: { repo: string; checkout: string | null; path: string }, { viewport, offset }: NewNote) {
    const records = consoleHost?.notes?.() || [];
    const record = {
      id: `note-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
      ...fields,
      rect: spawnRect(
        viewport || { width: 0, height: 0 },
        offset || { left: 0, top: 0 },
        records.length,
        consoleHost?.fenceRecords?.() || [],
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
    written.then(() => {
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
    const records = consoleHost?.notes?.() || [];
    const fences = consoleHost?.fenceRecords?.() || [];
    const nodes = new Map();
    for (const el of st.querySelectorAll<NoteCard>(".note-card")) nodes.set(el.dataset.noteId, el);
    const seen = new Set();
    for (const record of records) {
      if (isAway(record, fences)) continue;
      // In a fragment, an unknown record is not this window's to show.
      if (fragment && !nodes.has(record.id)) continue;
      seen.add(record.id);
      let el = nodes.get(record.id);
      if (!el) {
        el = buildCard(record);
        const home = record.path ? undefined : drafts.get(record.id);
        drafts.delete(record.id);
        if (home) mountHome(el, record, home);
        else loadInto(el, record);
        trackDormancy(el);
      }
      el._noteListed = true;
      paint(el, record, fences);
    }
    for (const [id, el] of nodes) {
      if (seen.has(id)) continue;
      // A popup card whose record this document's desk has never held (a
      // note created a moment before the detach) is not gone: it is the
      // snapshot's. Tearing it down would drop its text unsaved.
      if (fragment && el._noteOrphan && !el._noteListed) continue;
      // A card leaving the stage takes its editor with it; the record it was
      // built from has already gone (closed) or moved (detached). It is put
      // back first, so a detach carries it from its place and not from the
      // corner, and what was typed is written before the editor goes: a
      // detach, or a close from another client, tears the card down with no
      // other flush on the way. A NAMED note only: an unnamed one would be
      // named here while a detach popup opens the same record with no path,
      // and the two would write two files.
      if (id === onTopId) putBack();
      if (el._noteRecord?.path) {
        el._noteOrphan = el._noteRecord;
        flush(el).catch(() => {});
      }
      tearDownCard(el);
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
      mountDraft(el, record.draft);
    } else {
      loadInto(el, record);
    }
    paint(el, record, []);
    if (el._noteClaim) paintPath(el, el._noteClaim);
    return el;
  }

  // ---- a draft that crosses a detach (ADR-0064 §8, amended for #475) -----------

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
  function adoptDraft(id: string | null | undefined, draft: string | null | undefined, claim: string | null | undefined) {
    if (id && typeof draft === "string") drafts.set(id, { draft, claim: claim || null });
  }

  // A card built from a draft that came home. When the popup had chosen a
  // name, the file under that name is the newer text if it exists: it is
  // read, and the name is recorded. Otherwise the draft is mounted and
  // written under that name, so a re-attach never makes a second file.
  function mountHome(el: NoteCard, record: NoteSource, home: Home) {
    if (!home.claim) return mountDraft(el, home.draft);
    el._noteClaim = home.claim;
    paintPath(el, home.claim);
    return window.WBDaemon.observe(
      "note.read",
      window.WBDaemon.withCheckout({ repo: record.repo, path: home.claim }, record.checkout),
    )
      .then((reply): Promise<NoteEditor | null | void> | null => {
        if (el._noteGone) return null;
        if (!WBFail.isError(reply)) {
          el._noteClaim = null;
          patch(record.id, { path: home.claim! });
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

  // ---- a card on top (ADR-0064, 2026-09-26 amendment) ---------------------------

  // The one card on top in THIS tab, or null. Memory only: the desk, the
  // per-client view and a reload never see it, so a reload finds the card in
  // its place.
  let onTopId: string | null = null;

  function onTop(el: NoteCard) {
    return el.classList.contains("on-top");
  }

  function viewportSize() {
    const ws = document.getElementById("workspace");
    return { width: ws?.clientWidth || 0, height: ws?.clientHeight || 0 };
  }

  // The floating box lives in CSS variables and NEVER in the inline rect: the
  // inline rect stays the desk rect, as a maximized window's does, so a fence
  // move, membership and `persistCards` keep reading the card's place.
  // `--ws-*` is the viewport's box on the screen: the card is `position:
  // fixed` (no ancestor of the stage has a transform, a filter or `contain`),
  // so a scroll of the stage never has to move it and it does not shake
  // during a pan.
  function placeOnTop(el: NoteCard, box: FloatBox) {
    const ws = document.getElementById("workspace");
    if (ws) {
      const r = ws.getBoundingClientRect();
      el.style.setProperty("--ws-x", r.left + ws.clientLeft + "px");
      el.style.setProperty("--ws-y", r.top + ws.clientTop + "px");
      el.style.setProperty("--ws-w", ws.clientWidth + "px");
      el.style.setProperty("--ws-h", ws.clientHeight + "px");
    }
    el.classList.toggle("band", !!box.band);
    if (box.band) {
      el._noteOnTop = null;
      return;
    }
    el._noteOnTop = { left: box.left, top: box.top, width: box.width, height: box.height };
    el.style.setProperty("--ot-x", box.left + "px");
    el.style.setProperty("--ot-y", box.top + "px");
    el.style.setProperty("--ot-w", box.width + "px");
    el.style.setProperty("--ot-h", box.height + "px");
  }

  // The card's place while it floats: the desk rect, the tone and the title,
  // and no editor. A click on it puts the card back.
  function paintShadow(el: NoteCard) {
    let sh = el._noteShadow;
    if (!sh) {
      sh = document.createElement("div");
      sh.className = "note-shadow";
      sh.title = "Kept on top. Click to put back.";
      const name = document.createElement("span");
      name.className = "note-shadow-title";
      sh.append(name);
      sh.addEventListener("click", () => putBack());
      stage()?.append(sh);
      el._noteShadow = sh;
    }
    for (const side of ["left", "top", "width", "height"] as const) sh.style[side] = el.style[side];
    sh.style.setProperty("--note-tone", getComputedStyle(el).getPropertyValue("--note-tone"));
    sh.firstChild!.textContent = titleOf(el._noteMarkdown, "Untitled note");
  }

  // Float the card in front of the windows (decisions 1–4). One card at a
  // time: another card on top goes back first. Refused for a card that is in
  // a detached fence's popup (`isAway`). Inside the popup itself the card is
  // on top of that window (ADR-0064 §7, amended 2026-10-05); its record may be
  // the orphan `mountDetached` kept, when this window's desk does not hold it.
  function keepOnTop(id: string | null | undefined) {
    const el = cardEl(id);
    const record = recordOf(id) || (fragment ? el?._noteOrphan : null);
    if (!record || !el) return false;
    if (isAway(record, consoleHost?.fenceRecords?.() || [])) return false;
    if (onTopId === id) return true;
    putBack();
    wakeCard(el);
    onTopId = id!;
    el.classList.add("on-top");
    placeOnTop(el, onTopRect(record.rect || NOTE_DEFAULT, viewportSize()));
    paintShadow(el);
    watchViewport();
    stack?.focusWin(el);
    return true;
  }

  // Back to the desk rect, which the inline style never stopped holding.
  function putBack() {
    const id = onTopId;
    onTopId = null;
    const el = id ? cardEl(id) : null;
    if (!el) return;
    el.classList.remove("on-top", "band");
    for (const v of ["--ot-x", "--ot-y", "--ot-w", "--ot-h", "--ws-x", "--ws-y", "--ws-w", "--ws-h"]) {
      el.style.removeProperty(v);
    }
    el._noteOnTop = null;
    el._noteShadow?.remove();
    el._noteShadow = null;
    viewportWatch?.disconnect();
    viewportWatch = null;
    // Back on the plane the card keeps the z it was last focused with, which
    // is above a maximized console that covered its place before it floated.
    consoleHost?.raiseMaximized?.();
  }

  function onTopNow() {
    return onTopId;
  }

  // A change of the viewport's size — a window resize, a side panel that
  // opens, the tab coming back — keeps the floating card inside the view and
  // moves it in and out of the band. Observed only while a card is on top.
  let viewportWatch: ResizeObserver | null = null;
  function watchViewport() {
    if (viewportWatch || typeof window.ResizeObserver !== "function") return;
    const ws = document.getElementById("workspace");
    if (!ws) return;
    viewportWatch = new ResizeObserver(refitOnTop);
    viewportWatch.observe(ws);
  }
  function refitOnTop() {
    const el = onTopId ? cardEl(onTopId) : null;
    if (!el) return;
    const vp = viewportSize();
    // A hidden Consoles tab measures 0×0, which would read as a phone.
    if (!vp.width || !vp.height) return;
    const record = recordOf(onTopId);
    placeOnTop(
      el,
      el._noteOnTop ? onTopClamp(el._noteOnTop, vp) : onTopRect(record?.rect || NOTE_DEFAULT, vp),
    );
  }
  // The observer sees the viewport's SIZE; a window resize can also move it.
  window.addEventListener?.("resize", refitOnTop);

  // Drag (`dir` null) or resize the floating box. The plane's own gestures
  // cannot do this: they write the inline rect and persist it. Nothing here is
  // persisted — the floating box is thrown away when the card goes back.
  function floatGesture(el: NoteCard, dir: string | null) {
    return (e: PointerEvent) => {
      if (!onTop(el) || el.classList.contains("band") || !el._noteOnTop) return;
      if (e.button !== 0 || !e.isPrimary) return;
      if (!dir && (e.target as Element).closest("button, input")) return;
      const start = { ...el._noteOnTop };
      const vp = viewportSize();
      const from = { x: e.clientX, y: e.clientY };
      const pointerId = e.pointerId;
      const threshold = dragThreshold(e.pointerType);
      let armed = false;
      const onMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerId) return;
        if (ev.buttons === 0) {
          onUp();
          return;
        }
        const at = { x: ev.clientX, y: ev.clientY };
        if (!armed) {
          if (!dragBegins(from, at, threshold)) return;
          armed = true;
        }
        const delta = { dx: at.x - from.x, dy: at.y - from.y };
        const box = dir
          ? WBGeometry.resizeRect(dir, start, delta, NOTE_MIN, vp)
          : onTopClamp({ ...start, left: start.left + delta.dx, top: start.top + delta.dy }, vp);
        placeOnTop(el, box);
      };
      const onUp = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("pointercancel", onUp);
        window.removeEventListener("blur", onUp);
      };
      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("pointercancel", onUp);
      window.addEventListener("blur", onUp);
      e.preventDefault();
      if (dir) e.stopPropagation();
    };
  }

  // ---- the file actions (ADR-0064 §11) ------------------------------------------

  // The head's dropdowns — the `⋯` file menu and the `☰` index. ONE open at a
  // time across the whole plane, closed by the next press anywhere else: the
  // card is a small surface and a menu left open over the text is in the way
  // of the thing it belongs to. They share this opener because they share that
  // rule; two independent close-on-outside listeners left one hanging when the
  // other opened.
  let openMenu: HTMLElement | null = null;
  let openMenuTrigger = "";
  function openDrop(el: NoteCard, popSelector: string, triggerSelector: string, fill: (pop: HTMLElement) => boolean | void) {
    const pop = el.querySelector<HTMLElement>(popSelector);
    if (!pop) return;
    const wasOpen = !pop.hidden;
    closeMenu();
    closePalette();
    if (wasOpen) return;
    if (fill && fill(pop) === false) return;
    pop.hidden = false;
    openMenu = pop;
    openMenuTrigger = triggerSelector;
    document.addEventListener("pointerdown", closeOnOutside, true);
  }
  function toggleMenu(el: NoteCard) {
    openDrop(el, ".note-menu-card:not(.note-anchors)", ".note-more", (menu) => {
      const record = recordOf(el.dataset.noteId);
      // Only a saved note in the PRIMARY tree has file actions: a worktree note
      // cannot be renamed or deleted yet (ADR-0064, amendment: `note.write` is
      // the only Write verb that crosses the worktree gate).
      const writable = !!record?.path && !record?.checkout;
      const missing = el.classList.contains("missing");
      // Renaming and deleting need a file that is there.
      for (const item of menu.querySelectorAll<HTMLButtonElement>(".note-menu-file")) {
        item.disabled = !writable || missing;
      }
      // Only on a note that IS marked: hiding is the eye's, and this is the
      // one door out of it.
      const mark = menu.querySelector<HTMLButtonElement>(".note-menu-mark");
      if (mark) {
        mark.hidden = !veiledOf(el._noteMarkdown);
        mark.disabled = missing;
      }
      // Re-aiming is the MISSING card's verb and the only one it has, so it is
      // not shown at all until the card is in that state.
      const point = menu.querySelector<HTMLButtonElement>(".note-menu-point");
      if (point) {
        point.hidden = !missing;
        point.disabled = !record;
      }
    });
  }

  // The index (ADR-0064 §10), rebuilt on every open from the live document.
  function toggleIndex(el: NoteCard) {
    openDrop(el, ".note-anchors", ".note-index", (pop) => {
      syncFromEditor(el);
      const found = anchorsOf(el._noteMarkdown);
      pop.textContent = "";
      if (!found.length) return false;
      for (const a of found) {
        const row = document.createElement("button");
        row.className = "note-menu-item";
        row.type = "button";
        row.textContent = a.text;
        row.title = a.text;
        row.addEventListener("click", () => {
          closeMenu();
          scrollToAnchor(el, a.index);
        });
        pop.append(row);
      }
      return true;
    });
  }

  // Show the index button only when there is an index. A card whose note has
  // no `##` would otherwise offer a control that opens onto nothing.
  function paintIndex(el: NoteCard) {
    const btn = el.querySelector<HTMLElement>(".note-index");
    if (!btn) return;
    btn.hidden = anchorsOf(el._noteMarkdown).length === 0;
  }

  // Scroll this card's body to its `index`-th `##`. By ORDINAL and not by
  // text, for `jump`'s reason: a note may hold two sections with one name.
  //
  // The BODY is scrolled, and nothing else. `heading.scrollIntoView()` walks
  // every scrollable ancestor and moves each one — so picking a section also
  // panned the plane under the card, which is the operator's report: the
  // canvas moved when only the note should have. The offset is computed
  // against the body's own box, which is the one box that must move.
  function scrollToAnchor(el: NoteCard, index: number) {
    const body = el.querySelector<HTMLElement>(".note-body");
    const heading = body?.querySelectorAll?.("h2")?.[index];
    if (!body || !heading) return;
    const top =
      heading.getBoundingClientRect().top - body.getBoundingClientRect().top + body.scrollTop;
    body.scrollTo({ top: Math.max(0, top), behavior: reducedMotion() ? "auto" : "smooth" });
  }

  function closeMenu() {
    if (!openMenu) return;
    openMenu.hidden = true;
    openMenu = null;
    openMenuTrigger = "";
    document.removeEventListener("pointerdown", closeOnOutside, true);
  }
  function closeOnOutside(ev: PointerEvent) {
    if (openMenu && !openMenu.contains(ev.target as Node | null) && !(ev.target as Element | null)?.closest?.(openMenuTrigger)) {
      closeMenu();
    }
  }

  // ---- the veil (ADR-0064 §8 as amended) ----------------------------------------

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

  // ---- the cheat sheet ----------------------------------------------------------

  // WHAT TO TYPE. The editor is hybrid WYSIWYG (ADR-0064 §6) — the markup
  // dissolves the moment it is recognised — which is exactly why the marks
  // themselves are not discoverable: there is nothing left on screen to read
  // them off. The `/` menu answers the same question for BLOCKS and nothing
  // answered it for marks.
  //
  // The flavour is CommonMark + GFM, because that is what the editor's two
  // presets parse; every row below was typed into the real bundle and its
  // markdown read back (2026-09-22), so nothing here is a guess at upstream's
  // rules. Three of them are this bundle's own additions, because upstream had
  // no rule at all: `[ ] ` on a plain line, `[text](url)` (commonmark ships an
  // input rule for an IMAGE and none for a link), and the `@@path` shorthand.
  const MARKDOWN_HELP = [
    ["**bold**", "Bold"],
    ["*italic*", "Italic"],
    ["`code`", "Inline code"],
    ["~~struck~~", "Strikethrough"],
    ["[text](url)", "A link"],
    ["@@path/to/file", "A link to that file"],
    ["#", "Large heading"],
    ["##", "Heading. It is also listed in the heading index."],
    ["-", "A bullet"],
    ["1.", "A numbered item"],
    ["[ ]", "A task. [x] marks it as done."],
    [">", "A quote"],
    ["|3x3|", "A 3×3 table"],
    ["---", "A horizontal line"],
    ["```", "A code block"],
    ["```mermaid", "A diagram. Click it to edit."],
    ["/", "The block menu"],
    ["Shift+Enter", "A line break inside the block"],
  ];
  // The trailing SPACE is what fires a block mark, and it cannot be seen in a
  // chip — so the table drops it and this line carries it instead.
  const MARKDOWN_HELP_NOTE = "Type a space after a mark to apply it.";

  // Borrowed from `wb-messages.ts`'s `askConfirm`: the shell's modal CLASSES
  // over DOM this module builds itself, so the card keeps working in the
  // detached-fence popup, which has no Alpine and no modal markup.
  function markdownHelp() {
    const scrim = document.createElement("div");
    scrim.className = "modal-scrim wb-note-help";
    const modal = document.createElement("div");
    modal.className = "modal note-help-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "Markdown in a note");
    const head = document.createElement("div");
    head.className = "modal-head";
    head.innerHTML = '<i class="bi bi-markdown"></i>';
    const heading = document.createElement("span");
    heading.className = "modal-title";
    heading.textContent = "Markdown in a note";
    const sub = document.createElement("span");
    sub.className = "modal-sub";
    sub.textContent = "CommonMark + GFM";
    head.append(heading, sub);
    const table = document.createElement("table");
    table.className = "note-help-table";
    for (const [type, gets] of MARKDOWN_HELP) {
      const tr = document.createElement("tr");
      const td1 = document.createElement("td");
      const code = document.createElement("code");
      code.textContent = type;
      td1.append(code);
      const td2 = document.createElement("td");
      td2.textContent = gets;
      tr.append(td1, td2);
      table.append(tr);
    }
    const note = document.createElement("p");
    note.className = "note-help-note";
    note.textContent = MARKDOWN_HELP_NOTE;
    const foot = document.createElement("div");
    foot.className = "modal-foot";
    const ok = document.createElement("button");
    ok.className = "btn accent";
    ok.type = "button";
    ok.textContent = "Close";
    foot.append(ok);
    modal.append(head, table, note, foot);
    scrim.append(modal);
    document.body.append(scrim);
    ok.focus();
    const done = () => {
      document.removeEventListener("keydown", onKey, true);
      scrim.remove();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape" || (ev.key === "Enter" && document.activeElement === ok)) {
        ev.stopPropagation();
        done();
      }
    };
    // CAPTURE, for `askConfirm`'s reason: the plane's accelerators and a
    // console's terminal both take Escape before a bubbling listener would.
    document.addEventListener("keydown", onKey, true);
    ok.addEventListener("click", done);
    return scrim;
  }

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
    if (pointing) {
      // A whole rel path, not a file name: the card is being aimed somewhere
      // else in the checkout, and the read decides whether there is a note
      // there. Identity is `(repo, checkout, path)` (ADR-0064 §4) and this is
      // the one gesture that could break it: two cards over one file would be
      // two editors autosaving it, each overwriting the other with stale text
      // and neither told.
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
      return;
    }
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

  // ---- the map (ADR-0064 §10) ---------------------------------------------------

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
        onTop: record.id === onTopId,
        // The row's `Keep on top` is refused for a card in the popup.
        away: isAway(record, fences),
      };
    });
  }

  // Jump to a card: the plane moves to it and it takes the focus.
  function jump(id: string) {
    return consoleHost?.jumpToNote?.(id) ?? null;
  }

  function reducedMotion() {
    try {
      return !!window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches;
    } catch {
      return false;
    }
  }

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
      flush(el);
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
    // folds
    titleOf,
    anchorsOf,
    savedLabel,
    noteSlug,
    stampName,
    toneOf,
    bodyOf,
    colorOf,
    styleOf,
    withStyle,
    withTitle,
    titleFieldOf,
    veiledOf,
    withVeil,
    fillOf,
    inkOf,
    fontOf,
    sizeOf,
    lockedBy,
    spawnRect,
    noteDormancyDecision,
    onTopRect,
    onTopClamp,
    ON_TOP_FLOOR,
    ON_TOP_BAND_BELOW,
    ON_TOP_TOP,
    TONES,
    FILLS,
    INKS,
    FONTS,
    SIZES,
    DEFAULT_TONE,
    DEFAULT_FILL,
    DEFAULT_INK,
    DEFAULT_FONT,
    DEFAULT_SIZE,
    DEFAULT_DIR,
    NEW_NOTE_STYLE,
    MARKDOWN_HELP,
    NOTE_MIN,
    NOTE_DEFAULT,
    SAVE_AFTER_MS,
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
