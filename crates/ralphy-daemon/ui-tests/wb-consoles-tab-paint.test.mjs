// Unit tests for `paintColumns` in assets/ui/wb-consoles-tab.ts: what it does
// before it paints (a head that must be restored) and when the paint raises
// the windows. They are here, not in wb-consoles-tab.test.mjs, so a change of
// the tab's markup and a change of this method never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// `paintColumns` on a stage of `wins` (`[id, maximized, column]`). Every paint
// is recorded as `[ids, options]`, every restore by id.
function paintShell(wins) {
  const { scope: state, shell, document, window } = loadComponent("wbColumns");
  const paints = [];
  const restored = [];
  const real = globalThis.WBConsole;
  globalThis.WBConsole = {
    ...window.WBConsole,
    applyColumns: (painted, opts) => paints.push([painted.map((p) => p.id), opts]),
    columnMeasure: () => ({ viewport: 1920 }),
    focusedId: () => null,
    focusColumn() {},
    isRelaunching: () => false,
  };
  state.columnCap = () => 3;
  state.restoreColumn = (id) => restored.push(id);
  shell.$store.tabs.active = "code";
  document.querySelectorAll = () =>
    wins.map(([id, max, column]) => ({
      _deskId: id,
      classList: { contains: (c) => (c === "maximized" && max) || (c === "column" && column) },
    }));
  document.querySelector = () => null;
  return { state, paints, restored, done: () => (globalThis.WBConsole = real) };
}

test("a first column that is neither maximized nor a column is restored and nothing is painted", () => {
  const t = paintShell([["a"], ["b", true]]);
  try {
    t.state.columns = [["a"], ["b"]];
    t.state.paintColumns();
    assert.deepEqual(t.restored, ["a"]);
    assert.deepEqual(t.paints, []);
  } finally {
    t.done();
  }
});

test("a first column that is a column or maximized is painted, not restored", () => {
  for (const win of [["a", true], ["a", false, true]]) {
    const t = paintShell([win, ["b", false, true]]);
    try {
      t.state.columns = [["a"], ["b"]];
      t.state.paintColumns();
      assert.deepEqual(t.restored, []);
      assert.deepEqual(t.paints.map(([ids]) => ids), [["a", "b"]]);
    } finally {
      t.done();
    }
  }
});

test("a paint raises the windows when asked or when the painted columns changed", () => {
  const t = paintShell([["a", true], ["b", false, true]]);
  try {
    t.state.columns = [["a"], ["b"]];
    const raises = [];
    for (const opts of [undefined, undefined, { raise: true }, { raise: false }]) {
      t.state.paintColumns(opts);
      raises.push(t.paints.at(-1)[1].raise);
    }
    assert.deepEqual(raises, [true, false, true, false], "the first paint changed the columns; the second did not");
  } finally {
    t.done();
  }
});
