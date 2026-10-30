// Unit tests for assets/ui/wb-stage-window.ts — the console window's states.
// `createStageWindow` is driven with fake `deps` and fake windows, with no
// page: the workspace and the stage are absent, so only the window changes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createStageWindow } from "../assets/ui/wb-stage-window.ts";

// A window with a real class set and a style that records its custom properties.
function fakeWin(...classes) {
  const set = new Set(classes);
  const props = new Map([
    ["--max-left", "40px"],
    ["--max-top", "20px"],
  ]);
  return {
    props,
    classList: {
      contains: (c) => set.has(c),
      toggle: (c, on) => (on ? set.add(c) : set.delete(c), on),
    },
    style: { removeProperty: (p) => props.delete(p) },
  };
}

function stageWindow() {
  const written = [];
  const api = createStageWindow({
    document: {},
    workspace: () => null,
    stage: () => null,
    focusWin() {},
    setWin: (win, fields) => written.push(fields),
    heldByFence: () => false,
    refreshCover() {},
  });
  return { api, written };
}

test("a restored floating console drops the maximize pin, and a restored column keeps it", () => {
  const { api } = stageWindow();
  const floating = fakeWin("maximized");
  api.setMax(floating, false);
  assert.deepEqual([...floating.props.keys()], []);
  const column = fakeWin("maximized", "column");
  api.setMax(column, false);
  assert.deepEqual([...column.props.keys()], ["--max-left", "--max-top"]);
});

test("a maximize keeps the pin, and only a persisted toggle writes the desk", () => {
  const { api, written } = stageWindow();
  const win = fakeWin();
  api.setMax(win, true);
  assert.deepEqual([...win.props.keys()], ["--max-left", "--max-top"]);
  assert.deepEqual(written, []);
  api.setMax(win, false, true);
  assert.deepEqual(written, [{ max: false }]);
});
