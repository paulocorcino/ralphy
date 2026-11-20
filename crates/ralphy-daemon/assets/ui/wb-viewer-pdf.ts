// The PDF pane (ADR-0091 D9): pdf.js draws the pages, with the text layer and
// its search, and reads the file in ranges from the file bytes route (D2). The
// PDF's scripts never run, the annotation editor is off, a form shows and is
// never filled, and a web link opens in a new tab with `noopener noreferrer`.
import { setMarkup } from "./wb-html.ts";
import { need, needInput, required } from "./wb-dom-need.ts";
import { fileBytesUrl } from "./wb-file-bytes.ts";
import { clampScale } from "./wb-diagram-zoom.ts";
import type { Pair, Point } from "./wb-diagram-zoom.ts";
import { buildZoomBar } from "./wb-zoom-bar.ts";
import type { ZoomDrive } from "./wb-zoom-bar.ts";
import { loadVendor } from "./wb-vendor-load.ts";
import type { VendorLoad } from "./wb-vendor-load.ts";
import { devError } from "./wb-log.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

// `pdf.min.mjs` sets `globalThis.pdfjsLib`, which `pdf_viewer.mjs` reads when
// it loads: the core always loads first.
export const PDF_CORE_PATH = "./vendor/pdfjs/pdf.min.mjs";
export const PDF_VIEWER_PATH = "./vendor/pdfjs/pdf_viewer.mjs";
const PDF_CSS_PATH = "./vendor/pdfjs/pdf_viewer.css";
const WORKER_PATH = "./vendor/pdfjs/pdf.worker.min.mjs";
const WASM_PATH = "./vendor/pdfjs/wasm/";
const FONTS_PATH = "./vendor/pdfjs/standard_fonts/";
const MIN_SCALE = 0.1;
const FIT = "page-width";
export const PDF_ERROR_TEXT = "This file could not be opened.";

type PdfDocument = { numPages: number };
type PdfSource = {
  url: string;
  disableRange: boolean;
  disableStream: boolean;
  disableAutoFetch: boolean;
  isEvalSupported: boolean;
  enableXfa: boolean;
  wasmUrl: string;
  standardFontDataUrl: string;
};
type PdfCore = {
  GlobalWorkerOptions: { workerSrc: string };
  AnnotationEditorType: { DISABLE: number };
  AnnotationMode: { ENABLE: number };
  getDocument(src: PdfSource): { promise: Promise<PdfDocument> };
};
type EventBus = { dispatch(name: string, detail: Record<string, unknown>): void };
type LinkService = {
  setViewer(viewer: PdfView): void;
  setDocument(doc: PdfDocument): void;
};
type PdfView = {
  currentScale: number;
  currentScaleValue: string;
  setDocument(doc: PdfDocument): void;
};
type PdfViewerLib = {
  EventBus: new () => EventBus;
  PDFLinkService: new (o: {
    eventBus: EventBus;
    externalLinkTarget: number;
    externalLinkRel: string;
  }) => LinkService;
  PDFFindController: new (o: { eventBus: EventBus; linkService: LinkService }) => object;
  PDFViewer: new (o: Record<string, unknown>) => PdfView;
  LinkTarget: { BLANK: number };
};

function isPdfCore(m: unknown): m is PdfCore {
  return typeof m === "object" && m !== null && "getDocument" in m && "AnnotationMode" in m;
}

function isPdfViewerLib(m: unknown): m is PdfViewerLib {
  return typeof m === "object" && m !== null && "PDFViewer" in m && "LinkTarget" in m;
}

const styledDocuments = new WeakSet<Document>();

function linkStyle(document: Document) {
  if (styledDocuments.has(document)) return;
  styledDocuments.add(document);
  const link = document.createElement("link");
  link.setAttribute("rel", "stylesheet");
  link.setAttribute("href", PDF_CSS_PATH);
  document.head.append(link);
}

/** The options of the document: ranges of the bytes route, nothing that runs code. */
export function pdfSource(url: string): PdfSource {
  return {
    url,
    disableRange: false,
    disableStream: true,
    disableAutoFetch: true,
    isEvalSupported: false,
    enableXfa: false,
    wasmUrl: WASM_PATH,
    standardFontDataUrl: FONTS_PATH,
  };
}

