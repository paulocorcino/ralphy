// A console in its own tab (ADR-0037 §3d): the store `tabs` keeps the tab, the
// console's maximize is this page's own, and the stored columns of a tab come
// back. The console module is a fake that records each `tabMax` call.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { createStageWindow } from "../assets/ui/wb-stage-window.ts";

// A page whose console module has the consoles `ids` on its stage.
function tabShell(ids, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  window.Alpine.nextTick = (fn) => fn();
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const maxes = [];
  const stage = { ids: new Set(ids), relaunching: new Set() };
  window.WBConsole = {
    tabMax: (id, on) => {
      maxes.push([id, on]);
      return stage.ids.has(id);
    },
    list: () => [...stage.ids].map((id) => ({ id, name: "", agent: "codex" })),
    isRelaunching: (id) => stage.relaunching.has(id),
    refitAll() {},
  };
  return { tabs: state.$store.tabs, maxes, stage, stored: () => stored };
}

test("a console opens in one tab, maximized on this page; a second pick only activates it", () => {
  const { tabs, maxes } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.activate("consoles");
  tabs.openConsoleTab("w1", "codex");
  assert.deepEqual(
    tabs.tabs.map((t) => t.id),
    ["consoles", "console:w1"],
  );
  assert.equal(tabs.active, "console:w1");
  assert.ok(tabs.stageShown(), "the console tab shows the stage");
  assert.ok(tabs.holds("w1"));
  assert.deepEqual(maxes, [["w1", true], ["w1", true]]);
});

test("the × of a console tab gives the maximize back and leaves the console on the stage", () => {
  const { tabs, maxes } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  maxes.length = 0;
  tabs.closeTab("console:w1");
  assert.deepEqual(maxes, [["w1", false]]);
  assert.equal(tabs.holds("w1"), false);
  assert.equal(tabs.active, "consoles");
});

test("a console tab is stored per device and a tab whose console is gone is dropped", () => {
  const first = tabShell(["w1", "w2"]);
  first.tabs.openConsoleTab("w1", "codex");
  first.tabs.openConsoleTab("w2", "claude");
  assert.deepEqual(first.stored().consoleTabs, [
    { deskId: "w1", title: "codex" },
    { deskId: "w2", title: "claude" },
  ]);
  assert.equal(first.stored().active, "console:w2");

  // The next load: w2 left the stage.
  const next = tabShell(["w1"], first.stored());
  next.tabs.restoreView();
  assert.equal(next.tabs.active, "console:w2", "the stored tab comes back before the desk");
  next.tabs.syncConsoleTabs();
  assert.deepEqual(
    next.tabs.tabs.map((t) => t.id),
    ["consoles", "console:w1"],
  );
  assert.ok(next.maxes.some(([id, on]) => id === "w1" && on), "the kept tab maximizes its console");
});

test("a console that relaunches keeps its tab", () => {
  const { tabs, stage } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  stage.ids.delete("w1");
  stage.relaunching.add("w1");
  tabs.syncConsoleTabs();
  assert.ok(tabs.holds("w1"));
});

test("the stored columns of a console tab come back without a maximize on the desk", () => {
  const desk = [
    { id: "w1", max: false },
    { id: "w2", max: false },
  ];
  assert.deepEqual(WBColumns.fromStored([["w1"], ["w2"]], desk), [], "no maximize, no tab: ignored");
  assert.deepEqual(WBColumns.fromStored([["w1"], ["w2"]], desk, ["w1"]), [["w1"], ["w2"]]);
  assert.deepEqual(WBColumns.fromStored([["w1"], ["w2"]], desk, ["w2"]), [], "the tab's console must be first");
});

// A window with the classes `classes`, held by a tab when `held`.
function maxButton(classes, held) {
  const win = { _deskId: "w1", _maxBtn: { hidden: false, title: "", innerHTML: "" }, classList: new Set(classes) };
  win.classList.contains = win.classList.has;
  const sw = createStageWindow({
    document: {},
    workspace: () => null,
    stage: () => null,
    focusWin() {},
    setWin() {},
    heldByFence: () => false,
    refreshCover() {},
    heldByTab: () => held,
  });
  sw.paintMaxButton(win);
  return win._maxBtn;
}

test("the console of a console tab shows no Restore; a column in the tab keeps it", () => {
  assert.equal(maxButton(["maximized"], true).hidden, true);
  assert.equal(maxButton(["maximized"], false).hidden, false, "a maximize of the desk keeps Restore");
  const column = maxButton(["column"], true);
  assert.equal(column.hidden, false);
  assert.equal(column.title, "Restore");
});

// The columns of the console of a tab write no maximize to the desk.
function columnsPersist(held) {
  const { scope: state } = loadComponent("wbColumns");
  const realConsole = globalThis.WBConsole;
  const writes = [];
  globalThis.WBConsole = {
    ...realConsole,
    applyColumns: (_painted, opts) => writes.push(opts.persist),
    columnMeasure: () => ({ viewport: 1920 }),
    focusedId: () => null,
    focusColumn() {},
    isRelaunching: () => false,
  };
  try {
    globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
    if (held) state.$store.tabs.openConsoleTab("w1", "w1");
    state.columnCap = () => 3;
    state.columns = [["w1"], ["w2"], ["w3"]];
    state.restoreColumn("w3");
    return writes;
  } finally {
    globalThis.WBConsole = realConsole;
  }
}

test("a column restore in a console tab writes no maximize to the desk", () => {
  assert.ok(columnsPersist(false).every(Boolean), "on the stage the maximize is the desk's");
  const held = columnsPersist(true);
  assert.ok(held.length > 0);
  assert.ok(held.every((p) => p === false));
});
