// The note card's assembly: the chrome of one card put together, the lock glyph,
// and the keys the card answers to (ADR-0064 §§8, 13). `createNotes` builds it
// with the card's closure names as typed deps; the text of each function is the
// one it had in `wb-notes.ts`.
import {
  DEFAULT_FILL,
  DEFAULT_INK,
  DEFAULT_TONE,
  NEW_NOTE_STYLE,
  applyLook,
} from "./wb-notes-folds.ts";
import type { createNoteChrome } from "./wb-notes-chrome.ts";
import type { createNoteOnTop } from "./wb-notes-on-top.ts";
import type { createNotePalette } from "./wb-notes-palette.ts";
import type { Stack } from "./wb-stage-stack.ts";
import type { CardHost, NoteCard, NoteSource } from "./wb-types.d.ts";

/** What `createNotes` hands the assembly: the card's closure names. */
export type NoteBuildDeps = {
  /** The page's document. */
  document: Document;
  /** The page's consoles; null where a test has none. */
  consoleHost: CardHost | null;
  /** The document's z stack; null where a test has none. */
  stack: Stack | null;
  /** The consoles stage. */
  stage: () => HTMLElement | null;
  /** The parts of a card's chrome. */
  buildCardParts: ReturnType<typeof createNoteChrome>["buildCardParts"];
  /** The card's palette. */
  buildPalette: ReturnType<typeof createNotePalette>["buildPalette"];
  /** Wire the click rule of the links inside the card. */
  linkCard: (el: NoteCard) => void;
  /** Persist the placement of one or more cards. */
  persistCards: (els: NoteCard | NoteCard[]) => void;
  /** Write what the card holds now. */
  flush: (el: NoteCard) => Promise<unknown>;
  /** True while the card floats on top. */
  onTop: ReturnType<typeof createNoteOnTop>["onTop"];
  /** The pointer gesture of a card on top. */
  floatGesture: ReturnType<typeof createNoteOnTop>["floatGesture"];
};

export function createNoteBuild(deps: NoteBuildDeps) {
  const {
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
  } = deps;

  // Paint the lock. A locked card cannot be MOVED or RESIZED, and that is the
  // whole of it (the operator's words, 2026-09-22: "o cadeado no notes não é
  // impedir de editar é impedir de mover"). The note is still written, renamed,
  // recoloured and hidden — a pin through the card, not a read-only file. The
  // JS guards in `makeDraggable`/`startResize` are the truth for the gestures;
  // this is the glyph.
  function applyLock(el: NoteCard, locked: boolean) {
    el._noteLocked = locked;
    el.classList.toggle("locked", locked);
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
  function assembleCard(el: NoteCard, record: NoteSource) {
    el.className = "note-card";
    el.dataset.noteId = record.id;
    // A card with NO FILE YET is a new note and is born in the operator's
    // colours; one with a path is dressed from what it is about to read, and
    // starting it anywhere but the reading default would flash a colour the
    // file does not hold for the tick the read takes.
    const born =
      (record.path ?? "") === ""
        ? NEW_NOTE_STYLE
        : { tone: DEFAULT_TONE, fill: DEFAULT_FILL, ink: DEFAULT_INK };
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
      locked: () => el._noteLocked === true || onTop(el),
      onDrop: () => {
        persistCards(el);
      },
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
      const dom = el._noteEditor?.dom();
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
      const inEditor = el
        .querySelector<HTMLElement>(".note-body")
        ?.contains(document.activeElement);
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- focus is on an HTML element
      (document.activeElement as HTMLElement | null)?.blur();
      if (inEditor !== true) el.classList.remove("focused");
    });
  }

  return { assembleCard, applyLock };
}