function distance([a, b]: Pair) {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

// The zoom bar's drive over pdf.js's own scale. Fit is the page width.
function pdfDrive(view: PdfView, scroller: HTMLElement): ZoomDrive {
  const zoom = (factor: number) => {
    view.currentScale = clampScale(view.currentScale * factor, MIN_SCALE);
  };
  return {
    fit: () => {
      view.currentScaleValue = FIT;
    },
    zoom: (factor: number, _at: Point) => {
      zoom(factor);
    },
    actual: (_at: Point) => {
      view.currentScale = 1;
    },
    toggle: (_at: Point) => {
      if (view.currentScaleValue === FIT) view.currentScale = 1;
      else view.currentScaleValue = FIT;
    },
    pan: (dx: number, dy: number) => {
      scroller.scrollBy(-dx, -dy);
    },
    pinch: (prev: Pair, next: Pair) => {
      const before = distance(prev);
      if (before > 0) zoom(distance(next) / before);
    },
    resize: () => {
      if (view.currentScaleValue === FIT) view.currentScaleValue = FIT;
    },
    scale: () => view.currentScale,
  };
}

function wireFind(field: HTMLInputElement, bus: EventBus) {
  let last = "";
  field.addEventListener("keydown", (ev: KeyboardEvent) => {
    if (ev.key !== "Enter") return;
    ev.preventDefault();
    const query = field.value;
    bus.dispatch("find", {
      source: field,
      type: query === last ? "again" : "",
      query,
      caseSensitive: false,
      entireWord: false,
      highlightAll: true,
      findPrevious: ev.shiftKey,
      matchDiacritics: false,
    });
    last = query;
  });
}

type Pane = { el: HTMLElement; body: HTMLElement; pages: HTMLElement; meta: HTMLElement };

type Pdfjs = { core: PdfCore; lib: PdfViewerLib };

// Draw the document in the pane once both parts of pdf.js are loaded.
async function openDocument(host: ViewerReadOnlyDeps, pane: Pane, url: string, pdfjs: Pdfjs) {
  const { core, lib } = pdfjs;
  core.GlobalWorkerOptions.workerSrc = WORKER_PATH;
  const eventBus = new lib.EventBus();
  const linkService = new lib.PDFLinkService({
    eventBus,
    externalLinkTarget: lib.LinkTarget.BLANK,
    externalLinkRel: "noopener noreferrer",
  });
  const findController = new lib.PDFFindController({ eventBus, linkService });
  const view = new lib.PDFViewer({
    container: pane.body,
    viewer: pane.pages,
    eventBus,
    linkService,
    findController,
    enableScripting: false,
    annotationEditorMode: core.AnnotationEditorType.DISABLE,
    annotationMode: core.AnnotationMode.ENABLE,
  });
  linkService.setViewer(view);
  const bar = buildZoomBar(host.document, pdfDrive(view, pane.body), pane.body);
  pane.body.append(bar.el);
  wireFind(needInput(pane.el, ".pdf-find"), eventBus);
  const doc = await core.getDocument(pdfSource(url)).promise;
  view.setDocument(doc);
  linkService.setDocument(doc);
  view.currentScaleValue = FIT;
  bar.refresh();
  pane.meta.textContent = `${String(doc.numPages)} pages`;
}

async function loadPdfjs(load: (path: string) => Promise<VendorLoad>): Promise<Pdfjs | string> {
  const core = await load(PDF_CORE_PATH);
  if (!core.ok) return core.text;
  if (!isPdfCore(core.module)) return PDF_ERROR_TEXT;
  const viewer = await load(PDF_VIEWER_PATH);
  if (!viewer.ok) return viewer.text;
  if (!isPdfViewerLib(viewer.module)) return PDF_ERROR_TEXT;
  return { core: core.module, lib: viewer.module };
}

function buildShell(host: ViewerReadOnlyDeps, rec: BytesRecord): Pane {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer pdf-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="pdf-meta"></span>
      <span class="spacer"></span>
      <input class="pdf-find" type="search" placeholder="Find" aria-label="Find in the PDF" />
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body pdf-body"><div class="pdfViewer pdf-pages"></div></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
  const body = need(el, ".pdf-body");
  // pdf.js needs its scroll container absolutely positioned.
  body.style.position = "absolute";
  body.style.inset = "var(--viewer-toolbar-h, 2.25rem) 0 0 0";
  body.style.overflow = "auto";
  el.style.position = "relative";
  return { el, body, pages: need(el, ".pdf-pages"), meta: need(el, ".pdf-meta") };
}

export function buildPdf(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  load: (path: string) => Promise<VendorLoad> = loadVendor,
) {
  linkStyle(host.document);
  const pane = buildShell(host, rec);
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  void loadPdfjs(load)
    .then((pdfjs) => {
      if (typeof pdfjs === "string") {
        pane.meta.textContent = pdfjs;
        return null;
      }
      return openDocument(host, pane, url, pdfjs);
    })
    .catch((err: unknown) => {
      devError("[workbench] the PDF failed to open", err);
      pane.meta.textContent = PDF_ERROR_TEXT;
    });
  rec.el = pane.el;
}
