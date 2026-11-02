/* ---------------------------------------------------------------------------
   The note card's folds (ADR-0064 §§4, 7, 8, 14; ADR-0073 D8).

   The pure rules a card is laid out, named and slept by. The look is in
   `wb-notes-look.ts` and the front matter in `wb-notes-header.ts`; this module
   re-exports both, so an importer names one module. Nothing here reads the
   DOM, the clock, a timer or `window`, except where the fence geometry is
   asked (`lockedBy`, `spawnRect`).

   `wb-notes.ts` keeps every name of `NOTE_FOLDS` in its returned object.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import {
  DEFAULT_FILL,
  DEFAULT_FONT,
  DEFAULT_INK,
  DEFAULT_SIZE,
  DEFAULT_TONE,
  FILLS,
  FONTS,
  INKS,
  NEW_NOTE_STYLE,
  SIZES,
  TONES,
  fillOf,
  fontOf,
  inkOf,
  sizeOf,
  toneOf,
} from "./wb-notes-look.ts";
import {
  bodyOf,
  colorOf,
  legacyTitleOf,
  styleOf,
  titleFieldOf,
  veiledOf,
  withStyle,
  withTitle,
  withVeil,
} from "./wb-notes-header.ts";
import type { DeskFence, NoteSource, Offset, Rect, Size } from "./wb-types.d.ts";

export {
  ADD_TITLE,
  DEFAULT_FILL,
  DEFAULT_FONT,
  DEFAULT_INK,
  DEFAULT_SIZE,
  DEFAULT_TONE,
  EMPTY_TITLE_HINT,
  FILLS,
  FONTS,
  INKS,
  NEW_NOTE_STYLE,
  REMOVE_TITLE,
  SIZES,
  SWATCH_NAME,
  TONES,
  applyLook,
  fillOf,
  fontOf,
  inkOf,
  lookOf,
  noCredential,
  sizeOf,
  toneOf,
} from "./wb-notes-look.ts";
export {
  bodyOf,
  colorOf,
  fieldOf,
  legacyTitleOf,
  splitFrontMatter,
  styleOf,
  titleFieldOf,
  titleYaml,
  veiledOf,
  withHeader,
  withStyle,
  withTitle,
  withVeil,
} from "./wb-notes-header.ts";

/** What `noteDormancyDecision` reads: the card's facts and the clock. */
export type DormancyFacts = {
  visible: boolean;
  onTop: boolean;
  dirty: boolean;
  inFlight: boolean;
  asleep: boolean;
  elapsed: number;
  after: number;
};

/** Where a card on top floats: a band across the top, or a box in viewport pixels. */
export type FloatBox = { band: true } | ({ band?: false } & Rect);

// The card's floor. Below a console's minimum on purpose: a note is often a
// three-line reminder, and forcing it to a console's footprint would make
// the stage unreadable.
export const NOTE_MIN = { width: 160, height: 100 };
// What a new card measures — a post-it, not a document window, but wide
// enough for the editor's slash menu, which mounts INSIDE the editor and is
// clipped by a smaller card (measured; see vendor-build/crepe/entry.js).
export const NOTE_DEFAULT = { width: 320, height: 260 };
// The default landing directory (ADR-0064 §4), mirrored from `note::DIR`.
export const DEFAULT_DIR = ".ralphy/notes";
// Autosave: quiet for this long and the note is written (ADR-0064 §7). Short
// enough that a closed tab loses a sentence at most, long enough that typing
// a paragraph is one write and not forty.
export const SAVE_AFTER_MS = 800;

// ---- pure folds (tested without a document) ---------------------------------

// A note's title is the front matter's `title:` (ADR-0064 §4, amended
// 2026-09-22: the name lives in the header, not in the body). The LEGACY
// shape is still read — a `#` heading on the body's first line, which is
// where every note written before the amendment carries its name — so an
// existing file opens under the name it has always had. An untitled note is
// called what the card calls it.
export function titleOf(markdown: string | null | undefined, fallback?: string) {
  const named = titleFieldOf(markdown);
  if (named !== null) return named;
  const legacy = legacyTitleOf(bodyOf(markdown));
  if (legacy) return legacy.name;
  return fallback === undefined ? "Untitled note" : fallback;
}

