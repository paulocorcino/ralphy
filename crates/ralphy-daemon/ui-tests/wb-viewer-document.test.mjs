// Unit tests for the odt pane: the style rules of wb-odt-style.ts (only
// checked values become CSS) and the messages of wb-viewer-document.ts. The
// DOM walk of wb-odt.ts needs a real DOMParser; the browser check in
// tests/browser/files/wb_viewer_wait_and_controls.py draws a real odt.
import { test } from "node:test";
import assert from "node:assert/strict";
import { FORMAT_RULES, familyOf, resolveCss, underlineOf } from "../assets/ui/wb-odt-style.ts";
import {
  buildDocument,
  DOCUMENT_TOO_LARGE_TEXT,
} from "../assets/ui/wb-viewer-document.ts";
import { viewerKindOf } from "../assets/ui/wb-viewers.ts";

test("a style value passes only when it matches its rule", () => {
  assert.equal(FORMAT_RULES["font-weight"].value("bold"), "bold");
  assert.equal(FORMAT_RULES["font-weight"].value("700; color: red"), null);
  assert.equal(FORMAT_RULES["text-align"].value("start"), "left");
  assert.equal(FORMAT_RULES["text-align"].value("expression(x)"), null);
  assert.equal(FORMAT_RULES["font-size"].value("12pt"), "12pt");
  assert.equal(FORMAT_RULES["margin-left"].value("url(https://x.test)"), null);
  assert.equal(FORMAT_RULES["background-color"].value("#ffff00"), "#ffff00");
  assert.equal(FORMAT_RULES["color"].value("red;background:url(x)"), null);
});

test("a font family is a plain name, quoted, with a fallback", () => {
  assert.equal(familyOf("'Liberation Serif'"), '"Liberation Serif", sans-serif');
  assert.equal(familyOf("Aptos"), '"Aptos", sans-serif');
  assert.equal(familyOf('x"; } body { color: red'), null);
  assert.equal(familyOf(null), null);
});

test("an underline style other than none underlines", () => {
  assert.equal(underlineOf("solid"), "underline");
  assert.equal(underlineOf("none"), null);
  assert.equal(underlineOf(null), null);
});

test("a style takes its parent's CSS under its own, and a loop ends", () => {
  const styles = new Map([
    ["child", { css: { color: "#000001" }, parent: "parent" }],
    ["parent", { css: { color: "#000002", "font-weight": "bold" }, parent: null }],
    ["loop", { css: { "font-style": "italic" }, parent: "loop" }],
  ]);
  assert.deepEqual(resolveCss(styles, "child"), { color: "#000001", "font-weight": "bold" });
  assert.deepEqual(resolveCss(styles, "loop"), { "font-style": "italic" });
  assert.deepEqual(resolveCss(styles, null), {});
});

function fakeEl(tag) {
  const stubs = new Map();
  const el = {
    tag,
    style: {},
    dataset: {},
    className: "",
    textContent: "",
    innerHTML: "",
    children: [],
    append(...kids) {
      for (const kid of kids) kid.parent = el;
      el.children.push(...kids);
    },
    remove() {
      el.parent.children = el.parent.children.filter((kid) => kid !== el);
    },
    addEventListener() {},
    querySelector(sel) {
      if (!stubs.has(sel)) stubs.set(sel, fakeEl(sel));
      return stubs.get(sel);
    },
  };
  return el;
}

test("an odt over its cap shows the too large message", async () => {
  const mount = fakeEl("viewers");
  const host = {
    document: { createElement: (tag) => fakeEl(tag) },
    viewers: mount,
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const deps = {
    fetchBytes: () => Promise.resolve("too large"),
    load: () => Promise.resolve({ ok: true, module: { read() {}, utils: {}, CFB: {} } }),
  };
  buildDocument(host, { id: "t", project: "p", path: "a.odt", checkout: null }, deps);
  await new Promise((resolve) => {
    setImmediate(resolve);
  });
  assert.equal(mount.children[0].querySelector(".document-meta").textContent, DOCUMENT_TOO_LARGE_TEXT);
});

test("odt files open in the document viewer", () => {
  assert.equal(viewerKindOf("odt"), "document");
});
