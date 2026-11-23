// The one writer of HTML markup: every other module builds nodes, or writes through here.

function write(el: { innerHTML: string }, html: string): void {
  // oxlint-disable-next-line no-restricted-properties -- the one writer of markup
  el.innerHTML = html;
}

/** Writes markup the workbench wrote itself: fixed icons and layout, no outside text. */
export function setMarkup(el: { innerHTML: string }, markup: string): void {
  write(el, markup);
}

/** Writes HTML that may hold outside text, after DOMPurify cleans it. */
export function setSanitizedHtml(el: { innerHTML: string }, html: string): void {
  write(el, DOMPurify.sanitize(html));
}

// A `url()` that is not a `blob:` or `data:` address, and an `@import`: in a
// drawn document's CSS, either would load something from outside the page.
const OUTSIDE_URL = /url\(\s*(?!["']?(?:blob|data):)[^)]*\)/giu;
const IMPORT_RULE = /@import[^;]*;?/giu;

/** The CSS of a drawn document with every outside address removed. */
export function insideCss(css: string): string {
  return css.replaceAll(IMPORT_RULE, "").replaceAll(OUTSIDE_URL, "none");
}

// The image source attribute: `src` of an HTML image, `href` of an SVG one.
const IMAGE_SOURCE: Record<string, string> = { IMG: "src", image: "href" };

function keepBlobImage(node: Element, data: PurifyAttribute) {
  const image = IMAGE_SOURCE[node.tagName] === data.attrName;
  if (image && data.attrValue.startsWith("blob:")) data.forceKeepAttr = true;
}

function insideOnly(node: Element) {
  if (node.tagName === "STYLE") node.textContent = insideCss(node.textContent);
  const style = node.getAttribute("style");
  if (style !== null) node.setAttribute("style", insideCss(style));
  if (node.tagName === "A" && node.hasAttribute("href")) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
}

/**
 * Writes a document a library drew from a file, after DOMPurify cleans it:
 * its `<style>` stays, an image keeps a `blob:` address, nothing loads from
 * outside, and a link opens in a new tab.
 */
export function setSanitizedDocument(el: { innerHTML: string }, html: string): void {
  DOMPurify.addHook("uponSanitizeAttribute", keepBlobImage);
  DOMPurify.addHook("afterSanitizeAttributes", insideOnly);
  try {
    write(el, DOMPurify.sanitize(html, { FORCE_BODY: true, ADD_TAGS: ["style"] }));
  } finally {
    DOMPurify.removeHook("uponSanitizeAttribute");
    DOMPurify.removeHook("afterSanitizeAttributes");
  }
}

/** Writes an SVG that DOMPurify already cleaned with its SVG profile. */
export function setSanitizedSvg(el: { innerHTML: string }, svg: string): void {
  write(el, svg);
}

/** Reads the markup of an element and of all that is inside it. */
export function markupOf(el: { outerHTML: string }): string {
  // oxlint-disable-next-line no-restricted-properties -- a read; the rule guards the writes
  return el.outerHTML;
}
