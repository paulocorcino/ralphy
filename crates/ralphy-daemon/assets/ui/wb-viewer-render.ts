/* ---------------------------------------------------------------------------
   The rendered body of the markdown pane, by file kind. The Mermaid draw is
   in `wb-dbml-draw.ts`.

   `wb-file-viewer.ts` owns the pane: its toolbar, the Find bar, Edit and
   Preview, Save, Reload and Detach. This module fills the pane's article
   from the file's text and returns the Mermaid holders that are still to be
   drawn. The draw waits until the pane is on screen, because a hidden
   container measures 0.
   --------------------------------------------------------------------------- */

import { filterDbml, renderDbmlGrid } from "./wb-dbml-view.ts";

/** The file kinds the markdown pane shows as a rendered body. */
export type RenderedKind = "markdown" | "mermaid" | "dbml";

export function isRendered(kind: string): kind is RenderedKind {
  return kind === "markdown" || kind === "mermaid" || kind === "dbml";
}

/**
 * Fill `article` with the body of a file of `kind` whose text is `content`,
 * and return the Mermaid holders still to draw. `uid` makes the holder ids
 * unique on the page.
 */
export function renderBody(
  kind: RenderedKind,
  article: Element,
  content: string,
  uid: number,
  doc: Document,
): HTMLElement[] {
  switch (kind) {
    case "markdown":
      return markdownBody(article, content, uid, doc);
    case "mermaid":
      return diagramBody(article, content, uid, doc);
    case "dbml":
      renderDbmlGrid(doc, article, content);
      return [];
    default: {
      const unknown: never = kind;
      throw new Error(`no rendered body for the kind ${String(unknown)}`);
    }
  }
}

/**
 * Get the rendered body ready for an in-page find of `term`, before the text
 * is searched: a table grid hides the cards the term does not match. An empty
 * term restores the body. A `kind` that is not rendered has nothing to do.
 */
export function prepareFind(kind: string, article: Element, term: string): void {
  if (!isRendered(kind)) return;
  switch (kind) {
    case "markdown":
    case "mermaid":
      return;
    case "dbml":
      filterDbml(article, term);
      return;
    default: {
      const unknown: never = kind;
      throw new Error(`no find for the kind ${String(unknown)}`);
    }
  }
}

function markdownBody(article: Element, content: string, uid: number, doc: Document) {
  article.innerHTML = DOMPurify.sanitize(marked.parse(content));

  // mermaid fences: marked emits <pre><code class="language-mermaid">. Defer
  // the actual draw to first paint (a hidden container measures as 0). An
  // inline `<code class="language-mermaid">` from raw HTML has no <pre> and
  // stays as code.
  const pending: HTMLElement[] = [];
  article.querySelectorAll("code.language-mermaid").forEach((code, i) => {
    const pre = code.closest("pre");
    if (!pre) return;
    const holder = doc.createElement("div");
    holder.className = "mermaid";
    holder.dataset.src = code.textContent;
    holder.id = `mmd-${uid}-${i}`;
    pre.replaceWith(holder);
    pending.push(holder);
  });
  return pending;
}

// A `.mmd` file is one diagram: the whole text is its source.
function diagramBody(article: Element, content: string, uid: number, doc: Document) {
  article.innerHTML = "";
  const holder = doc.createElement("div");
  holder.className = "mermaid";
  holder.id = `mmd-${uid}-0`;
  holder.dataset.src = content;
  article.append(holder);
  return [holder];
}