// When this note was last written, for the footer (ADR-0064 §7 as amended).
// The TIME on the day it happened and the date as well on any other: a note
// saved four minutes ago and one saved last Tuesday must not read alike, and
// "19:42" alone says the wrong thing about the second. Local, because the
// operator saved it where they are. `null` for a note no save has landed
// for yet — a card is not going to claim a time it does not have.
export function savedLabel(at: number | null | undefined, now?: number) {
  if (!at) return "";
  const then = new Date(at);
  const today = new Date(now ?? Date.now());
  const hh = String(then.getHours()).padStart(2, "0");
  const mm = String(then.getMinutes()).padStart(2, "0");
  const sameDay =
    then.getFullYear() === today.getFullYear() &&
    then.getMonth() === today.getMonth() &&
    then.getDate() === today.getDate();
  if (sameDay) return `Saved ${hh}:${mm}`;
  const dd = String(then.getDate()).padStart(2, "0");
  const mo = String(then.getMonth() + 1).padStart(2, "0");
  return `Saved ${dd}/${mo} ${hh}:${mm}`;
}

// The `##` headings, in document order — the jump anchors (ADR-0064 §10).
// `#` is the title and `###` and below are structure inside a section; only
// the second level is an anchor.
export function anchorsOf(markdown: string | null | undefined) {
  const out: { text: string; index: number }[] = [];
  const lines = String(markdown || "").split("\n");
  let fenced = false;
  for (const line of lines) {
    // A `##` inside a code fence is source, not a heading.
    if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
    if (fenced) continue;
    const text = /^##\s+(.+?)\s*$/.exec(line)?.[1];
    if (text !== undefined) out.push({ text, index: out.length });
  }
  return out;
}

