// Unit tests for assets/ui/wb-viewer-panes.ts — the rules behind the file
// viewer's second pane: when a slot splits the canvas, where each pane goes,
// and which editor a focused slot hands the keyboard to.
import { test } from "node:test";
import assert from "node:assert/strict";
import { focusTarget, isSplit, paneLayout } from "../assets/ui/wb-viewer-panes.ts";

const EDITOR = { name: "editor" };

test("a pinned slot splits once its pane is open; a mirror needs a mounted code editor", () => {
  const pin = { id: "b", mirror: false, ratio: null };
  const mirror = { id: "a", mirror: true, ratio: null };
  // [case, slot, the slot's pane, split]
  const rows = [
    ["no slot", null, null, false],
    ["a pin whose pane is open", pin, { kind: "image" }, true],
    ["a pin whose pane is not open yet", pin, undefined, false],
    ["a mirror over a mounted code editor", mirror, { kind: "code", ed: EDITOR }, true],
    ["a mirror before the editor mounted", mirror, { kind: "code" }, false],
    ["a mirror over a pane that is not code", mirror, { kind: "markdown", ed: EDITOR }, false],
  ];
  for (const [name, slot, rec, split] of rows) assert.equal(isSplit(slot, rec), split, name);
});

test("the active pane goes to column 1 and the pinned one to column 3", () => {
  assert.deepEqual(paneLayout("a", "a", true, "b"), { on: true, display: "flex", gridColumn: "1" });
  assert.deepEqual(paneLayout("b", "a", true, "b"), { on: true, display: "flex", gridColumn: "3" });
  assert.deepEqual(paneLayout("c", "a", true, "b"), { on: false, display: "none", gridColumn: "" });
  // One pane alone has no grid column, and the slot's pane is hidden.
  assert.deepEqual(paneLayout("a", "a", false, "b"), { on: true, display: "flex", gridColumn: "" });
  assert.deepEqual(paneLayout("b", "a", false, "b"), { on: false, display: "none", gridColumn: "" });
  // Nothing on screen: every pane is hidden.
  assert.deepEqual(paneLayout("a", null, false, undefined), { on: false, display: "none", gridColumn: "" });
});

test("a focused mirror gets the mirror's editor; a focused pin gets its pane's editor", () => {
  const own = { name: "own" };
  const mirrored = { name: "mirror" };
  assert.equal(focusTarget({ id: "a", mirror: true, ratio: null }, { ed: mirrored }, own), mirrored);
  assert.equal(focusTarget({ id: "b", mirror: false, ratio: null }, { ed: mirrored }, own), own);
  // A mirror not built yet has no editor to focus.
  assert.equal(focusTarget({ id: "a", mirror: true, ratio: null }, null, own), undefined);
});
