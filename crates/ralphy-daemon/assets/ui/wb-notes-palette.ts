// The note card's palette: the swatches that change a note's colors, font and
// size, written into the file (ADR-0064 §8, amended 2026-09-22). `createNotes`
// builds it with the card's names as typed deps; the text of each function is
// the one it had in `wb-notes.ts`.
import {
  FILLS,
  FONTS,
  INKS,
  SIZES,
  SWATCH_NAME,
  TONES,
  applyLook,
  lookOf,
  withStyle,
} from "./wb-notes-folds.ts";
import type { NoteCard, NoteLook } from "./wb-types.d.ts";

/** What `createNotes` hands the palette: the card's and the editor's names. */
export type NotePaletteDeps = {
  /** The page's document. */
  document: Document;
  /** Is this card showing nothing right now? */
  veiledNow: (el: NoteCard) => boolean;
  /** Close the open drop-down menu, if any. */
  closeMenu: () => void;
  /** Pull the editor's text into the card's document. */
  syncFromEditor: (el: NoteCard) => void;
  /** Mark the card's document as changed. */
  markDirty: (el: NoteCard) => void;
};

export function createNotePalette(deps: NotePaletteDeps) {
  const { document, veiledNow, closeMenu, syncFromEditor, markDirty } = deps;

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
    if (
      openPalette &&
      !openPalette.contains(ev.target as Node | null) &&
      !(ev.target as Element | null)?.closest?.(".note-tone")
    ) {
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
    const row = (
      label: string,
      cls: string,
      names: string[],
      pick: (el: NoteCard, name: string) => void,
      text?: (name: string) => string,
    ) => {
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

  return { togglePalette, closePalette, buildPalette };
}
