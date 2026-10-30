// The Markdown pane (and the Mermaid and DBML panes that share it): its
// toolbar, links, images, outline and the edit/preview toggle. The rendered
// body itself is `wb-viewer-render.ts`.
import { sendDocument } from "./wb-events.ts";
import { decorateZoomable } from "./wb-zoom-overlay.ts";
import { drawDiagram, slugOf } from "./wb-dbml-draw.ts";
import { isRendered, renderBody } from "./wb-viewer-render.ts";
import { encodingBtnHtml } from "./wb-viewer-encoding.ts";
import type { MermaidWindow } from "./wb-dbml-draw.ts";
import type { MonacoEditor } from "./wb-monaco.ts";
import type { BytesRecord, ViewerRecord } from "./wb-file-viewer.ts";
import type { ViewerEncoding } from "./wb-viewer-encoding.ts";
import type { ViewerFind } from "./wb-viewer-find.ts";
import type { ViewerSave } from "./wb-viewer-save.ts";

// The words of that notice, from the image's `src` and `alt` alone: a short
// `text` that keeps a row of badges one row, and the `reason` for its
// tooltip. A plain-`http:` image stays blocked even with remote images on
// (the policy admits `https:` only), so it gets its own reason.
export function remoteImageNotice(src: string, alt: string) {
  let url: URL | null = null;
  try {
    url = new URL(src);
  } catch {
    // Not an absolute URL: no host to name.
  }
  const what = alt.trim() || "image";
  const from = url?.host ? ` (${url.host})` : "";
  const reason =
    url?.protocol === "http:"
      ? "Images over plain http are not shown."
      : "Turn on Remote images in Security settings, then reload the page.";
  return { text: ` Image not shown: ${what}${from}`, reason };
}

// A markdown `src` folded against `dir` into a repo-relative path: query and
// fragment dropped, percent-escapes decoded (a `%20` in a filename is the
// markdown spelling of a space), `.`/`..` segments resolved. Returns `null`
// for anything that climbs OUT of the repo — the daemon would refuse it
// anyway, and not asking is the honest way to spell "not ours".
function repoRelative(dir: string, src: string) {
  let clean = src.split(/[?#]/)[0] ?? "";
  try {
    clean = decodeURIComponent(clean);
  } catch {
    // A malformed escape is not a path we can resolve; use it verbatim and let
    // the daemon refuse it.
  }
  const out: string[] = [];
  for (const part of (dir ? dir.split("/") : []).concat(clean.split("/"))) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (!out.length) return null;
      out.pop();
      continue;
    }
    out.push(part);
  }
  return out.join("/") || null;
}

// What a rendered link points at, decided from its `href` alone (no DOM, no
// daemon): the pure half of `linkClick`, so the decision table is testable.
//   • `#frag`                → { kind: "fragment", fragment }  — same document
//   • scheme or `/`-rooted   → { kind: "external" }            — the author's
//     explicit request for something outside the repo; left to the browser
//   • anything else          → { kind: "file", path, fragment } — a repo file,
//     folded against the document's own directory like an `<img src>`
//   • climbs out of the repo → null — not ours, and the daemon would refuse it
export function linkTarget(dir: string, href: string | null) {
  if (!href) return null;
  if (href.startsWith("#")) return { kind: "fragment" as const, fragment: href.slice(1) };
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith("/")) return { kind: "external" as const };
  const path = repoRelative(dir, href);
  if (!path) return null;
  const hash = href.indexOf("#");
  return { kind: "file" as const, path, fragment: hash < 0 ? "" : href.slice(hash + 1) };
}

/** What `createViewer` hands the Markdown pane. */
export type ViewerMarkdownDeps = {
  window: MermaidWindow;
  document: Document;
  /** The element that holds every pane; null where a test has none. */
  viewers: HTMLElement | null;
  setPathLabel: (el: HTMLElement, rec: ViewerRecord) => void;
  setCaption: (btn: HTMLElement, icon: string, caption: string) => void;
  detachBtnHtml: (rec: ViewerRecord) => string;
  detachClick: (rec: BytesRecord) => void;
  /** Mount a code editor in `container`: the pane's Edit mode. */
  mountEditor: (rec: BytesRecord, container: HTMLElement, opts: { path?: string; wordWrap?: string }) => unknown;
  saver: Pick<ViewerSave, "save" | "reloadFile" | "applyFresh" | "hideDiskBadge">;
  encoding: Pick<ViewerEncoding, "wireEncodingMenu">;
  find: Pick<ViewerFind, "mdSearch" | "mdSearchStep" | "mdSearchClose">;
};

