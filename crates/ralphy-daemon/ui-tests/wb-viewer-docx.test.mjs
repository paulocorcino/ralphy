// Unit tests for assets/ui/wb-viewer-docx.ts and the document writer of
// wb-html.ts: the options docx-preview gets, the shadow root the drawing goes
// into, and what the sanitizing keeps and removes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildDocx, DOCX_OPTIONS, DOCX_PREVIEW_PATH } from "../assets/ui/wb-viewer-docx.ts";
import { insideCss, setSanitizedDocument } from "../assets/ui/wb-html.ts";
import { VIEWERS, viewerKindOf } from "../assets/ui/wb-viewers.ts";

function fakeEl(tag, attrs = {}) {
  const el = {
    tagName: tag,
    attrs: { ...attrs },
    className: "",
    dataset: {},
    style: {},
    textContent: "",
    innerHTML: "",
    outerHTML: `<${tag}></${tag}>`,
    children: [],
    shadow: null,
    append(...kids) {
      for (const kid of kids) kid.parent = el;
      el.children.push(...kids);
    },
    remove() {
      el.parent.children = el.parent.children.filter((kid) => kid !== el);
    },
    attachShadow(options) {
      el.shadow = { options, innerHTML: "" };
      return el.shadow;
    },
    getAttribute: (name) => (name in el.attrs ? el.attrs[name] : null),
    setAttribute(name, value) {
      el.attrs[name] = value;
    },
    hasAttribute: (name) => name in el.attrs,
    addEventListener() {},
    querySelector: (sel) => el.stubs?.[sel] ?? null,
  };
  return el;
}

// A DOMPurify that records its config and runs the hooks on given nodes.
function fakePurify(nodes, attrs) {
  const seen = { config: null, hooks: new Map(), removed: [] };
  globalThis.DOMPurify = {
    addHook: (entry, fn) => seen.hooks.set(entry, fn),
    removeHook: (entry) => seen.removed.push(entry),
    sanitize(html, config) {
      seen.config = config;
      for (const [node, data] of attrs) seen.hooks.get("uponSanitizeAttribute")(node, data);
      for (const node of nodes) seen.hooks.get("afterSanitizeAttributes")(node);
      return `clean:${html}`;
    },
  };
  return seen;
}

test("css: an outside url and an @import go; blob and data addresses stay", () => {
  const css =
    "@import 'https://x.test/a.css'; p { background: url(https://x.test/i.png) }" +
    " @font-face { src: url(blob:http://h/1) } i { background: url('data:image/png;base64,AA') }";
  const out = insideCss(css);
  assert.equal(out.includes("x.test"), false, out);
  assert.equal(out.includes("url(blob:http://h/1)"), true);
  assert.equal(out.includes("url('data:image/png"), true);
});

test("sanitize: style kept, blob images kept, links safe, hooks removed after", () => {
  const style = fakeEl("STYLE");
  style.textContent = "p { background: url(https://x.test/i.png) }";
  const link = fakeEl("A", { href: "https://example.com" });
  const img = fakeEl("IMG");
  const blob = { attrName: "src", attrValue: "blob:http://h/1" };
  const web = { attrName: "src", attrValue: "https://x.test/i.png" };
  const svgImage = { attrName: "href", attrValue: "blob:http://h/2" };
  const seen = fakePurify(
    [style, link],
    [
      [img, blob],
      [img, web],
      [fakeEl("image"), svgImage],
    ],
  );
  const root = { innerHTML: "" };
  try {
    setSanitizedDocument(root, "<p>x</p>");
  } finally {
    delete globalThis.DOMPurify;
  }
  // oxlint-disable-next-line no-restricted-properties -- reads a fake element; the rule is for writes
  assert.equal(root.innerHTML, "clean:<p>x</p>");
  assert.deepEqual(seen.config, { FORCE_BODY: true, ADD_TAGS: ["style"] });
  assert.equal(blob.forceKeepAttr, true, "a blob image source stays");
  assert.equal(svgImage.forceKeepAttr, true, "a blob SVG image source stays");
  assert.equal(web.forceKeepAttr, undefined, "a web image source is left to DOMPurify");
  assert.equal(style.textContent.includes("x.test"), false, "an outside url leaves the CSS");
  assert.equal(link.attrs.target, "_blank");
  assert.equal(link.attrs.rel, "noopener noreferrer");
  assert.deepEqual(seen.removed, ["uponSanitizeAttribute", "afterSanitizeAttributes"]);
});

function settle() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

function docxPane(deps) {
  const made = [];
  const document = {
    createElement(tag) {
      const el = fakeEl(tag);
      made.push(el);
      if (made.length === 1) {
        el.stubs = {};
        for (const sel of [".document-meta", ".docx-body", '[data-act="detach"]']) {
          el.stubs[sel] = fakeEl(sel);
        }
      }
      return el;
    },
  };
  const host = {
    document,
    viewers: fakeEl("viewers"),
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const rec = { id: "t", project: "me/proj", path: "a.docx", checkout: null };
  buildDocx(host, rec, deps);
  return { pane: made[0], made };
}

test("draw: docx-preview gets the safe options and the drawing goes into a shadow root", async () => {
  const seen = { parse: null, render: null, path: null };
  const module = {
    parseAsync: (bytes, options) => {
      seen.parse = options;
      return Promise.resolve({ bytes });
    },
    renderDocument: (doc, options) => {
      seen.render = options;
      return Promise.resolve([fakeEl("section")]);
    },
  };
  const deps = {
    fetchBytes: () => Promise.resolve(new Uint8Array([1])),
    load: (path) => {
      seen.path = path;
      return Promise.resolve({ ok: true, module });
    },
  };
  globalThis.DOMPurify = { addHook() {}, removeHook() {}, sanitize: (h) => h };
  try {
    const { pane } = docxPane(deps);
    await settle();
    await settle();
    const body = pane.stubs[".docx-body"];
    const page = body.children.find((kid) => kid.className === "docx-page");
    assert.ok(page?.shadow, "the drawing is in a shadow root");
    assert.equal(page.shadow.options.mode, "open");
    assert.equal(
      body.children.some((kid) => kid.className.includes("viewer-loading")),
      false,
    );
  } finally {
    delete globalThis.DOMPurify;
  }
  assert.equal(seen.path, DOCX_PREVIEW_PATH);
  assert.equal(seen.parse.renderAltChunks, false, "HTML stored in the docx is never drawn");
  assert.deepEqual(seen.render, DOCX_OPTIONS);
});

test("docx opens in the docx viewer; odt stays on the document viewer", () => {
  assert.equal(viewerKindOf("docx"), "docx");
  assert.equal(VIEWERS.docx.build, buildDocx);
  assert.equal(viewerKindOf("odt"), "document");
});
