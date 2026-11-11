/* ---------------------------------------------------------------------------
   The note card's look: the closed sets of tones, fills, inks, hands and
   sizes, the folds that default a name out of them, and the paint of a card
   (ADR-0064 §§3, 8). `wb-notes-folds.ts` re-exports every name.
   --------------------------------------------------------------------------- */
import type { NoteCard, NoteLook } from "./wb-types.d.ts";

// The tones the file's front matter may name (ADR-0064 §3). The stylesheet
// owns the actual colours; this list is the closed set. The daemon reads no
// field of the front matter — its codec carries bytes — so the shell is the
// one side that has to agree with itself.
export const TONES = ["ochre", "sage", "rose", "slate", "plum", "sand"];
// The empty line's title control (ADR-0064, 2026-09-27 amendment). The
// words live here, not in the vendored bundle, so `xtask ui-copy` reads them.
export const ADD_TITLE = "Add title";
export const REMOVE_TITLE = "Remove title";
export const EMPTY_TITLE_HINT = "Write a title";
export const DEFAULT_TONE = "sand";
// How much of the tone the card's GROUND takes. `wash` is ADR-0064 §8's
// quiet tint; `solid` is the tone itself, which is what makes "a yellow note
// with white text" possible at all — a 10 % tint over a dark ground is not a
// colour anyone would call yellow. The stylesheet owns both mixes.
export const FILLS = ["wash", "solid"];
export const DEFAULT_FILL = "wash";
// The ink, chosen BESIDE the ground so the pair is the operator's: the
// theme's own text, a near-white and a near-black for the solid fills, and
// the six tones again for a coloured hand. Unknown names fall back like a
// tone's.
export const INKS = ["default", "light", "dark"].concat(TONES);
export const DEFAULT_INK = "default";
// The hand the note is written in (asked 2026-09-22). A CLOSED SET like
// every other field of the look, and for a reason this one makes sharper: a
// free `font-family` string would put a font the writer happens to have
// installed into a file someone else opens, and the card would paint as a
// fallback nobody chose. The names are ROLES; 01-base.css owns which stack
// each is on this machine (13-notes.css).
export const FONTS = ["sans", "serif", "mono"];
export const DEFAULT_FONT = "sans";
// The reading size, as a STEP and not a pixel count. `--reading-size` is the
// plane's own (ADR-0035) and each step is a ratio of it, so a note keeps its
// relation to the chrome around it instead of pinning a number that stops
// agreeing with the rest of the workbench the moment that scale moves.
export const SIZES = ["xs", "s", "m", "l", "xl"];
export const DEFAULT_SIZE = "m";
// What the palette calls each name of the sets above. The file keeps the
// key; only the tooltip and the accessible name read this.
export const SWATCH_NAME: Record<string, string> = {
  ochre: "Ochre",
  sage: "Sage",
  rose: "Rose",
  slate: "Slate",
  plum: "Plum",
  sand: "Sand",
  wash: "Tint",
  solid: "Solid",
  default: "Theme",
  light: "Light",
  dark: "Dark",
  sans: "Sans serif",
  serif: "Serif",
  mono: "Monospace",
  xs: "Extra small",
  s: "Small",
  m: "Medium",
  l: "Large",
  xl: "Extra large",
};
// What a NEW note is dressed in, which is NOT the same question as what an
// absent field means. The `DEFAULT_*` above are the READING fallback — a
// file that names no `fill:` is a wash — and changing them to restyle new
// notes would restyle every note already written, because the field they
// omit is the one being redefined. So the operator's choice (2026-09-22)
// lives here and is written out explicitly by `withHeader`.
export const NEW_NOTE_STYLE = { tone: "ochre", fill: "solid", ink: "dark" };

// The tone a record/document names, defaulted. Never throws on a tone from a
// hand-edited file: an unknown name is sand.
export function toneOf(name: string | null | undefined): string {
  return TONES.includes(name as string) ? (name as string) : DEFAULT_TONE;
}
export function fillOf(name: string | null | undefined): string {
  return FILLS.includes(name as string) ? (name as string) : DEFAULT_FILL;
}
export function inkOf(name: string | null | undefined): string {
  return INKS.includes(name as string) ? (name as string) : DEFAULT_INK;
}
export function fontOf(name: string | null | undefined): string {
  return FONTS.includes(name as string) ? (name as string) : DEFAULT_FONT;
}
export function sizeOf(name: string | null | undefined): string {
  return SIZES.includes(name as string) ? (name as string) : DEFAULT_SIZE;
}

// The look a card is WEARING, in the two places it has to be: the fields the
// writers read, and the `data-*` the stylesheet selects on. One function
// because the look grew from three fields to five — every site that set them
// by hand was a place the next field would be forgotten, and a card wearing
// four of five is a card whose file and paint disagree.
export function applyLook(el: NoteCard, style: Partial<NoteLook> | null | undefined) {
  el._noteTone = toneOf(style?.tone);
  el._noteFill = fillOf(style?.fill);
  el._noteInk = inkOf(style?.ink);
  el._noteFont = fontOf(style?.font);
  el._noteSize = sizeOf(style?.size);
  el.dataset["tone"] = el._noteTone;
  el.dataset["fill"] = el._noteFill;
  el.dataset["ink"] = el._noteInk;
  el.dataset["font"] = el._noteFont;
  el.dataset["size"] = el._noteSize;
}

// What that card is wearing, as the shape every writer takes.
export function lookOf(el: NoteCard): NoteLook {
  return {
    tone: el._noteTone,
    fill: el._noteFill,
    ink: el._noteInk,
    font: el._noteFont,
    size: el._noteSize,
  };
}

// A field on a card is NOT a credential. Measured (2026-09-22): renaming a
// note raised the browser's "save your password?" prompt with the note's
// title offered as the username — the password manager had classified the
// title box as the login form's user field, because every input the page
// leaves unowned by a `<form>` is grouped into one synthetic form with the
// login password beside it. The attributes below are the documented way out,
// and the two `data-*` ones say the same thing to 1Password and LastPass,
// which read their own.
export function noCredential(input: HTMLInputElement) {
  input.type = "text";
  input.autocomplete = "off";
  input.setAttribute("autocorrect", "off");
  input.setAttribute("autocapitalize", "off");
  input.setAttribute("data-1p-ignore", "");
  input.setAttribute("data-lpignore", "true");
  input.setAttribute("data-form-type", "other");
}
