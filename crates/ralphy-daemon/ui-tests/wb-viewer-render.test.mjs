// Unit tests for assets/ui/wb-viewer-render.ts — the rendered body of the
// markdown pane: which file kinds that pane shows, and the anchor of a
// heading that a `#fragment` link names.
import { test } from "node:test";
import assert from "node:assert/strict";
import { slugOf } from "../assets/ui/wb-dbml-draw.ts";
import { isRendered, renderBody } from "../assets/ui/wb-viewer-render.ts";

test("isRendered names the kinds the rendered pane shows", () => {
  // [kind, shown by the rendered pane]
  const rows = [
    ["markdown", true],
    ["mermaid", true],
    ["dbml", true],
    ["code", false],
    ["image", false],
    ["diff", false],
    ["note", false],
    ["binary", false],
  ];
  for (const [kind, want] of rows) assert.equal(isRendered(kind), want, kind);
});

test("slugOf makes the anchor of a heading", () => {
  // [heading text, anchor]
  const rows = [
    ["Hello, World!", "hello-world"],
    ["  Fase 2 — Ação  ", "fase-2-ação"],
    ["Already-dashed words", "already-dashed-words"],
  ];
  for (const [text, want] of rows) assert.equal(slugOf(text), want, text);
});

// A `.mmd` file is one diagram: the article holds a single holder whose
// source is the whole file, drawn later when the pane is on screen.
test("a mermaid file is one diagram holder with the whole file as its source", () => {
  const appended = [];
  const article = { innerHTML: "<p>old</p>", append: (el) => appended.push(el) };
  const doc = { createElement: (tag) => ({ tag, className: "", id: "", dataset: {} }) };
  const source = "flowchart TD\n  A[Start] --> B[Finish]\n";
  const pending = renderBody("mermaid", article, source, 7, doc);
  assert.equal(article.innerHTML, "");
  assert.equal(appended.length, 1);
  assert.deepEqual(pending, appended);
  assert.deepEqual(pending[0], { tag: "div", className: "mermaid", id: "mmd-7-0", dataset: { src: source } });
});
