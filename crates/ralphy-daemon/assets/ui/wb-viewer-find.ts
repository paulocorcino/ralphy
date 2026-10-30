// The viewer's find: the find bar of a rendered pane, and the find a code
// editor opens seeded with a term (a content-search open).
import { inPane } from "./wb-dom-need.ts";
import { prepareFind } from "./wb-viewer-render.ts";
import type { MonacoEditor } from "./wb-monaco.ts";
import type { ViewerRecord } from "./wb-file-viewer.ts";

/** What `createViewer` hands the find. */
export type ViewerFindDeps = {
  document: Document;
  /** Whether `rec` is still the record open under its id. */
  alive: (rec: ViewerRecord) => boolean;
};

export type ViewerFind = ReturnType<typeof createViewerFind>;

// Whether a text node of the article is visible text that a find may mark.
function takesText(n: Node) {
  const parent = n.parentElement;
  return (
    !!n.nodeValue?.trim() && !!parent && !parent.closest("svg, script, style, button, [hidden]")
  );
}

export function createViewerFind(deps: ViewerFindDeps) {
  const { document, alive } = deps;

  // --- in-page find over rendered markdown -------------------------------
  function clearHits(rec: ViewerRecord) {
    (rec.hits || []).forEach((mk) => {
      const t = document.createTextNode(mk.textContent);
      mk.replaceWith(t);
    });
    inPane(rec, ".md-body").normalize();
    rec.hits = [];
    rec.hitIdx = -1;
  }

  // Land a mounted code editor on the first occurrence of `term` and open the
  // find widget seeded with it, so F3/Enter walks the rest — the content
  // search's "jump to line", done with what Monaco already has. Literal and
  // case-insensitive, like the search that produced the hit.
  function findInEditor(rec: ViewerRecord, term: string) {
    const ed = rec.ed as MonacoEditor | undefined;
    if (!ed || !term) return;
    // The pane must be laid out BEFORE the widget opens: Monaco keeps the
    // widest "N of M" measurement in a module-wide maximum, and one taken in a
    // settling pane squeezed every find input on the page to 12px (MEASURED
    // 2026-09-15). A pane not on screen waits for `setActive`.
    if (!rec.visible) {
      rec.pendingFind = term;
      return;
    }
    ed.layout();
    requestAnimationFrame(() => {
      if (!alive(rec) || !rec.ed) return;
      const model = ed.getModel();
      const hit = model?.findNextMatch?.(
        term,
        { lineNumber: 1, column: 1 },
        false,
        false,
        null,
        false,
      );
      if (hit) {
        ed.setSelection(hit.range);
        ed.revealRangeInCenter(hit.range);
      }
      ed.focus();
      ed.trigger("wb-file-search", "actions.find", {
        searchString: term,
        isRegex: false,
        matchCase: false,
      });
    });
  }

  // The markdown pane's equivalent: its own find bar, opened and seeded.
  function findInMarkdown(rec: ViewerRecord, term: string) {
    const find = rec.el?.querySelector(".md-find");
    const input = rec.el?.querySelector<HTMLInputElement>(".md-find-input");
    if (!find || !input) return;
    find.classList.add("open");
    input.value = term;
    mdSearch(rec, term);
  }

  function mdSearch(rec: ViewerRecord, term: string) {
    clearHits(rec);
    const count = inPane(rec, ".md-find-count");
    const article = inPane(rec, ".md-body");
    prepareFind(rec.kind, article, term);
    if (!term) {
      count.textContent = "";
      return;
    }
    const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT, {
      acceptNode: (n) => (takesText(n) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT),
    });
    const targets: Text[] = [];
    let node;
    while ((node = walker.nextNode())) targets.push(node as Text);
    const needle = term.toLowerCase();
    const hits: HTMLElement[] = [];
    for (const text of targets) {
      const val = text.data;
      const lower = val.toLowerCase();
      let idx = lower.indexOf(needle);
      if (idx < 0) continue;
      const frag = document.createDocumentFragment();
      let last = 0;
      while (idx >= 0) {
        if (idx > last) frag.append(document.createTextNode(val.slice(last, idx)));
        const mk = document.createElement("mark");
        mk.className = "find-hit";
        mk.textContent = val.slice(idx, idx + term.length);
        frag.append(mk);
        hits.push(mk);
        last = idx + term.length;
        idx = lower.indexOf(needle, last);
      }
      if (last < val.length) frag.append(document.createTextNode(val.slice(last)));
      text.replaceWith(frag);
    }
    rec.hits = hits;
    rec.hitIdx = -1;
    count.textContent = hits.length ? `0/${hits.length}` : "No matches";
    if (hits.length) mdSearchStep(rec, 1);
  }

  function mdSearchStep(rec: ViewerRecord, dir: number) {
    const hits = rec.hits;
    if (!hits || !hits.length) return;
    const before = rec.hitIdx ?? -1;
    if (before >= 0) hits[before]?.classList.remove("current");
    const idx = (before + dir + hits.length) % hits.length;
    rec.hitIdx = idx;
    const mk = hits[idx];
    if (!mk) return;
    mk.classList.add("current");
    mk.scrollIntoView({ block: "center", behavior: "smooth" });
    inPane(rec, ".md-find-count").textContent = `${idx + 1}/${hits.length}`;
  }

  function mdSearchClose(rec: ViewerRecord) {
    mdSearch(rec, "");
    inPane(rec, ".md-find").classList.remove("open");
    inPane<HTMLInputElement>(rec, ".md-find-input").value = "";
  }

  return { findInEditor, findInMarkdown, mdSearch, mdSearchStep, mdSearchClose };
}
