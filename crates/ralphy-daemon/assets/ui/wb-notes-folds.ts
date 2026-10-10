/* ---------------------------------------------------------------------------
   The note card's folds and front matter (ADR-0064 §§4, 7, 8, 14; ADR-0073 D8).

   The constants of a note (its tones, hands, sizes and the default directory),
   the functions that read and write its front-matter block, and the pure rules
   a card is laid out and slept by. Nothing here reads the DOM, the clock, a
   timer or `window`, except where a card is painted (`applyLook`, `noCredential`)
   or the fence geometry is asked (`lockedBy`, `spawnRect`).

   `wb-notes.ts` imports these and keeps every name in its returned object.
   --------------------------------------------------------------------------- */
import { WBGeometry } from "./wb-geometry.ts";
import type { DeskFence, NoteCard, NoteLook, NoteSource, Offset, Rect, Size } from "./wb-types.d.ts";

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

// The body's leading `#` heading, when that is what the body opens with —
// `{name, rest}`, or `null`. Deliberately NOT "the first `#` anywhere", the
// old rule: with the title out of the document, a `#` further down is a
// section the operator wrote and absorbing it into the header would delete
// text nobody asked to move.
export function legacyTitleOf(body: string | null | undefined) {
  const lines = String(body || "").split("\n");
  let at = 0;
  while (at < lines.length && lines[at].trim() === "") at += 1;
  const m = /^#\s+(.+?)\s*$/.exec(lines[at] ?? "");
  if (!m) return null;
  // The blank line that separated the heading from the text goes with it.
  let after = at + 1;
  if ((lines[after] ?? "").trim() === "") after += 1;
  return { name: m[1], rest: lines.slice(after).join("\n") };
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
    const m = /^##\s+(.+?)\s*$/.exec(line);
    if (m) out.push({ text: m[1], index: out.length });
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
  el.dataset.tone = el._noteTone;
  el.dataset.fill = el._noteFill;
  el.dataset.ink = el._noteInk;
  el.dataset.font = el._noteFont;
  el.dataset.size = el._noteSize;
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

// ---- front matter -----------------------------------------------------------
// Byte-identical to `note::with_color`/`color_of`/`body_of` in the daemon:
// the two sides write the same header for the same note, so an autosave after
// a read never rewrites the file for no reason. One field, a closed set — a
// YAML parser here would be a dependency for `color: sage`.

export function splitFrontMatter(markdown: string | null | undefined) {
  const text = String(markdown || "");
  const open = text.startsWith("---\n") ? 4 : text.startsWith("---\r\n") ? 5 : 0;
  if (!open) return null;
  const rest = text.slice(open);
  let offset = 0;
  for (const line of rest.split("\n")) {
    if (line.replace(/\r$/, "") === "---") {
      return { inner: rest.slice(0, offset), body: rest.slice(offset + line.length + 1) };
    }
    offset += line.length + 1;
  }
  return null;
}

// The document without its front matter — what the editor shows.
export function bodyOf(markdown: string | null | undefined) {
  const split = splitFrontMatter(markdown);
  return split ? split.body : String(markdown || "");
}

// One field out of the block, by name, or `null` when it is absent or names
// something outside its closed set.
export function fieldOf(markdown: string | null | undefined, key: string, set: string[]) {
  const split = splitFrontMatter(markdown);
  if (!split) return null;
  const re = new RegExp("^\\s*" + key + ":\\s*(\\S+)\\s*$");
  for (const line of split.inner.split("\n")) {
    const m = re.exec(line.replace(/\r$/, ""));
    if (m) return set.includes(m[1]) ? m[1] : null;
  }
  return null;
}

// The colour the document names, or `null`.
export function colorOf(markdown: string | null | undefined) {
  return fieldOf(markdown, "color", TONES);
}

// Does this note OPEN VEILED (ADR-0064 §8 as amended)? `hidden: true` is the
// note saying it is not for whoever happens to be looking at the screen.
// Deliberately NOT part of `styleOf`: the look is how the card is painted,
// and this decides whether there is anything painted at all.
export function veiledOf(markdown: string | null | undefined) {
  return fieldOf(markdown, "hidden", ["true", "false"]) === "true";
}

// The `title:` field — free text, so it cannot go through `fieldOf`'s closed
// set. `null` when the block has none, which is what tells `titleOf` to look
// for the legacy heading. Written as a double-quoted YAML scalar and read as
// one, because a title says "Sprint 12: what is left" often enough that a
// bare value would be invalid YAML to anyone else's parser.
export function titleFieldOf(markdown: string | null | undefined) {
  const split = splitFrontMatter(markdown);
  if (!split) return null;
  for (const line of split.inner.split("\n")) {
    const m = /^\s*title:\s*(.*?)\s*$/.exec(line.replace(/\r$/, ""));
    if (!m) continue;
    const raw = m[1];
    if (!raw.startsWith('"')) return raw;
    return raw
      .slice(1, raw.endsWith('"') && raw.length > 1 ? -1 : undefined)
      .replace(/\\(["\\])/g, "$1");
  }
  return null;
}
export function titleYaml(name: string) {
  return '"' + String(name).replace(/[\\"]/g, "\\$&") + '"';
}

// The whole look of the card, defaulted: the ground's tone, how much of it
// the ground takes, and the ink over it (ADR-0064 §8, amended 2026-09-22).
export function styleOf(markdown: string | null | undefined) {
  return {
    tone: toneOf(colorOf(markdown) || DEFAULT_TONE),
    fill: fillOf(fieldOf(markdown, "fill", FILLS) || DEFAULT_FILL),
    ink: inkOf(fieldOf(markdown, "ink", INKS) || DEFAULT_INK),
    font: fontOf(fieldOf(markdown, "font", FONTS) || DEFAULT_FONT),
    size: sizeOf(fieldOf(markdown, "size", SIZES) || DEFAULT_SIZE),
  };
}

// `body` under the one front-matter block this shell writes. ONE shape, so
// an autosave never rewrites the header for no reason — and the three
// defaults are OMITTED, so a note nobody restyled and nobody named carries
// the same single `color:` line it always did. `title` leads because it is
// the document's name; `fill`/`ink` trail because they are refinements of
// the colour above them.
export function withHeader(body: string, title: string | null | undefined, style: Partial<NoteLook> | null | undefined, veiled?: boolean) {
  const tone = toneOf(style?.tone);
  const fill = fillOf(style?.fill);
  const ink = inkOf(style?.ink);
  const font = fontOf(style?.font);
  const size = sizeOf(style?.size);
  const name = String(title || "").replace(/[\r\n]+/g, " ").trim();
  const lines: string[] = [];
  if (name) lines.push(`title: ${titleYaml(name)}`);
  lines.push(`color: ${tone}`);
  if (fill !== DEFAULT_FILL) lines.push(`fill: ${fill}`);
  if (ink !== DEFAULT_INK) lines.push(`ink: ${ink}`);
  if (font !== DEFAULT_FONT) lines.push(`font: ${font}`);
  if (size !== DEFAULT_SIZE) lines.push(`size: ${size}`);
  // Last, and only when true: `hidden: false` is what every note in the
  // world already is, and writing it would put a line in every file to say
  // nothing.
  if (veiled) lines.push("hidden: true");
  return `---\n${lines.join("\n")}\n---\n${body}`;
}

// Restyling keeps the name, whatever the caller happens to hold: the two
// fields live in one block and a writer that knows about only one of them
// would drop the other every time it ran.
export function withStyle(markdown: string | null | undefined, style: Partial<NoteLook>) {
  // The FIELD, never `titleOf`: promoting a legacy heading here would half
  // migrate a note — the name in the header and the heading still in the
  // body — on a gesture that was about colour. `withTitle` is the one
  // migration door.
  return withHeader(bodyOf(markdown), titleFieldOf(markdown) ?? "", style, veiledOf(markdown));
}

// The veil, as a WRITE — the ONE door that changes it, which is what lets
// every other writer carry it without knowing what it is for.
export function withVeil(markdown: string | null | undefined, veiled: boolean) {
  return withHeader(bodyOf(markdown), titleFieldOf(markdown) ?? "", styleOf(markdown), !!veiled);
}

// The visible name, as a WRITE (ADR-0064 §4 as amended 2026-09-22: the title
// is a front-matter field, so retitling never touches the body and never
// renames the file — the latter is what `⋯ → Rename file…` is for).
//
// This is also the ONE place a legacy note migrates: the heading the old
// rule called the title is lifted out of the body and into the header, so
// the name stops being shown twice. It happens on a retitle and nowhere else
// — opening a note rewrites nothing.
export function withTitle(markdown: string | null | undefined, title: string | null | undefined) {
  const name = String(title || "").replace(/[\r\n]+/g, " ").trim();
  const body = bodyOf(markdown);
  const legacy = legacyTitleOf(body);
  return withHeader(legacy ? legacy.rest : body, name, styleOf(markdown), veiledOf(markdown));
}

// Is this card read-only? Its own `locked`, or the lock of the fence that
// holds it — the same derivation a console's lock uses (ADR-0051 §6), which
// is why the fence's lock is NOT copied onto the record.
export function lockedBy(record: Pick<NoteSource, "locked" | "rect"> | null | undefined, fences: DeskFence[] | null | undefined) {
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
export function spawnRect(viewport: Partial<Size> | null | undefined, offset: Partial<Offset> | null | undefined, taken: number | null | undefined, fences: DeskFence[] | null | undefined) {
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
export function noteDormancyDecision({ visible, onTop, dirty, inFlight, asleep, elapsed, after }: DormancyFacts) {
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
export function onTopRect(rect: Partial<Size> | null | undefined, viewport: Partial<Size> | null | undefined): FloatBox {
  const vw = viewport?.width || 0;
  const vh = viewport?.height || 0;
  if (vw < ON_TOP_BAND_BELOW) return { band: true };
  const width = Math.round(Math.min(Math.max(rect?.width || 0, ON_TOP_FLOOR.width), vw * 0.5));
  const height = Math.round(
    Math.min(Math.max(rect?.height || 0, ON_TOP_FLOOR.height), vh * 0.8, vh - ON_TOP_TOP - ON_TOP_MARGIN),
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
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(String(href || ""));
  // No scheme means a `/`-rooted path, which the browser resolves on this
  // origin: an ordinary navigation, not a foreign one.
  if (!scheme) return true;
  return SAFE_SCHEMES.includes(scheme[1].toLowerCase() + ":");
}

// The detached-fence popup loads no `wb-file-viewer.ts`, so without this every
// link in a note there would fall into the `!target` branch and silently do
// nothing. A repo-relative link still cannot be opened from a popup that has
// no explorer — that one stays inert, and says so by doing nothing.
export function popupLinkTarget(href: string | null): { kind: "fragment"; fragment: string } | { kind: "external" } | null {
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
