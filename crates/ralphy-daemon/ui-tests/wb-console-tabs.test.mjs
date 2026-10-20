// A console in its own tab (ADR-0037 §3d): the store `tabs` keeps the tab, the
// console's maximize is this page's own, and the stored columns of a tab come
// back. The console module is a fake that records each `tabMax` call.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { createStageWindow } from "../assets/ui/wb-stage-window.ts";

// The store reaches `WBViewer` as a bare global: each test puts the real one back.
const realViewer = globalThis.WBViewer;
afterEach(() => {
  globalThis.WBViewer = realViewer;
});

// A page whose console module has the consoles `ids` on its stage.
function tabShell(ids, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  window.Alpine.nextTick = (fn) => fn();
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const maxes = [];
  const stage = { ids: new Set(ids), relaunching: new Set(), names: {} };
  window.WBConsole = {
    tabMax: (id, on) => {
      maxes.push([id, on]);
      return stage.ids.has(id);
    },
    list: () => [...stage.ids].map((id) => ({ id, name: stage.names[id] || "", agent: "codex" })),
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
  assert.equal(next.tabs.active, "console:w1", "the active tab that was dropped falls back to its neighbour");
});

test("a console tab takes the console's new name", () => {
  const { tabs, stage, stored } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  stage.names.w1 = "build";
  tabs.syncConsoleTabs();
  const title = tabs.tabs.find((t) => t.id === "console:w1").title;
  assert.notEqual(title, "codex");
  assert.match(title, /build/);
  assert.deepEqual(stored().consoleTabs, [{ deskId: "w1", title }]);
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

// A window with the classes `classes`, and the window states of a page where
// a tab holds it when `held`. `writes` records each desk write.
function stageWin(classes, held) {
  const set = new Set(classes);
  const classList = {
    contains: (c) => set.has(c),
    toggle: (c, on) => (on ? set.add(c) : set.delete(c), on),
  };
  const win = { _deskId: "w1", _maxBtn: { hidden: false, title: "", innerHTML: "" }, classList, style: { removeProperty() {} } };
  const writes = [];
  const sw = createStageWindow({
    document: {},
    workspace: () => null,
    stage: () => null,
    focusWin() {},
    setWin: (_w, fields) => writes.push(fields),
    heldByFence: () => false,
    refreshCover() {},
    heldByTab: () => held,
  });
  return { win, sw, writes };
}

function maxButton(classes, held) {
  const { win, sw } = stageWin(classes, held);
  sw.paintMaxButton(win);
  return win._maxBtn;
}

test("the × keeps a maximize the desk records and drops one only the tab made, writing nothing", () => {
  const kept = stageWin(["maximized"], false);
  kept.sw.tabMax(kept.win, false, true);
  assert.ok(kept.win.classList.contains("maximized"));
  const dropped = stageWin(["maximized"], false);
  dropped.sw.tabMax(dropped.win, false, false);
  assert.equal(dropped.win.classList.contains("maximized"), false);
  assert.deepEqual([...kept.writes, ...dropped.writes], []);
});

test("a double click on the title bar of a tab's console changes nothing", () => {
  const held = stageWin(["maximized"], true);
  held.sw.toggleMax(held.win);
  assert.ok(held.win.classList.contains("maximized"));
  assert.deepEqual(held.writes, []);
  const free = stageWin(["maximized"], false);
  free.sw.toggleMax(free.win);
  assert.deepEqual(free.writes, [{ max: false }], "outside a tab the toggle writes the desk");
});

test("the console of a console tab shows no Restore; a column in the tab keeps it", () => {
  assert.equal(maxButton(["maximized"], true).hidden, true);
  assert.equal(maxButton(["maximized"], false).hidden, false, "a maximize of the desk keeps Restore");
  const column = maxButton(["column"], true);
  assert.equal(column.hidden, false);
  assert.equal(column.title, "Restore");
});

// The columns of the console of a tab write no maximize to the desk.
function columnsPersist(held, act = (state) => state.restoreColumn("w3")) {
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
    act(state);
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

test("closing a console tab ends its columns and writes no maximize", () => {
  let cols = null;
  const writes = columnsPersist(true, (state) => {
    state.endTabColumns("w1");
    cols = state.columns;
  });
  assert.deepEqual(cols, []);
  assert.deepEqual(writes, [false]);
});

test("on a phone, Switch in a console tab opens the tab of the picked console", () => {
  let tabs = null;
  const writes = columnsPersist(true, (state) => {
    state.$store.tabs.activate("console:w1");
    state.columnFrom = "w1";
    state.swapColumn("w2");
    tabs = state.$store.tabs;
  });
  assert.equal(tabs.active, "console:w2");
  assert.deepEqual(writes, [], "no swap is painted");
});
