// The tab menu of the store `tabs` (wb-tabs-store-menu.ts): "Close the tabs
// to the right", run on the real store.
import { afterEach, test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

// The store reaches `WBViewer` as a bare global: each test puts the real one back.
const realViewer = globalThis.WBViewer;
afterEach(() => {
  globalThis.WBViewer = realViewer;
});

// The store with the file tabs `ids` after Consoles, and the menu its last
// right click painted.
function tabsWith(ids) {
  const { state, window } = loadShell();
  window.WBView = { patch() {}, read: () => null };
  window.Alpine.nextTick = (fn) => fn();
  globalThis.WBViewer = { setActive() {}, width: () => 1280, close() {} };
  const tabs = state.$store.tabs;
  for (const id of ids) tabs.tabs.push({ id, kind: "code", title: id, closable: true });
  let painted = [];
  tabs.renderMenu = (_x, _y, items) => (painted = items);
  const menuOf = (id) => {
    tabs.showTabMenu(0, 0, tabs.tabs.find((t) => t.id === id));
    return painted;
  };
  return { tabs, menuOf };
}

const entry = (items, label) => items.find((it) => it.label === label);
const ids = (tabs) => tabs.tabs.map((t) => t.id);

test("Close the tabs to the right closes every tab after the clicked one", () => {
  const { tabs, menuOf } = tabsWith(["a", "b", "c", "d"]);
  tabs.activate("b");
  entry(menuOf("b"), "Close the tabs to the right").run();
  assert.deepEqual(ids(tabs), ["consoles", "a", "b"]);
  assert.equal(tabs.active, "b");
});

test("an active tab to the right gives its place to the clicked tab", () => {
  // Closed alone, the active "b" would hand its place to "c", which closes next.
  const { tabs, menuOf } = tabsWith(["a", "b", "c"]);
  tabs.activate("b");
  // Every change of the active tab goes through `showTab`.
  const shown = [];
  const showTab = tabs.showTab.bind(tabs);
  tabs.showTab = (id) => (shown.push(id), showTab(id));
  entry(menuOf("a"), "Close the tabs to the right").run();
  assert.deepEqual(ids(tabs), ["consoles", "a"]);
  assert.deepEqual(shown, ["a"], "no tab that closes is shown on the way");
});

test("the last tab has nothing to its right: the entry is off", () => {
  const { menuOf } = tabsWith(["a", "b"]);
  assert.equal(entry(menuOf("b"), "Close the tabs to the right").disabled, true);
  assert.equal(entry(menuOf("a"), "Close the tabs to the right").disabled, false);
});