// The filename a first save derives from the title (ADR-0064 §4). Lowercase,
// ASCII-ish, hyphen-joined, capped — and `""` when the title yields nothing,
// which is the caller's signal to fall back to a stamp.
export function noteSlug(title: string | null | undefined) {
  return String(title || "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48)
    .replace(/-+$/g, "");
}

// `note-<UTC yyyymmdd-hhmmss>` — the name a note with no heading yet takes.
export function stampName(now?: Date | number) {
  const d = now instanceof Date ? now : new Date(now ?? Date.now());
  const p = (n: number) => String(n).padStart(2, "0");
  return (
    "note-" +
    d.getUTCFullYear() +
    p(d.getUTCMonth() + 1) +
    p(d.getUTCDate()) +
    "-" +
    p(d.getUTCHours()) +
    p(d.getUTCMinutes()) +
    p(d.getUTCSeconds())
  );
}

// Is this card read-only? Its own `locked`, or the lock of the fence that
// holds it — the same derivation a console's lock uses (ADR-0051 §6), which
// is why the fence's lock is NOT copied onto the record.
export function lockedBy(
  record: Pick<NoteSource, "locked" | "rect"> | null | undefined,
  fences: DeskFence[] | null | undefined,
) {
  if (record?.locked) return "self";
  const held = WBGeometry?.fenceOf?.(fences || [], record?.rect);
  return held?.locked ? "fence" : null;
}

// Where a new card lands: the centre of what the operator is looking at,
// nudged by how many cards already sit there so two `New note`s in a row do
// not stack perfectly.
// `fences` is optional: given, the cascade steps PAST a slot whose centre
// lands inside one, because a note is born outside any fence (ADR-0064 §11)
// — a card that opened inside a locked fence would be read-only the moment
// it appeared. A plane where every slot is fenced falls back to the first:
// somewhere visible beats nowhere.
export const SPAWN_SLOTS = 6;
export function spawnRect(
  viewport: Partial<Size> | null | undefined,
  offset: Partial<Offset> | null | undefined,
  taken: number | null | undefined,
  fences: DeskFence[] | null | undefined,
) {
  const at = (n: number): Rect => {
    const step = 24 * (n % SPAWN_SLOTS);
    return {
      left: Math.max(
        0,
        Math.round((offset?.left || 0) + (viewport?.width || 0) / 2 - NOTE_DEFAULT.width / 2) +
          step,
      ),
      top: Math.max(
        0,
        Math.round((offset?.top || 0) + (viewport?.height || 0) / 2 - NOTE_DEFAULT.height / 2) +
          step,
      ),
      width: NOTE_DEFAULT.width,
      height: NOTE_DEFAULT.height,
    };
  };
  const start = taken || 0;
  const first = at(start);
  if (!fences || !fences.length) return first;
  for (let i = 0; i < SPAWN_SLOTS; i++) {
    const rect = at(start + i);
    if (!WBGeometry?.fenceOf?.(fences, rect)) return rect;
  }
  return first;
}

// Sleep or wake, as a fold (ADR-0064 §14). A card off-screen for long enough
// gives its editor back — a Crepe instance is a ProseMirror view, and thirty
// of them idle on a plane nobody is looking at is the memory the consoles'
// own dormancy exists to save. The two refusals are the point: a card with
// unsaved text or a write in flight NEVER sleeps, because tearing the editor
// down is what would lose it.
//
// A card on top counts as visible. It is `position: fixed`, so it is not in
// the containing-block chain of the observer's root (`#workspace`), and the
// observer reports it as not intersecting while it floats in plain view
// (measured 2026-09-27: it fell asleep one sweep after the timeout).
export function noteDormancyDecision({
  visible,
  onTop,
  dirty,
  inFlight,
  asleep,
  elapsed,
  after,
}: DormancyFacts) {
  if (visible || onTop) return asleep ? "wake" : "stay";
  if (dirty || inFlight) return asleep ? "wake" : "stay";
  if (asleep) return "stay";
  return elapsed >= after ? "sleep" : "stay";
}

// A card on top (ADR-0064, 2026-09-26 amendment §§2, 8): where it floats, in
// VIEWPORT pixels. The rect's own size with a floor and a ceiling, in the
// top-right corner — the centre covers the prompt the operator is reading.
// The top clears a maximized console's title bar (32.6 px, measured in
// Chromium on 2026-09-26), whose restore and close buttons sit in that
// same corner.
// Below `ON_TOP_BAND_BELOW` the half-width ceiling is under the floor, so
// the card is a band across the top instead, and CSS owns its geometry.
export const ON_TOP_FLOOR = { width: 420, height: 320 };
export const ON_TOP_BAND_BELOW = 840;
export const ON_TOP_MARGIN = 12;
export const ON_TOP_TOP = 44;
export function onTopRect(
  rect: Partial<Size> | null | undefined,
  viewport: Partial<Size> | null | undefined,
): FloatBox {
  const vw = viewport?.width || 0;
  const vh = viewport?.height || 0;
  if (vw < ON_TOP_BAND_BELOW) return { band: true };
  const width = Math.round(Math.min(Math.max(rect?.width || 0, ON_TOP_FLOOR.width), vw * 0.5));
  const height = Math.round(
    Math.min(
      Math.max(rect?.height || 0, ON_TOP_FLOOR.height),
      vh * 0.8,
      vh - ON_TOP_TOP - ON_TOP_MARGIN,
    ),
  );
  return { band: false, left: vw - width - ON_TOP_MARGIN, top: ON_TOP_TOP, width, height };
}

// Keep a floating box inside the viewport after a drag or a window resize:
// the size shrinks to fit first, then the corner is pulled in.
export function onTopClamp(box: Rect, viewport: Partial<Size> | null | undefined): FloatBox {
  const vw = viewport?.width || 0;
  const vh = viewport?.height || 0;
  if (vw < ON_TOP_BAND_BELOW) return { band: true };
  const width = Math.min(box.width, vw);
  const height = Math.min(box.height, vh);
  return {
    band: false,
    left: Math.max(0, Math.min(box.left, vw - width)),
    top: Math.max(0, Math.min(box.top, vh - height)),
    width,
    height,
  };
}

// The three schemes a note may send the browser to. Everything else — and
// that includes `javascript:`, `data:` and `vbscript:` — is inert.
export const SAFE_SCHEMES = ["http:", "https:", "mailto:"];
export function isSafeScheme(href: string | null) {
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(String(href || ""))?.[1];
  // No scheme means a `/`-rooted path, which the browser resolves on this
  // origin: an ordinary navigation, not a foreign one.
  if (scheme === undefined) return true;
  return SAFE_SCHEMES.includes(scheme.toLowerCase() + ":");
}

// The detached-fence popup loads no `wb-file-viewer.ts`, so without this every
// link in a note there would fall into the `!target` branch and silently do
// nothing. A repo-relative link still cannot be opened from a popup that has
// no explorer — that one stays inert, and says so by doing nothing.
export function popupLinkTarget(
  href: string | null,
): { kind: "fragment"; fragment: string } | { kind: "external" } | null {
  if (!href) return null;
  if (href.startsWith("#")) return { kind: "fragment", fragment: href.slice(1) };
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("/")) return { kind: "external" };
  return null;
}

export function baseName(path: string) {
  return path.includes("/") ? path.slice(path.lastIndexOf("/") + 1) : path;
}
export function dirName(path: string) {
  return path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
}

// The folds and constants `WBNotes` hands out: browser checks, the markup
// and the torn-off pages read them there by name.
export const NOTE_FOLDS = {
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
  NOTE_MIN,
  NOTE_DEFAULT,
  SAVE_AFTER_MS,
};
