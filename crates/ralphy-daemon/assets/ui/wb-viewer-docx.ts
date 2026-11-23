// The docx pane (ADR-0091 D10, amendment 2026-11-23): docx-preview draws the
// file with its layout, DOMPurify cleans the result, and it goes into a
// shadow root, so the document's styles never reach the workbench.
import { setSanitizedDocument, markupOf } from "./wb-html.ts";
import { fileBytesUrl, readWholeFile } from "./wb-file-bytes.ts";
import { loadVendor } from "./wb-vendor-load.ts";
import {
  buildDrawnPane,
  DOCUMENT_ERROR_TEXT,
  DOCUMENT_TOO_LARGE_TEXT,
} from "./wb-viewer-document.ts";
import type { VendorLoad } from "./wb-vendor-load.ts";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const DOCX_PREVIEW_PATH = "./vendor/docx-preview/docx-preview.js";

/** The options of docx-preview: no HTML stored in the file, no comments, no changes. */
export const DOCX_OPTIONS = {
  renderAltChunks: false,
  renderComments: false,
  renderChanges: false,
  useBase64URL: false,
  experimental: false,
} as const;

type DocxOptions = typeof DOCX_OPTIONS;

/** The part of the docx-preview module the pane uses. */
export type DocxPreview = {
  parseAsync(data: Uint8Array, options: DocxOptions): Promise<object>;
  renderDocument(doc: object, options: DocxOptions): Promise<Node[]>;
};

function isDocxPreview(m: unknown): m is DocxPreview {
  return typeof m === "object" && m !== null && "parseAsync" in m && "renderDocument" in m;
}

/** The steps of a docx pane: the file's bytes and the library; a test passes its own. */
export type DocxDeps = {
  fetchBytes: (url: string) => Promise<Uint8Array | "too large">;
  load: (path: string) => Promise<VendorLoad>;
};

export const liveDocxDeps: DocxDeps = { fetchBytes: readWholeFile, load: loadVendor };

async function drawDocx(document: Document, deps: DocxDeps, url: string, into: HTMLElement) {
  const [bytes, outcome] = await Promise.all([deps.fetchBytes(url), deps.load(DOCX_PREVIEW_PATH)]);
  if (bytes === "too large") return DOCUMENT_TOO_LARGE_TEXT;
  if (!outcome.ok) return outcome.text;
  if (!isDocxPreview(outcome.module)) return DOCUMENT_ERROR_TEXT;
  const docx = outcome.module;
  const nodes = await docx.renderDocument(await docx.parseAsync(bytes, DOCX_OPTIONS), DOCX_OPTIONS);
  const drawn = document.createElement("div");
  drawn.append(...nodes);
  const host = document.createElement("div");
  host.className = "docx-page";
  into.append(host);
  setSanitizedDocument(host.attachShadow({ mode: "open" }), markupOf(drawn));
  return "";
}

export function buildDocx(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  deps: DocxDeps = liveDocxDeps,
) {
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  buildDrawnPane(host, rec, {
    className: "docx-viewer",
    bodyClass: "docx-body",
    draw: (into) => drawDocx(host.document, deps, url, into),
    errorText: DOCUMENT_ERROR_TEXT,
  });
}
