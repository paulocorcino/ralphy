// A console in its own tab (ADR-0037 §3d): the store `tabs` keeps the tab,
// the console module moves the window into the tab's pane and back, and the
// stage's rules leave a window in a tab alone.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { createPlace } from "../assets/ui/wb-console-place.ts";
import { createStageWindow } from "../assets/ui/wb-stage-window.ts";
import { createDormancy } from "../assets/ui/wb-console-dormancy.ts";

// The store reaches `WBViewer` as a bare global: each test puts the real one back.
const realViewer = globalThis.WBViewer;
afterEach(() => {
  globalThis.WBViewer = realViewer;
});

// A page whose console module has the consoles `ids`. `places` records each
// `placeTabs` call as `[held, shown]`.
function tabShell(ids, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  window.Alpine.nextTick = (fn) => fn();
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const places = [];
  const page = { ids: new Set(ids), relaunching: new Set(), names: {} };
  window.WBConsole = {
    placeTabs: (held, shown) => places.push([[...held], shown]),
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
  assert.deepEqual(last(), [["w1"], "w1"]);
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
  assert.deepEqual(last(), [["w1"], "w1"]);
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
    { deskId: "w1", title: "codex" },
    { deskId: "w2", title: "claude" },
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
  assert.deepEqual(next.last(), [["w1"], "w1"], "the kept tab holds its window");
});

test("a console tab takes the console's new name", () => {
  const { tabs, page, stored } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  page.names.w1 = "build";
  tabs.syncConsoleTabs();
  const title = tabs.tabs.find((t) => t.id === "console:w1").title;
  assert.notEqual(title, "codex");
  assert.match(title, /build/);
  assert.deepEqual(stored().consoleTabs, [{ deskId: "w1", title }]);
});

test("a console that relaunches keeps its tab", () => {
  const { tabs, page } = tabShell(["w1"]);
  tabs.openConsoleTab("w1", "codex");
  page.ids.delete("w1");
  page.relaunching.add("w1");
  tabs.syncConsoleTabs();
  assert.ok(tabs.holds("w1"));
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
  return {
    _deskId: id,
    parentElement: null,
    style: { left: "40px", top: "50px", width: "600px", height: "400px" },
    classList: {
      contains: (c) => set.has(c),
      toggle: (c, on) => (on ? set.add(c) : set.delete(c), on),
    },
  };
}

// The stage with the windows `ids`, and the pane of the console tabs.
function placePage(ids) {
  const stage = parent("stage");
  const pane = parent("pane");
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
  p.place.placeTabs(["w1"], "w1");
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
  assert.deepEqual(w1.style, { left: "40px", top: "50px", width: "600px", height: "400px" });
  assert.deepEqual(p.moved, ["w1"]);
  assert.equal(p.sent.length, 1, "going back to the stage sends nothing");
});

test("only the active tab's window is shown, and a second call moves nothing", () => {
  const p = placePage(["w1", "w2", "w3"]);
  p.place.placeTabs(["w1", "w2", "w9"], "w2");
  assert.equal(p.byId.w1.classList.contains("tab-shown"), false);
  assert.ok(p.byId.w2.classList.contains("tab-shown"));
  assert.equal(p.byId.w3.parentElement, p.stage);
  p.moved.length = 0;
  p.place.placeTabs(["w1", "w2"], "w2");
  assert.deepEqual(p.moved, []);
  p.place.placeTabs(["w1", "w2"], "w1");
  assert.deepEqual(p.moved, ["w1", "w2"], "the shown window changes, nothing moves");
  assert.equal(p.sent.length, 1);
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
    tabs.activate("consoles");
    const row = { id: "w2", enabled: false, reason: WBColumns.REASON_TAB };
    assert.equal(menu.rowOff(row), false);
    menu.pick(row);
    assert.equal(tabs.active, "console:w2");
    assert.deepEqual(
      tabs.tabs.map((t) => t.id),
      ["consoles", "console:w2"],
    );

    tabs.activate("consoles");
    const { scope: menus } = loadComponent("wbConsoleMenus", { from: { state: shell, window: globalThis.window, document: globalThis.document } });
    menus.revealWindow("w2");
    assert.equal(tabs.active, "console:w2");
  } finally {
    globalThis.WBConsole = realConsole;
  }
});
