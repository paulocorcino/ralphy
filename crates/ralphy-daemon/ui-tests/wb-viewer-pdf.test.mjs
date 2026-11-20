// Unit tests for assets/ui/wb-viewer-pdf.ts, the PDF pane: the options it
// gives pdf.js (no scripts, no editor, range loading, safe web links), the
// zoom bar on the viewer's scale, and the text of a failed load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildPdf, PDF_CORE_PATH, PDF_VIEWER_PATH } from "../assets/ui/wb-viewer-pdf.ts";
import { VIEWERS, viewerKindOf } from "../assets/ui/wb-viewers.ts";
import { fileBytesUrl } from "../assets/ui/wb-file-bytes.ts";
import { VENDOR_FAILURE_TEXT } from "../assets/ui/wb-vendor-load.ts";

// An element that records its children, style and listeners, and finds the
// children a selector names through stubs.
function fakeEl(tag) {
  const listeners = new Map();
  const stubs = new Map();
  const el = {
    tag,
    attrs: {},
    style: {},
    dataset: {},
    className: "",
    textContent: "",
    value: "",
    children: [],
    clientWidth: 400,
    clientHeight: 300,
    setAttribute(name, value) {
      el.attrs[name] = value;
    },
    append(...kids) {
      el.children.push(...kids);
    },
    addEventListener(name, fn) {
      listeners.set(name, fn);
    },
    fire(name, ev = {}) {
      listeners.get(name)?.({ preventDefault() {}, ...ev });
    },
    querySelector(sel) {
      return stubs.get(sel) ?? null;
    },
    stub(sel) {
      const child = fakeEl(sel);
      stubs.set(sel, child);
      return child;
    },
  };
  return el;
}

const PANE_PARTS = [
  ".pdf-meta",
  ".pdf-body",
  ".pdf-pages",
  ".pdf-find",
  '[data-act="detach"]',
  ".viewer-path",
];

function PDFFindController() {
  return {};
}

// A fake pdf.js: the core and the viewer module, recording what the pane asks.
function fakePdfjs() {
  const seen = { documentOptions: null, viewerOptions: null, linkOptions: null, finds: [] };
  const doc = { numPages: 120 };
  const core = {
    GlobalWorkerOptions: { workerSrc: "" },
    AnnotationEditorType: { DISABLE: -1, NONE: 0 },
    AnnotationMode: { DISABLE: 0, ENABLE: 1, ENABLE_FORMS: 2 },
    getDocument(options) {
      seen.documentOptions = options;
      return { promise: Promise.resolve(doc) };
    },
  };
  // pdf.js calls these with `new`; a function that returns an object serves.
  function EventBus() {
    return {
      dispatch(name, detail) {
        if (name === "find") seen.finds.push(detail);
      },
    };
  }
  function PDFLinkService(options) {
    seen.linkOptions = options;
    return { setViewer() {}, setDocument() {} };
  }
  function PDFViewer(options) {
    seen.viewerOptions = options;
    return { currentScale: 1, currentScaleValue: "auto", setDocument() {} };
  }
  const viewerLib = {
    EventBus,
    PDFLinkService,
    PDFFindController,
    PDFViewer,
    LinkTarget: { NONE: 0, SELF: 1, BLANK: 2 },
  };
  return { seen, core, viewerLib };
}

function failedLoad() {
  return Promise.resolve({ ok: false, text: VENDOR_FAILURE_TEXT });
}

function settle() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

function pane(load) {
  const mount = fakeEl("viewers");
  const stubs = {};
  const head = fakeEl("head");
  const document = {
    head,
    createElement: (tag) => {
      const el = fakeEl(tag);
      if (tag === "div" && Object.keys(stubs).length === 0) {
        for (const sel of PANE_PARTS) stubs[sel] = el.stub(sel);
      }
      return el;
    },
  };
  const host = {
    document,
    viewers: mount,
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const rec = {
    id: "t",
    uid: 3,
    project: "me/proj",
    path: "docs/a b.pdf",
    checkout: "feat x",
    content: "",
    refused: null,
  };
  buildPdf(host, rec, load);
  return { rec, stubs, head };
}

async function opened() {
  const pdfjs = fakePdfjs();
  const asked = [];
  const load = (path) => {
    asked.push(path);
    const module = path === PDF_CORE_PATH ? pdfjs.core : pdfjs.viewerLib;
    return Promise.resolve({ ok: true, module });
  };
  const parts = pane(load);
  await settle();
  return { ...pdfjs, ...parts, asked };
}

test("scripting: the viewer runs with enableScripting false and no eval", async () => {
  const { seen } = await opened();
  assert.equal(seen.viewerOptions.enableScripting, false);
  assert.equal(seen.documentOptions.isEvalSupported, false);
});

test("editor: the annotation editor mode is the core's DISABLE", async () => {
  const { seen, core } = await opened();
  assert.equal(seen.viewerOptions.annotationEditorMode, core.AnnotationEditorType.DISABLE);
  assert.equal(seen.viewerOptions.annotationMode, core.AnnotationMode.ENABLE, "forms not filled");
});

test("range: the document loads from the bytes route with range requests on", async () => {
  const { seen, asked } = await opened();
  const opts = seen.documentOptions;
  assert.equal(opts.url, fileBytesUrl("me/proj", "docs/a b.pdf", "feat x"));
  assert.equal(opts.disableRange, false);
  assert.equal(opts.disableAutoFetch, true, "pages are read as they are shown");
  assert.deepEqual(asked, [PDF_CORE_PATH, PDF_VIEWER_PATH], "the core loads first");
});

test("links: a web link opens in a new tab with noopener noreferrer", async () => {
  const { seen, viewerLib } = await opened();
  assert.equal(seen.linkOptions.externalLinkTarget, viewerLib.LinkTarget.BLANK);
  assert.equal(seen.linkOptions.externalLinkRel, "noopener noreferrer");
});

test("zoom: the bar's Zoom in raises the viewer's currentScale", async () => {
  const { stubs } = await opened();
  const body = stubs[".pdf-body"];
  const bar = body.children.find((kid) => kid.className === "zoom-bar");
  const zoomIn = bar.children.find((kid) => kid.dataset.zoom === "in");
  const viewer = stubs[".pdf-pages"];
  assert.ok(viewer);
  zoomIn.fire("click");
  const percent = bar.children.find((kid) => kid.className === "zoom-bar-percent");
  assert.equal(percent.textContent, "125%");
});

test("search: Enter in the find field asks the viewer to find the text", async () => {
  const { stubs, seen } = await opened();
  const field = stubs[".pdf-find"];
  field.value = "budget";
  field.fire("keydown", { key: "Enter", shiftKey: false });
  assert.equal(seen.finds.length, 1);
  assert.equal(seen.finds[0].query, "budget");
});

test("a failed load of pdf.js shows the loader's text", async () => {
  const { stubs } = pane(failedLoad);
  await settle();
  assert.equal(stubs[".pdf-meta"].textContent, VENDOR_FAILURE_TEXT);
});

test("the table opens pdf files with the pdf builder", () => {
  assert.equal(viewerKindOf("pdf"), "pdf");
  assert.equal(VIEWERS.pdf.build, buildPdf);
});
