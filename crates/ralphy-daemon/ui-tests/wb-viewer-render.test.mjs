// Unit tests for assets/ui/wb-viewer-render.ts — the rendered body of the
// markdown pane: which file kinds that pane shows, and the anchor of a
// heading that a `#fragment` link names.
import { test } from "node:test";
import assert from "node:assert/strict";
import { isRendered, slugOf } from "../assets/ui/wb-viewer-render.ts";

test("isRendered names the kinds the rendered pane shows", () => {
  // [kind, shown by the rendered pane]
  const rows = [
    ["markdown", true],
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
