// The styles of an odt file, as CSS (ADR-0091 D10, amendment 2026-11-23). A
// style gives only the properties below, each value checked by its pattern,
// so nothing in the file becomes a URL, an expression or a selector.

/** A CSS property and the value an odt attribute gives it, or null. */
type Rule = { css: string; value: (raw: string) => string | null };

const LENGTH = /^-?\d+(\.\d+)?(cm|mm|in|pt|pc|px)$/u;
const COLOR = /^#[0-9a-f]{6}$/iu;
const ALIGN: Record<string, string> = {
  start: "left",
  left: "left",
  end: "right",
  right: "right",
  center: "center",
  justify: "justify",
};

const lengthOf = (raw: string) => (LENGTH.test(raw) ? raw : null);
const colorOf = (raw: string) => (COLOR.test(raw) ? raw : null);

/** The formatting attributes a style may give, by their local name. */
export const FORMAT_RULES: Record<string, Rule> = {
  "font-weight": { css: "font-weight", value: (raw) => (raw === "bold" ? "bold" : null) },
  "font-style": { css: "font-style", value: (raw) => (raw === "italic" ? "italic" : null) },
  "text-align": { css: "text-align", value: (raw) => ALIGN[raw] ?? null },
  "font-size": { css: "font-size", value: lengthOf },
  color: { css: "color", value: colorOf },
  "background-color": { css: "background-color", value: colorOf },
  "margin-left": { css: "margin-left", value: lengthOf },
  "margin-top": { css: "margin-top", value: lengthOf },
  "margin-bottom": { css: "margin-bottom", value: lengthOf },
  "text-indent": { css: "text-indent", value: lengthOf },
};

/** The CSS of one underline attribute: anything but "none" underlines. */
export function underlineOf(raw: string | null): string | null {
  return raw !== null && raw !== "" && raw !== "none" ? "underline" : null;
}

const FAMILY = /^[\p{L}\p{N} ._-]{1,64}$/u;

/** A font family as CSS: a name of letters, digits and spaces, quoted. */
export function familyOf(raw: string | null): string | null {
  const name = (raw ?? "").replaceAll(/^['"]|['"]$/gu, "").trim();
  return FAMILY.test(name) ? `"${name}", sans-serif` : null;
}

/** The CSS of a style: its own properties over its parent's, through at most 10 parents. */
export function resolveCss<T extends { css: Record<string, string>; parent: string | null }>(
  styles: Map<string, T>,
  name: string | null,
): Record<string, string> {
  const chain: Record<string, string>[] = [];
  let at = name;
  for (let depth = 0; at !== null && depth < 10; depth++) {
    const style = styles.get(at);
    if (!style) break;
    chain.unshift(style.css);
    at = style.parent;
  }
  const css: Record<string, string> = {};
  for (const one of chain) Object.assign(css, one);
  return css;
}
