// Consoles in their own tab (ADR-0037 §3d): the store `tabs` keeps the tab
// and its grid, the console module moves the windows into the tab's pane and
// back, and the stage's rules leave a window in a tab alone.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

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
  return { tabs: state.$store.tabs, places, page, stored: () => stored, last: () => places.at(-1) };
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

test("a console tab takes the new name of the console that names it, and of no other", () => {
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
  assert.match(title, /build/u);
  assert.equal(stored().consoleTabs[0].title, title);
  tabs.leaveTab("w1");
  assert.equal(tabs.tabOf("w2").title, "tests (codex)", "the console left in the tab names it");
  page.names.w1 = "later";
  tabs.syncConsoleTabs();
  assert.equal(tabs.tabOf("w2").title, "tests (codex)", "the console that left names it no more");
  page.names.w2 = "e2e";
  tabs.syncConsoleTabs();
  assert.equal(tabs.tabOf("w2").title, "e2e (codex)");
  assert.equal(stored().consoleTabs[0].deskId, "w2");
});

test("the first console of the grid names a tab whose naming console left it", () => {
  const { tabs, page } = tabShell(["w1", "w2", "w3", "w4"]);
  Object.assign(page.names, { w1: "one", w2: "two", w3: "three", w4: "four" });
  tabs.openConsoleTab("w2", "two (codex)");
  tabs.setTabGrid("console:w2", [["w1"], ["w2", "w3"]]);
  tabs.leaveTab("w2");
  assert.equal(tabs.tabOf("w1").title, "one (codex)", "Restore: the first console in reading order");
  tabs.setTabGrid("console:w2", [["w4"], ["w3"]]);
  assert.equal(tabs.tabOf("w4").title, "four (codex)", "Swap: the console that came in first");
  page.ids.delete("w4");
  tabs.syncConsoleTabs();
  assert.equal(tabs.tabOf("w3").title, "three (codex)", "a console that left the page");
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

test("Restore in a tab takes the console back to the stage; the tab keeps its id", () => {
  const { tabs, last } = tabShell(["w1", "w2"]);
  tabs.openConsoleTab("w1", "codex");
  tabs.setTabGrid("console:w1", [["w1"], ["w2"]]);
  tabs.leaveTab("w1");
  assert.equal(tabs.holds("w1"), false);
  const tab = tabs.tabOf("w2");
  assert.equal(tab.id, "console:w1");
  assert.deepEqual(last(), [["w2"], [["w2"]]]);
  // A new tab for the console that left takes the next free id.
  tabs.openConsoleTab("w1", "codex");
  assert.equal(tabs.tabOf("w1").id, "console:w1-2");
  assert.equal(tabs.active, "console:w1-2");
});
