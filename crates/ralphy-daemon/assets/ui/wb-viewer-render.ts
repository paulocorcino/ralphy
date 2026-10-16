/* ---------------------------------------------------------------------------
   The rendered body of the markdown pane, by file kind, and the one place
   that draws a Mermaid diagram.

   `wb-file-viewer.ts` owns the pane: its toolbar, the Find bar, Edit and
   Preview, Save, Reload and Detach. This module fills the pane's article
   from the file's text and returns the Mermaid holders that are still to be
   drawn. The draw waits until the pane is on screen, because a hidden
   container measures 0.
   --------------------------------------------------------------------------- */

/** The vendored mermaid (`vendor/mermaid`): the members this module calls. */
type Mermaid = {
  initialize(config: {
    startOnLoad: boolean;
    securityLevel: string;
    theme: string;
    htmlLabels: boolean;
    flowchart: { htmlLabels: boolean };
  }): void;
  render(id: string, source: string): Promise<{ svg: string }>;
};

/** A window that may have loaded the vendored mermaid. */
export type MermaidWindow = Window & { mermaid?: Mermaid };

/** The file kinds the markdown pane shows as a rendered body. */
export type RenderedKind = "markdown";

export function isRendered(kind: string): kind is RenderedKind {
  return kind === "markdown";
}

// The anchor a `#fragment` uses for a heading: the GitHub-style slug of the
// heading text, the spelling authors write. `marked` emits no heading ids
// (the outline assigns positional ones), so a jump matches by this slug.
export function slugOf(text: string) {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s+/g, "-");
}

/**
 * Fill `article` with the body of a file of `kind` whose text is `content`,
 * and return the Mermaid holders still to draw. `uid` makes the holder ids
 * unique on the page.
 */
export function renderBody(kind: RenderedKind, article: Element, content: string, uid: number, doc: Document): HTMLElement[] {
  switch (kind) {
    case "markdown":
      return markdownBody(article, content, uid, doc);
    default: {
      const unknown: never = kind;
      throw new Error(`no rendered body for the kind ${String(unknown)}`);
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

let mermaidReady = false;

export function initMermaid(win: MermaidWindow) {
  if (mermaidReady || !win.mermaid) return;
  // `strict`, not `loose`: diagram source is repo bytes (an agent-written plan,
  // a PR fixture, a cloned README), i.e. untrusted. `loose` skips mermaid's
  // own sanitize pass over the emitted SVG and turns a `click A "javascript:…"`
  // directive into a live <a href>. Nothing here calls `bindFunctions`, so
  // click bindings were never wired up and `strict` costs no working feature.
  // `htmlLabels: false` is LOAD-BEARING, not a style (found while building
  // ADR-0064 §15): mermaid's default label is HTML inside a
  // `<foreignObject>`, and DOMPurify 3.4 dropped that tag from its SVG
  // allowlist — so `drawDiagram`'s sanitize pass below removed every label
  // and the diagram arrived as unlabelled boxes. MEASURED against 3.4.12.
  // Plain `<text>` labels survive it untouched.
  win.mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "dark",
    htmlLabels: false,
    flowchart: { htmlLabels: false },
  });
  mermaidReady = true;
}

/**
 * Draw the Mermaid `source` as the SVG element `id`, and return its markup,
 * already sanitized for the page.
 */
export function drawDiagram(win: MermaidWindow, id: string, source: string): Promise<string> {
  initMermaid(win);
  // The source is read raw (DOMPurify escaped it in the markdown pass), so
  // the SVG Mermaid returns has never met the sanitizer: it is sanitized
  // here, before it reaches the page.
  return win.mermaid!
    .render(id, source)
    .then(({ svg }) => DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true, html: true } }));
}
