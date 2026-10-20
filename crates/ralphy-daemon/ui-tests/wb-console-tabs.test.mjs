// Consoles in their own tab (ADR-0037 §3d): the store `tabs` keeps the tab
// and its grid, the console module moves the windows into the tab's pane and
// back, and the stage's rules leave a window in a tab alone.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { createPlace } from "../assets/ui/wb-console-place.ts";
import { createStageWindow } from "../assets/ui/wb-stage-window.ts";
import { createDormancy } from "../assets/ui/wb-console-dormancy.ts";
import { createStack } from "../assets/ui/wb-stage-stack.ts";
import { wirePress } from "../assets/ui/wb-console-press.ts";

// The store reaches `WBViewer` as a bare global: each test puts the real one back.
const realViewer = globalThis.WBViewer;
afterEach(() => {
  globalThis.WBViewer = realViewer;
});

// A page whose console module has the consoles `ids`. `places` records each
// `placeTabs` call as `[held, grid of the active tab]`.
function tabShell(ids, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  window.Alpine.nextTick = (fn) => fn();
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const places = [];
  const page = { ids: new Set(ids), relaunching: new Set(), names: {} };
  window.WBConsole = {
    placeTabs: (held, grid) => places.push([[...held], grid && grid.map((c) => [...c])]),
    list: () => [...page.ids].map((id) => ({ id, name: page.names[id] || "", agent: "codex" })),
    isRelaunching: (id) => page.relaunching.has(id),
    refitAll() {},
  };
  return { tabs: state.$store.tabs, places, page, stored: () => stored, last: () => places[places.length - 1] };
}

test("a console opens in one tab with its own pane; a second pick only activates it", () => {
  const { tabs, last } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.activate("consoles");
  tabs.openConsoleTab("w1", "codex");
  assert.deepEqual(
    tabs.tabs.map((t) => t.id),
    ["consoles", "console:w1"],
  );
  assert.equal(tabs.active, "console:w1");
  assert.equal(tabs.stageShown(), false, "a console tab does not show the stage");
  assert.ok(tabs.consoleTabShown());
  assert.deepEqual(last(), [["w1"], [["w1"]]]);
});

test("another tab hides the console tab's window and keeps it in the pane", () => {
  const { tabs, last } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.activate("consoles");
  assert.ok(tabs.stageShown());
  assert.deepEqual(last(), [["w1"], null]);
});