export type ViewerMarkdown = ReturnType<typeof createViewerMarkdown>;

export function createViewerMarkdown(deps: ViewerMarkdownDeps) {
  const { window, document, viewers, setPathLabel, setCaption, detachBtnHtml, detachClick, mountEditor } = deps;
  const { save, reloadFile, applyFresh, hideDiskBadge } = deps.saver;
  const { wireEncodingMenu } = deps.encoding;
  const { mdSearch, mdSearchStep, mdSearchClose } = deps.find;

  // --- a Markdown tab -----------------------------------------------------
  function buildMarkdown(rec: BytesRecord) {
    const el = document.createElement("div");
    el.className = "viewer md-viewer";
    el.dataset.tabId = rec.id;
    el.dataset.kind = rec.kind;
    el.style.display = "none";
    el.innerHTML = `
      <div class="viewer-toolbar">
        <span class="viewer-path"></span>
        <button class="vbtn md-toc-btn" data-act="outline" title="Contents" aria-label="Contents"><i class="bi bi-list-ul"></i></button>
        <span class="spacer"></span>
        <button class="vbtn" data-act="find" title="Find" aria-label="Find"><i class="bi bi-search"></i><span class="vbtn-label">Find</span></button>
        <button class="vbtn" data-act="reload" title="Reload" aria-label="Reload"><i class="bi bi-arrow-clockwise"></i><span class="vbtn-label">Reload</span></button>
        <button class="vbtn" data-act="toggle" title="Edit" aria-label="Edit"><i class="bi bi-pencil"></i><span class="vbtn-label">Edit</span></button>
        <button class="vbtn viewer-disk-badge" data-act="disk" style="display:none" title="Changed on disk — reload" aria-label="Changed on disk — reload"><i class="bi bi-exclamation-triangle"></i><span class="vbtn-label">Changed on disk — reload</span></button>
        <span class="viewer-save-err" style="display:none"></span>
        ${encodingBtnHtml(rec)}
        <button class="vbtn save" data-act="save" title="Save" aria-label="Save"><i class="bi bi-save"></i><span class="vbtn-label">Save</span></button>
        ${detachBtnHtml(rec)}
      </div>
      <div class="md-find">
        <input class="md-find-input" placeholder="Find in page…" />
        <span class="md-find-count"></span>
        <button class="vbtn" data-find="prev" title="Previous match" aria-label="Previous match"><i class="bi bi-chevron-up"></i></button>
        <button class="vbtn" data-find="next" title="Next match" aria-label="Next match"><i class="bi bi-chevron-down"></i></button>
        <button class="vbtn" data-find="close" title="Close search" aria-label="Close search"><i class="bi bi-x"></i></button>
      </div>
      <div class="md-split">
        <nav class="md-outline"></nav>
        <div class="md-scroll"><article class="md-body"></article></div>
        <div class="md-editor" style="display:none"></div>
      </div>`;
    setPathLabel(el, rec);
    viewers!.append(el);
    wireEncodingMenu(rec, el);
    rec.el = el;
    rec.saveBtn = el.querySelector<HTMLElement>('[data-act="save"]');

    // edit / preview toggle
    el.querySelector<HTMLElement>('[data-act="toggle"]')!.onclick = () => toggleEdit(rec);
    // The heading outline on a NARROW pane: the same <nav>, laid over the
    // article by CSS while `.md-split` carries `toc-open` (on a wide pane the
    // button is not shown and the nav is the column it always was). A jump or
    // a tap on the article closes it — the index is a way in, not a fixture.
    const split = el.querySelector(".md-split")!;
    el.querySelector<HTMLElement>('[data-act="outline"]')!.onclick = () => split.classList.toggle("toc-open");
    el.querySelector(".md-outline")!.addEventListener("click", (ev) => {
      if ((ev.target as Element).closest(".outline-item")) split.classList.remove("toc-open");
    });
    el.querySelector(".md-scroll")!.addEventListener("pointerdown", () => split.classList.remove("toc-open"));
    el.querySelector<HTMLElement>('[data-act="save"]')!.onclick = () => save(rec);
    el.querySelector<HTMLElement>('[data-act="reload"]')!.onclick = () => reloadFile(rec);
    el.querySelector<HTMLElement>('[data-act="disk"]')!.onclick = () => {
      applyFresh(rec, rec.pendingDisk!);
      hideDiskBadge(rec);
    };
    el.querySelector<HTMLElement>('[data-act="detach"]')!.onclick = () => detachClick(rec);
    // in-page find over the rendered article
    const find = el.querySelector(".md-find")!;
    const input = el.querySelector<HTMLInputElement>(".md-find-input")!;
    el.querySelector<HTMLElement>('[data-act="find"]')!.onclick = () => {
      find.classList.add("open");
      input.focus();
      input.select();
    };
    input.addEventListener("input", () => mdSearch(rec, input.value));
    input.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Enter") mdSearchStep(rec, e.shiftKey ? -1 : 1);
      if (e.key === "Escape") mdSearchClose(rec);
    });
    el.querySelector<HTMLElement>('[data-find="next"]')!.onclick = () => mdSearchStep(rec, 1);
    el.querySelector<HTMLElement>('[data-find="prev"]')!.onclick = () => mdSearchStep(rec, -1);
    el.querySelector<HTMLElement>('[data-find="close"]')!.onclick = () => mdSearchClose(rec);
    // Links inside the rendered article: one delegated listener for the pane's
    // lifetime, so a re-render (reload, edit→preview) never re-wires anything.
    el.querySelector(".md-body")!.addEventListener("click", (ev) => linkClick(rec, ev));

    renderMarkdown(rec);
  }

  function renderMarkdown(rec: BytesRecord) {
    if (!isRendered(rec.kind)) return;
    const article = rec.el!.querySelector(".md-body")!;
    rec.mermaidPending = renderBody(rec.kind, article, rec.content, rec.uid, document);
    resolveImages(rec, article);
    decorateZoomable(article);
    buildOutline(rec, article);
    if (rec.visible) drawMermaid(rec);
  }

  // Repo-relative `<img>` sources resolve through `file.image` (ADR-0049 §5),
  // against the DOCUMENT's own directory. This runs on the SANITIZED DOM, after
  // DOMPurify, so nothing set here re-enters the sanitizer's decision. An
  // absolute or `http(s)` source is the author's explicit request for a remote
  // asset and is left exactly as written — the daemon's CSP decides whether it
  // loads, and `blockedImage` explains a refusal; a source that REFUSES is left alone
  // too — a broken image is an honest rendering of a broken link, and a
  // placeholder would fabricate.
  function resolveImages(rec: ViewerRecord, article: Element) {
    if (!window.WBDaemon?.readImage) return;
    const dir = rec.path.includes("/") ? rec.path.slice(0, rec.path.lastIndexOf("/")) : "";
    article.querySelectorAll<HTMLImageElement>("img[src]").forEach((img) => {
      const src = img.getAttribute("src") || "";
      // Anything carrying a scheme (`data:`, `https:`) or rooted at `/` is not
      // ours to resolve.
      if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("/")) return;
      const rel = repoRelative(dir, src);
      if (!rel) return;
      WBDaemon.readImage(rec.project, rel, undefined, rec.checkout)
        .then((url) => {
          if (url) img.src = url;
        })
        .catch(() => {});
    });
  }

  // An image the page's CSP refused (a remote one, while remote images are
  // off: ADR-0032 amendment 2026-09-27) says so, instead of a broken icon. The
  // event comes from the browser, so the notice only shows when the policy of
  // THIS page really blocked the image. Chromium 140 fires it at the document
  // with no element (measured), so the article's images are matched by URL.
  // Built with `textContent`, never `innerHTML`: the alt text is document
  // content.
  function blockedImage(ev: SecurityPolicyViolationEvent) {
    if (ev.effectiveDirective !== "img-src" || !ev.blockedURI) return;
    document.querySelectorAll<HTMLImageElement>(".md-body img").forEach((img) => {
      if (img.src !== ev.blockedURI) return;
      const note = document.createElement("span");
      note.className = "md-img-blocked";
      const icon = document.createElement("i");
      icon.className = "bi bi-image";
      const { text, reason } = remoteImageNotice(img.getAttribute("src") || "", img.getAttribute("alt") || "");
      note.title = reason;
      note.append(icon, text);
      img.replaceWith(note);
    });
  }

  // A click on a rendered `<a>`: a raw `href` would navigate the WHOLE
  // window. A repo file becomes an open REQUEST to the shell; external links
  // open in a new tab so the workbench is not what gets replaced.
  function linkClick(rec: ViewerRecord, ev: Event) {
    const a = (ev.target as Element).closest?.<HTMLAnchorElement>("a[href]");
    if (!a || !rec.el!.contains(a)) return;
    const dir = rec.path.includes("/") ? rec.path.slice(0, rec.path.lastIndexOf("/")) : "";
    const target = linkTarget(dir, a.getAttribute("href"));
    if (!target) {
      ev.preventDefault();
      return;
    }
    if (target.kind === "external") {
      a.target = "_blank";
      a.rel = "noopener";
      return;
    }
    ev.preventDefault();
    if (target.kind === "fragment") {
      jumpTo(rec, target.fragment);
      return;
    }
    sendDocument(document, "workbench:open-request", {
      project: rec.project,
      path: target.path,
      fragment: target.fragment,
      checkout: rec.checkout ?? null,
    });
  }

  // Scroll a rendered document to the heading a `#fragment` names, matched
  // by the slug of the heading text (`slugOf`).
  function jumpTo(rec: ViewerRecord, fragment: string) {
    if (!fragment) return;
    let want = fragment;
    try {
      want = decodeURIComponent(fragment);
    } catch {
      // A malformed escape still names SOMETHING; match it as written.
    }
    want = want.toLowerCase();
    const heads = rec.el!.querySelectorAll(".md-body h1, .md-body h2, .md-body h3, .md-body h4, .md-body h5, .md-body h6");
    for (const h of heads) {
      if (slugOf(h.textContent) === want) {
        h.scrollIntoView({ behavior: "smooth", block: "start" });
        return;
      }
    }
  }

  function drawMermaid(rec: ViewerRecord) {
    if (!rec.mermaidPending || !rec.mermaidPending.length) return;
    const pending = rec.mermaidPending;
    rec.mermaidPending = [];
    pending.forEach((holder) => {
      // `drawDiagram` returns the SVG already sanitized.
      drawDiagram(window, holder.id + "-svg", holder.dataset.src!)
        .then((svg) => (holder.innerHTML = svg))
        .catch((err: unknown) => {
          holder.classList.add("mermaid-error");
          holder.textContent = "Mermaid error: " + String(err instanceof Error && err.message ? err.message : err);
        });
    });
  }

  // Heading outline: the jump index, one entry per heading, indented by level.
  function buildOutline(rec: ViewerRecord, article: Element) {
    const nav = rec.el!.querySelector(".md-outline")!;
    nav.innerHTML = "";
    const heads = article.querySelectorAll("h1, h2, h3, h4");
    if (!heads.length) {
      nav.innerHTML = '<div class="outline-empty">No headings</div>';
      return;
    }
    heads.forEach((h, i) => {
      const id = `h-${rec.uid}-${i}`;
      h.id = id;
      const a = document.createElement("a");
      a.className = "outline-item lvl-" + h.tagName.toLowerCase();
      a.textContent = h.textContent;
      a.title = h.textContent;
      a.onclick = () => h.scrollIntoView({ behavior: "smooth", block: "start" });
      nav.append(a);
    });
  }

  // Swap the markdown pane between rendered preview and a raw-source editor.
  function toggleEdit(rec: BytesRecord) {
    const split = rec.el!.querySelector(".md-split")!;
    const editor = rec.el!.querySelector<HTMLElement>(".md-editor")!;
    const toggle = rec.el!.querySelector<HTMLElement>('[data-act="toggle"]')!;
    rec.editing = !rec.editing;
    if (rec.editing) {
      split.classList.add("editing");
      split.classList.remove("toc-open");
      editor.style.display = "block";
      if (!rec.ed) {
        mountEditor(rec, editor, { wordWrap: "on" });
      } else {
        (rec.ed as MonacoEditor).setValue(rec.content);
      }
      setCaption(toggle, "bi-eye", "Preview");
      setTimeout(() => rec.ed?.layout(), 0);
    } else {
      // `rec.editing` is already false here, so read the editor directly —
      // contentOf() would hand back the pre-edit bytes.
      if (rec.ed) rec.content = (rec.ed as MonacoEditor).getValue();
      split.classList.remove("editing");
      editor.style.display = "none";
      setCaption(toggle, "bi-pencil", "Edit");
      renderMarkdown(rec);
      if (rec.visible) drawMermaid(rec);
    }
  }

  // One listener for every markdown pane; a test's stub document has none.
  document.addEventListener?.("securitypolicyviolation", blockedImage);
  return { buildMarkdown, renderMarkdown, drawMermaid, jumpTo };
}
