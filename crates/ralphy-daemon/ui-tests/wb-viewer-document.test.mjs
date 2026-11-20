// Unit tests for assets/ui/wb-viewer-document.ts, the document pane: text
// from the file stays text, a merged cell spans its columns, and only an
// embedded image of an image type draws, from a `blob:` address.
import { test } from "node:test";
import assert from "node:assert/strict";
import { renderDocument } from "../assets/ui/wb-viewer-document.ts";
import { viewerKindOf } from "../assets/ui/wb-viewers.ts";

function node(tag, text) {
  const el = {
    tag,
    attrs: {},
    children: [],
    text,
    append(...kids) {
      el.children.push(...kids);
    },
    setAttribute(name, value) {
      el.attrs[name] = String(value);
    },
    addEventListener() {},
  };
  return el;
}

// A document that records each element it makes, so a test can ask whether
// a tag was ever made.
function fakeDocument() {
  const made = [];
  return {
    made,
    createElement(tag) {
      const el = node(tag, "");
      made.push(el);
      return el;
    },
    createTextNode: (text) => node("#text", text),
  };
}

function textOf(nodes) {
  return nodes.map((n) => (n.tag === "#text" ? n.text : textOf(n.children))).join("");
}

function find(nodes, tag) {
  for (const n of nodes) {
    if (n.tag === tag) return n;
    const inner = find(n.children, tag);
    if (inner) return inner;
  }
  return null;
}

const plain = { bold: false, italic: false, strike: false, code: false };
const words = (text) => ({ kind: "text", text, style: plain });
const para = (text) => ({ kind: "paragraph", content: [words(text)] });
const model = (blocks, assets = []) => ({ blocks, notes: [], assets });

test("a paragraph holding <script> text shows as text and adds no script element", () => {
  const document = fakeDocument();
  const nodes = renderDocument(document, model([para("<script>alert(1)</script>")]));
  assert.equal(textOf(nodes), "<script>alert(1)</script>");
  assert.ok(!document.made.some((el) => el.tag === "script"), "no script element is made");
});


// One cell that starts at its slot and spans `colSpan` columns.
const origin = (text, colSpan = 1) => ({
  kind: "origin",
  cell: { blocks: [para(text)], colSpan, rowSpan: 1 },
});

test("a table with a merged cell draws it with colspan", () => {
  const covered = { kind: "covered", originRow: 0, originCol: 0 };
  const table = {
    kind: "table",
    table: {
      grid: [
        [origin("wide", 2), covered],
        [origin("a"), origin("b")],
      ],
      headerRows: 0,
    },
  };
  const nodes = renderDocument(fakeDocument(), model([table]));
  const rows = find(nodes, "tbody").children;
  assert.equal(rows[0].children.length, 1);
  assert.equal(rows[0].children[0].attrs.colspan, "2");
  assert.equal(rows[1].children.length, 2);
});

const pictured = (mediaType) =>
  model(
    [
      {
        kind: "paragraph",
        content: [{ kind: "image", alt: "a dot", source: { kind: "asset", assetId: 0 } }],
      },
    ],
    [{ id: 0, mediaType, originPart: "word/media/image1", data: new Uint8Array([1, 2, 3]) }],
  );

test("an image asset of type image/png becomes an img with a blob: address", () => {
  const img = find(renderDocument(fakeDocument(), pictured("image/png")), "img");
  assert.ok(img, "an img is drawn");
  assert.match(img.attrs.src, /^blob:/u);
  assert.equal(img.attrs.alt, "a dot");
});

test("an asset of a non-image type draws nothing", () => {
  const document = fakeDocument();
  renderDocument(document, pictured("text/html"));
  assert.ok(!document.made.some((el) => el.tag === "img"), "no img is drawn");
});

test("docx and odt files open in the document viewer", () => {
  assert.equal(viewerKindOf("docx"), "document");
  assert.equal(viewerKindOf("odt"), "document");
});