test("the × of a console tab puts its window back on the stage", () => {
  const { tabs, places, last } = tabShell(["w1", "w2"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.openConsoleTab("w2", "claude");
  places.length = 0;
  tabs.closeTab("console:w2");
  assert.deepEqual(places[0], [["w1"], null], "the window leaves the pane before the next tab shows");
  assert.deepEqual(last(), [["w1"], [["w1"]]]);
  // A tab in the background: no other tab shows, and its window still goes back.
  tabs.activate("consoles");
  places.length = 0;
  tabs.closeTab("console:w1");
  assert.deepEqual(places, [[[], null]]);
  assert.equal(tabs.holds("w1"), false);
  assert.equal(tabs.active, "consoles");
});

test("a console tab is stored per device and a tab whose console is gone is dropped", () => {
  const first = tabShell(["w1", "w2"]);
  first.tabs.openConsoleTab("w1", "codex");
  first.tabs.openConsoleTab("w2", "claude");
  assert.deepEqual(first.stored().consoleTabs, [
    { id: "console:w1", deskId: "w1", title: "codex", grid: [["w1"]] },
    { id: "console:w2", deskId: "w2", title: "claude", grid: [["w2"]] },
  ]);
  assert.equal(first.stored().active, "console:w2");

  // The next load: w2 left the page.
  const next = tabShell(["w1"], first.stored());
  next.tabs.restoreView();
  assert.equal(next.tabs.active, "console:w2", "the stored tab comes back before the desk");
  next.tabs.syncConsoleTabs();
  assert.deepEqual(
    next.tabs.tabs.map((t) => t.id),
    ["consoles", "console:w1"],
  );
  assert.equal(next.tabs.active, "console:w1", "the active tab that was dropped falls back to its neighbour");
  assert.deepEqual(next.last(), [["w1"], [["w1"]]], "the kept tab holds its window");
});

test("a console tab stored before the grid comes back under the id it had", () => {
  // `WBView.read` gives such a tab a null id and a grid of its one console.
  const old = tabShell(["w1"], {
    consoleTabs: [{ id: null, deskId: "w1", title: "codex", grid: [["w1"]] }],
    active: "console:w1",
  });
  old.tabs.restoreView();
  assert.equal(old.tabs.active, "console:w1");
  assert.deepEqual(old.tabs.tabOf("w1").grid, [["w1"]]);
});

test("a console tab takes the new name of the console it opened for, and of no other", () => {
  const { tabs, page, stored } = tabShell(["w1", "w2"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.setTabGrid("console:w1", [["w1"], ["w2"]]);
  tabs.syncConsoleTabs();
  const before = tabs.tabOf("w1").title;
  page.names.w2 = "tests";
  tabs.syncConsoleTabs();
  assert.equal(tabs.tabOf("w1").title, before, "another console of the tab does not name it");
  page.names.w1 = "build";
  tabs.syncConsoleTabs();
  const title = tabs.tabOf("w1").title;
  assert.match(title, /build/);
  assert.equal(stored().consoleTabs[0].title, title);
  tabs.leaveTab("w1");
  page.names.w1 = "later";
  tabs.syncConsoleTabs();
  assert.equal(tabs.tabOf("w2").title, title, "the name stays once its console left the tab");
});

test("a console that relaunches keeps its tab", () => {
  const { tabs, page } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  page.ids.delete("w1");
  page.relaunching.add("w1");
  tabs.syncConsoleTabs();
  assert.ok(tabs.holds("w1"));
});

test("a console tab holds a grid: a console that leaves the page leaves the grid, and an empty tab closes", () => {
  const { tabs, page, stored, last } = tabShell(["w1", "w2", "w3"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.setTabGrid("console:w1", [["w1", "w2"], ["w3"]]);
  assert.deepEqual(last(), [["w1", "w2", "w3"], [["w1", "w2"], ["w3"]]]);
  assert.deepEqual(stored().consoleTabs[0].grid, [["w1", "w2"], ["w3"]]);
  assert.equal(tabs.tabOf("w3").id, "console:w1");
  page.ids.delete("w3");
  tabs.syncConsoleTabs();
  assert.deepEqual(tabs.tabOf("w1").grid, [["w1", "w2"]]);
  assert.deepEqual(last(), [["w1", "w2"], [["w1", "w2"]]]);
  page.ids.clear();
  tabs.syncConsoleTabs();
  assert.deepEqual(
    tabs.tabs.map((t) => t.id),
    ["consoles"],
  );
});

test("Restore in a tab takes the console back to the stage; the tab keeps its id and its name", () => {
  const { tabs, last } = tabShell(["w1", "w2"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.setTabGrid("console:w1", [["w1"], ["w2"]]);
  tabs.leaveTab("w1");
  assert.equal(tabs.holds("w1"), false);
  const tab = tabs.tabOf("w2");
  assert.equal(tab.id, "console:w1");
  assert.equal(tab.title, "codex");
  assert.deepEqual(last(), [["w2"], [["w2"]]]);
  // A new tab for the console that left takes the next free id.
  tabs.openConsoleTab("w1", "codex");
  assert.equal(tabs.tabOf("w1").id, "console:w1-2");
  assert.equal(tabs.active, "console:w1-2");
});

// A small DOM: a parent keeps its children, and `append` moves a child.
function parent(name) {
  const el = {
    name,
    children: [],
    append(child) {
      const from = child.parentElement;
      if (from) from.children.splice(from.children.indexOf(child), 1);
      el.children.push(child);
      child.parentElement = el;
    },
  };
  return el;
}

function consoleWin(id, classes = []) {
  const set = new Set(classes);
  const vars = {};
  return {
    _deskId: id,
    parentElement: null,
    dataset: {},
    vars,
    style: {
      left: "40px",
      top: "50px",
      width: "600px",
      height: "400px",
      setProperty: (k, v) => (vars[k] = v),
      removeProperty: (k) => delete vars[k],
    },
    classList: {
      contains: (c) => set.has(c),
      toggle: (c, on) => (on ? set.add(c) : set.delete(c), on),
    },
  };
}

const rectOf = (w) => ({ left: w.style.left, top: w.style.top, width: w.style.width, height: w.style.height });

// The stage with the windows `ids`, and the pane of the console tabs.
function placePage(ids, paneWidth = 1280) {
  const stage = parent("stage");
  const pane = parent("pane");
  pane.clientWidth = paneWidth;
  const wins = ids.map((id) => consoleWin(id));
  for (const w of wins) stage.append(w);
  const sent = [];
  const focused = [];
  const moved = [];
  const place = createPlace({
    document: { dispatchEvent: (e) => sent.push([e.type, e.detail]) },
    stage: () => stage,
    tabsPane: () => pane,
    pageWindows: () => [...stage.children, ...pane.children],
    focusWin: (w) => focused.push(w._deskId),
    moved: (w) => moved.push(w._deskId),
  });
  const byId = Object.fromEntries(wins.map((w) => [w._deskId, w]));
  return { place, stage, pane, byId, sent, focused, moved };
}

test("a console's window moves into the tab's pane and back, with its rect and its classes", () => {
  const p = placePage(["w1", "w2"]);
  const w1 = p.byId.w1;
  w1.classList.toggle("maximized", true);
  p.place.placeTabs(["w1"], [["w1"]]);
  assert.equal(w1.parentElement, p.pane);
  assert.equal(p.byId.w2.parentElement, p.stage);
  assert.ok(w1.classList.contains("in-tab") && w1.classList.contains("tab-shown"));
  assert.deepEqual(p.focused, ["w1"]);
  assert.deepEqual(p.moved, ["w1"]);
  assert.deepEqual(p.sent, [["workbench:columns-leave", { ids: ["w1"] }]], "a column leaves the grid");

  p.moved.length = 0;
  p.place.placeTabs([], null);
  assert.equal(w1.parentElement, p.stage);
  assert.equal(w1.classList.contains("in-tab"), false);
  assert.equal(w1.classList.contains("tab-shown"), false);
  assert.ok(w1.classList.contains("maximized"), "the desk's maximize is the window's again");
  assert.deepEqual(rectOf(w1), { left: "40px", top: "50px", width: "600px", height: "400px" });
  assert.deepEqual(w1.vars, {}, "no box of the tab is left on the stage");
  assert.deepEqual(p.moved, ["w1"]);
  assert.equal(p.sent.length, 1, "going back to the stage sends nothing");
});

test("only the active tab's window is shown, and a second call moves nothing", () => {
  const p = placePage(["w1", "w2", "w3"]);
  p.place.placeTabs(["w1", "w2", "w9"], [["w2"]]);
  assert.equal(p.byId.w1.classList.contains("tab-shown"), false);
  assert.ok(p.byId.w2.classList.contains("tab-shown"));
  assert.equal(p.byId.w3.parentElement, p.stage);
  p.moved.length = 0;
  p.place.placeTabs(["w1", "w2"], [["w2"]]);
  assert.deepEqual(p.moved, []);
  p.place.placeTabs(["w1", "w2"], [["w1"]]);
  assert.deepEqual(p.moved, ["w1", "w2"], "the shown window changes, nothing moves");
  assert.equal(p.sent.length, 1);
});

test("a tab's grid gives each window its box, and a phone width paints only the first", () => {
  const p = placePage(["w1", "w2", "w3"]);
  p.place.placeTabs(["w1", "w2", "w3"], [["w1"], ["w2", "w3"]]);
  const { w1, w2, w3 } = p.byId;
  assert.deepEqual(w1.vars, {
    "--tab-col-index": "0",
    "--tab-col-count": "2",
    "--tab-row-index": "0",
    "--tab-row-count": "1",
  });
  assert.deepEqual(w3.vars, {
    "--tab-col-index": "1",
    "--tab-col-count": "2",
    "--tab-row-index": "1",
    "--tab-row-count": "2",
  });
  assert.ok([w1, w2, w3].every((w) => w.classList.contains("tab-shown") && w.classList.contains("tab-grid")));
  assert.deepEqual(p.focused, ["w1"], "the first console of the tab takes the focus mark");

  const phone = placePage(["w1", "w2"], 400);
  phone.place.placeTabs(["w1", "w2"], [["w1"], ["w2"]]);
  assert.ok(phone.byId.w1.classList.contains("tab-shown"));
  assert.equal(phone.byId.w2.classList.contains("tab-shown"), false);
  assert.equal(phone.byId.w1.vars["--tab-col-count"], "1");
  assert.ok(phone.byId.w1.classList.contains("tab-grid"), "Restore still shows: the tab holds two consoles");

  const lone = placePage(["w1"]);
  lone.place.placeTabs(["w1"], [["w1"]]);
  assert.equal(lone.byId.w1.classList.contains("tab-grid"), false, "a tab of one console has no Restore");
});

// A window that measures, with the classes `classes`.
function measuredWin(classes) {
  const set = new Set(classes);
  return {
    offsetLeft: 0,
    offsetTop: 0,
    offsetWidth: 1280,
    offsetHeight: 700,
    style: { left: "40px", top: "50px", width: "600px", height: "400px" },
    classList: { contains: (c) => set.has(c) },
  };
}

test("the rect of a console in its tab is its stage rect, never the pane's box", () => {
  const sw = createStageWindow({
    document: { fullscreenElement: null },
    workspace: () => null,
    stage: () => null,
    focusWin() {},
    setWin() {},
    heldByFence: () => false,
    refreshCover() {},
  });
  assert.deepEqual(sw.restoreRect(measuredWin(["in-tab"])), { left: 40, top: 50, width: 600, height: 400 });
  assert.deepEqual(sw.restoreRect(measuredWin([])), { left: 0, top: 0, width: 1280, height: 700 });
});

// The dormancy of the windows `wins`, each `[classes, visible]`.
function dormancyOf(wins) {
  const els = wins.map(([classes, visible]) => {
    const set = new Set(classes);
    return { _visible: visible, classList: { contains: (c) => set.has(c) } };
  });
  const d = createDormancy({
    window: { addEventListener() {} },
    document: { addEventListener() {} },
    wins: new Set(els),
    stack: { setFocusHook() {} },
    sync: { hasPending: () => false },
    staleProbeNow: () => null,
    applyKeyBar() {},
    budget: () => ({ scheduleGpu() {} }),
    fillsViewport: (w) => w.classList.contains("maximized"),
    isFull: () => false,
    windowShown: () => true,
    attachTerminal: () => null,
    scheduleDeskFlush() {},
    revivePlaceholders: async () => {},
  });
  return { d, els };
}

test("a console in its tab covers no window of the stage, and none covers it", () => {
  const { d, els } = dormancyOf([
    [["in-tab", "tab-shown", "maximized"], true],
    [[], true],
    [["maximized"], true],
  ]);
  const [tabbed, floating] = els;
  assert.equal(d.isCovered(tabbed), false, "the stage's maximize does not cover the tab");
  assert.equal(d.isCovered(floating), true, "the stage's own maximize still covers");
  const alone = dormancyOf([
    [["in-tab", "tab-shown", "maximized"], true],
    [[], true],
  ]);
  assert.equal(alone.d.isCovered(alone.els[1]), false, "the tab's window covers nothing on the stage");
});

test("the console of the active tab stays awake though the stage's observer calls it outside", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { d, els } = dormancyOf([[["in-tab", "tab-shown"], false]]);
  const [win] = els;
  win._term = { sessionId: 7 };
  assert.equal(d.applyDormancy(win), "hold");
  t.mock.timers.tick(60000);
  assert.notEqual(win._dormant, true);
});

test("a console in its own tab cannot be opened as a column, and its row says why", () => {
  const groups = WBColumns.listFold({
    rows: [
      { id: "w1", agent: "codex", kind: "agent" },
      { id: "w2", agent: "claude", kind: "agent" },
    ],
    columns: [],
    from: null,
    tabbed: ["w2"],
  });
  const [w1, w2] = groups[0].rows;
  assert.equal(w1.enabled, true);
  assert.equal(w2.enabled, false);
  assert.equal(w2.swappable, false, "Switch does not take it out of its tab");
  assert.equal(w2.reason, WBColumns.REASON_TAB);
});

test("the + and Go-to show the tab of a console that has one", () => {
  const { scope: menu, shell } = loadComponent("wbConsoleTabMenu");
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const tabs = shell.$store.tabs;
  const realConsole = globalThis.WBConsole;
  globalThis.WBConsole = { ...realConsole, placeTabs: () => [], refitAll() {} };
  try {
    tabs.openConsoleTab("w2", "claude");
    tabs.setTabGrid("console:w2", [["w2"], ["w3"]]);
    tabs.activate("consoles");
    const row = { id: "w3", enabled: false, reason: WBColumns.REASON_TAB };
    assert.equal(menu.rowOff(row), false);
    menu.pick(row);
    assert.equal(tabs.active, "console:w2");
    assert.deepEqual(
      tabs.tabs.map((t) => t.id),
      ["consoles", "console:w2"],
    );

    tabs.activate("consoles");
    const { scope: menus } = loadComponent("wbConsoleMenus", {
      from: { state: shell, window: globalThis.window, document: globalThis.document },
    });
    menus.revealWindow("w3");
    assert.equal(tabs.active, "console:w2");
  } finally {
    globalThis.WBConsole = realConsole;
  }
});

// A document whose `#workspace` and `#console-tabs-pane` answer the selector
// of the focus mark.
function stackPage(onStage, inPane) {
  const matches = (el, sel) =>
    sel.split(",").some((p) =>
      p
        .trim()
        .split(".")
        .filter(Boolean)
        .every((c) => el.classList.contains(c)),
    );
  const root = (els) => ({ querySelectorAll: (sel) => els.filter((el) => matches(el, sel)) });
  const ids = { workspace: root(onStage), "console-tabs-pane": root(inPane) };
  return { getElementById: (id) => ids[id] ?? null };
}

function surface() {
  const set = new Set(["session-window"]);
  return {
    style: { zIndex: "" },
    classList: { add: (c) => set.add(c), remove: (c) => set.delete(c), contains: (c) => set.has(c) },
  };
}

test("one focus mark on the page: focusing a window clears the mark of a window in a tab", () => {
  const onStage = surface();
  const tabbed = surface();
  const stack = createStack(stackPage([onStage], [tabbed]));
  stack.focusWin(tabbed);
  stack.focusWin(onStage);
  assert.equal(tabbed.classList.contains("focused"), false);
  stack.focusWin(tabbed);
  assert.equal(onStage.classList.contains("focused"), false);
});

// The column list of a console in a tab works on the tab's grid.
function sliceShell() {
  const { scope: state, shell } = loadComponent("wbColumns", { magics: { $nextTick: () => {}, $refs: {} } });
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const tabs = shell.$store.tabs;
  const calls = [];
  const realConsole = globalThis.WBConsole;
  const rows = ["w1", "w2", "w3", "w4"].map((id) => ({ id, agent: "codex", kind: "agent", name: null }));
  globalThis.WBConsole = {
    ...realConsole,
    placeTabs: (held, grid) => calls.push(["place", grid]),
    applyColumns: () => calls.push(["stage"]),
    columnRoster: () => ({ rows, fences: [], membership: {}, detached: {} }),
    columnMeasure: () => ({ viewport: 1920 }),
    focusColumn: (id) => calls.push(["focus", id]),
    focusedId: () => "w1",
    list: () => rows,
    isRelaunching: () => false,
    refitAll() {},
  };
  return { state, tabs, calls, restore: () => (globalThis.WBConsole = realConsole) };
}

test("Slice in a tab opens and swaps in the tab's grid, and never paints the stage", () => {
  const { state, tabs, calls, restore } = sliceShell();
  try {
    tabs.openConsoleTab("w1", "codex");
    tabs.openConsoleTab("w4", "codex");
    tabs.activate("console:w1");
    state.toggleColumnMenu("w1", undefined);
    const rows = Object.fromEntries(state.columnGroups.flatMap((g) => g.rows).map((r) => [r.id, r]));
    assert.equal(rows.w2.enabled, true);
    assert.equal(rows.w4.enabled, false, "a console of another tab is not opened here");
    calls.length = 0;
    state.openColumn("w2");
    assert.deepEqual(tabs.tabOf("w2").grid, [["w1"], ["w2"]]);
    state.columnFrom = "w2";
    state.swapColumn("w3");
    assert.deepEqual(tabs.tabOf("w3").grid, [["w1"], ["w3"]]);
    assert.equal(tabs.holds("w2"), false, "the console swapped out goes back to the stage");
    assert.equal(calls.filter(([k]) => k === "stage").length, 0, "the stage's columns are not painted");
    assert.deepEqual(state.columns, [], "the stage's grid does not change");
  } finally {
    restore();
  }
});

test("Alt+Shift+arrows walk the grid of the active console tab", () => {
  const { state, tabs, calls, restore } = sliceShell();
  try {
    tabs.openConsoleTab("w1", "codex");
    tabs.setTabGrid("console:w1", [["w1"], ["w2"]]);
    calls.length = 0;
    assert.equal(state.arrowStep("x", 1), true);
    assert.deepEqual(
      calls.filter(([k]) => k === "focus"),
      [["focus", "w2"]],
    );
  } finally {
    restore();
  }
});

// The terminal's key handler: it gives Alt+Shift+arrows to the page in a
// column of the stage and in the grid of a console tab, and keeps them in a
// lone window.
test("a terminal in a tab's grid lets Alt+Shift+arrows go to the page", () => {
  const keep = (selectorHit) => {
    let handler = null;
    wirePress({
      term: { attachCustomKeyEventHandler: (h) => (handler = h) },
      body: { closest: (sel) => (selectorHit(sel) ? {} : null), addEventListener() {} },
      window: {},
      writeClipboard() {},
    });
    return handler({ type: "keydown", altKey: true, shiftKey: true, ctrlKey: false, metaKey: false, code: "ArrowLeft" });
  };
  assert.equal(keep((sel) => sel.includes(".session-window.tab-grid")), false);
  assert.equal(keep((sel) => sel.includes(".session-window.column")), false);
  assert.equal(keep(() => false), true, "a lone window keeps the chord for its child");
});
