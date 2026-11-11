/* ---------------------------------------------------------------------------
   The note's front matter: the block a note opens with, and the folds that
   read and write it (ADR-0064 §§4, 8). `wb-notes-folds.ts` re-exports every
   name.
   --------------------------------------------------------------------------- */
import {
  DEFAULT_FILL,
  DEFAULT_FONT,
  DEFAULT_INK,
  DEFAULT_SIZE,
  DEFAULT_TONE,
  FILLS,
  FONTS,
  INKS,
  SIZES,
  TONES,
  fillOf,
  fontOf,
  inkOf,
  sizeOf,
  toneOf,
} from "./wb-notes-look.ts";
import type { NoteLook } from "./wb-types.d.ts";

// The body's leading `#` heading, when that is what the body opens with —
// `{name, rest}`, or `null`. Deliberately NOT "the first `#` anywhere", the
// old rule: with the title out of the document, a `#` further down is a
// section the operator wrote and absorbing it into the header would delete
// text nobody asked to move.
export function legacyTitleOf(body: string | null | undefined) {
  const lines = String(body || "").split("\n");
  let at = 0;
  while (at < lines.length && lines[at]?.trim() === "") at += 1;
  const name = /^#\s+(.+?)\s*$/.exec(lines[at] ?? "")?.[1];
  if (name === undefined) return null;
  // The blank line that separated the heading from the text goes with it.
  let after = at + 1;
  if ((lines[after] ?? "").trim() === "") after += 1;
  return { name, rest: lines.slice(after).join("\n") };
}

// ---- front matter -----------------------------------------------------------
// Byte-identical to `note::with_color`/`color_of`/`body_of` in the daemon:
// the two sides write the same header for the same note, so an autosave after
// a read never rewrites the file for no reason. One field, a closed set — a
// YAML parser here would be a dependency for `color: sage`.

// The length of the `---` line that opens a front matter, or 0 when there is none.
function openingLength(text: string) {
  if (text.startsWith("---\n")) return 4;
  return text.startsWith("---\r\n") ? 5 : 0;
}

export function splitFrontMatter(markdown: string | null | undefined) {
  const text = String(markdown || "");
  const open = openingLength(text);
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
    const value = re.exec(line.replace(/\r$/, ""))?.[1];
    if (value !== undefined) return set.includes(value) ? value : null;
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
    const raw = /^\s*title:\s*(.*?)\s*$/.exec(line.replace(/\r$/, ""))?.[1];
    if (raw === undefined) continue;
    if (!raw.startsWith('"')) return raw;
    return raw
      .slice(1, raw.endsWith('"') && raw.length > 1 ? -1 : undefined)
      .replaceAll(/\\(["\\])/gu, "$1");
  }
  return null;
}
export function titleYaml(name: string) {
  return '"' + name.replaceAll(/[\\"]/gu, "\\$&") + '"';
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
export function withHeader(
  body: string,
  title: string | null | undefined,
  style: Partial<NoteLook> | null | undefined,
  veiled?: boolean,
) {
  const name = String(title || "")
    .replaceAll(/[\r\n]+/gu, " ")
    .trim();
  const lines: string[] = [];
  if (name) lines.push(`title: ${titleYaml(name)}`);
  lines.push(`color: ${toneOf(style?.tone)}`, ...refinementLines(style));
  // Last, and only when true: `hidden: false` is what every note in the
  // world already is, and writing it would put a line in every file to say
  // nothing.
  if (veiled) lines.push("hidden: true");
  return `---\n${lines.join("\n")}\n---\n${body}`;
}

// The header lines of the look after the colour: a default is omitted.
function refinementLines(style: Partial<NoteLook> | null | undefined) {
  const fill = fillOf(style?.fill);
  const ink = inkOf(style?.ink);
  const font = fontOf(style?.font);
  const size = sizeOf(style?.size);
  const lines: string[] = [];
  if (fill !== DEFAULT_FILL) lines.push(`fill: ${fill}`);
  if (ink !== DEFAULT_INK) lines.push(`ink: ${ink}`);
  if (font !== DEFAULT_FONT) lines.push(`font: ${font}`);
  if (size !== DEFAULT_SIZE) lines.push(`size: ${size}`);
  return lines;
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
  const name = String(title || "")
    .replaceAll(/[\r\n]+/gu, " ")
    .trim();
  const body = bodyOf(markdown);
  const legacy = legacyTitleOf(body);
  return withHeader(legacy ? legacy.rest : body, name, styleOf(markdown), veiledOf(markdown));
}
