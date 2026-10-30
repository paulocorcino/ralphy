/* ---------------------------------------------------------------------------
   The one place that draws a Mermaid diagram, and the heading slug a
   `#fragment` jump matches.

   It imports no other module, so the markdown pane and the DBML view can
   both import it without an import cycle.
   --------------------------------------------------------------------------- */

/** The vendored mermaid (`vendor/mermaid`): the members this module calls. */
type Mermaid = {
  initialize(config: {
    startOnLoad: boolean;
    securityLevel: string;
    theme: string;
    htmlLabels: boolean;
    flowchart: { htmlLabels: boolean };
    maxTextSize: number;
  }): void;
  render(id: string, source: string): Promise<{ svg: string }>;
};

/** A window that may have loaded the vendored mermaid. */
export type MermaidWindow = Window & { mermaid?: Mermaid };

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
    // Mermaid's default limit is 50,000 characters, and over it Mermaid does
    // not throw: it draws a "Maximum text size in diagram exceeded" box
    // instead (measured, Mermaid 11.17.2). A real 79-table `erd.mmd` has
    // 61,823 characters. 2 MiB is the most `file.read` serves.
    maxTextSize: 2 * 1024 * 1024,
  });
  mermaidReady = true;
}

/**
 * Draw the Mermaid `source` as the SVG element `id`, and return its markup,
 * already sanitized for the page.
 */
export function drawDiagram(win: MermaidWindow, id: string, source: string): Promise<string> {
  initMermaid(win);
  const mermaid = win.mermaid;
  if (!mermaid) throw new Error("mermaid is not loaded");
  // The source is read raw (DOMPurify escaped it in the markdown pass), so
  // the SVG Mermaid returns has never met the sanitizer: it is sanitized
  // here, before it reaches the page.
  return mermaid
    .render(id, source)
    .then(({ svg }) =>
      DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true, html: true } }),
    );
}
