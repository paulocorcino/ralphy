// The odt pane (ADR-0091 D10, amendment 2026-11-23): SheetJS opens the zip,
// `wb-odt.ts` turns its text into DOM, and the DOM is drawn on a white page.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { fileBytesUrl, readWholeFile } from "./wb-file-bytes.ts";
import { devError } from "./wb-log.ts";
import { showLoading } from "./wb-viewer-loading.ts";
import { loadSheetJs } from "./wb-sheet-read.ts";
import { readOdt } from "./wb-odt.ts";
import { loadVendor } from "./wb-vendor-load.ts";
import type { VendorLoad } from "./wb-vendor-load.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const DOCUMENT_ERROR_TEXT = "This file could not be read as a document.";
export const DOCUMENT_TOO_LARGE_TEXT = "This document is too large to show.";

/** The steps of an odt pane: the file's bytes, and SheetJS; a test passes its own. */
export type DocumentDeps = {
  fetchBytes: (url: string) => Promise<Uint8Array | "too large">;
  load: (path: string) => Promise<VendorLoad>;
};

export const liveDocumentDeps: DocumentDeps = { fetchBytes: readWholeFile, load: loadVendor };

// The page of the document: the default paragraph style of the file over a
// white sheet, the way an office program shows it.
async function drawOdt(document: Document, deps: DocumentDeps, url: string, into: HTMLElement) {
  const [bytes, xlsx] = await Promise.all([deps.fetchBytes(url), loadSheetJs(deps.load)]);
  if (bytes === "too large") return DOCUMENT_TOO_LARGE_TEXT;
  if (typeof xlsx === "string") return xlsx;
  const read = readOdt(document, xlsx.CFB.read(bytes, { type: "array" }));
  const page = document.createElement("div");
  page.className = "odt-page";
  for (const [prop, value] of Object.entries(read.base)) page.style.setProperty(prop, value);
  page.append(...read.nodes);
  into.append(page);
  return "";
}

/** What a pane drawn from a file needs: its class, and how to draw into its body. */
export type DrawnPane = {
  className: string;
  bodyClass: string;
  // Draws into `into`; its text goes to the toolbar, "" when the draw is complete.
  draw: (into: HTMLElement) => Promise<string>;
  errorText: string;
};

/** A pane whose body a library draws from a file, with the loading note while it reads. */
export function buildDrawnPane(host: ViewerReadOnlyDeps, rec: BytesRecord, pane: DrawnPane) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = `viewer document-viewer ${pane.className}`;
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="document-meta"></span>
      <span class="spacer"></span>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body ${pane.bodyClass}"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);
  need(el, '[data-act="detach"]').addEventListener("click", () => {
    host.detachClick(rec);
  });
  const meta = need(el, ".document-meta");
  const loaded = showLoading(document, need(el, `.${pane.bodyClass}`));
  void pane.draw(need(el, `.${pane.bodyClass}`)).then(
    (text) => {
      loaded();
      meta.textContent = text;
      return null;
    },
    (err: unknown) => {
      loaded();
      devError(`[workbench] ${rec.path} failed to draw`, err);
      meta.textContent = pane.errorText;
    },
  );
  rec.el = el;
}

export function buildDocument(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  deps: DocumentDeps = liveDocumentDeps,
) {
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  buildDrawnPane(host, rec, {
    className: "odt-viewer",
    bodyClass: "document-body",
    draw: (into) => drawOdt(host.document, deps, url, into),
    errorText: DOCUMENT_ERROR_TEXT,
  });
}
