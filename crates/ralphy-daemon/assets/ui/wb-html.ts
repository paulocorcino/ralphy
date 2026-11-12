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

/** Writes an SVG that DOMPurify already cleaned with its SVG profile. */
export function setSanitizedSvg(el: { innerHTML: string }, svg: string): void {
  write(el, svg);
}
