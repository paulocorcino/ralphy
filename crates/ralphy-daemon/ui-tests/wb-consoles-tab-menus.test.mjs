// Unit tests for assets/ui/wb-consoles-tab-menus.ts, the Alpine component
// `wbConsoleMenus` on the toolbar of the Consoles tab: its menus and their
// keys. The tests that drive it together with `wbColumns` are in
// wb-consoles-tab.test.mjs. Each is built with `loadComponent`, so every test
// here also fails when the component reads a shell() name it does not list in
// `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// ADR-0063 §3: a NEW console opens in the checkout selected at the moment of
// the click, and in the primary when none is — the console keeps it afterwards
// (the title comes from the daemon's announcement, never from this selection).
// A console is born in the primary whatever the Files chip shows (ADR-0063,
// amendment 2026-09-16 b): the selection is what the panels LOOK at; only the
// console's own title switcher moves it.
test("newConsole opens the agent in the primary even under a selected checkout", () => {
  const { scope: state, shell } = loadComponent("wbConsoleMenus");
  const calls = [];
  const real = globalThis.WBConsole;
  // The component reaches `WBConsole` as a bare global; the real one needs a DOM.
  globalThis.WBConsole = { open: (o) => calls.push(o), count: () => 0 };
  try {
    shell.$store.tabs.active = "consoles";
    state.$store.projects.setOpen("o/r");
    shell.$store.checkouts.checkouts = { "o/r": "wt-a" };
    state.newConsole("claude");
    shell.$store.checkouts.checkouts = {};
    state.newConsole("codex");
  } finally {
    globalThis.WBConsole = real;
  }
  assert.deepEqual(calls, [
    { repo: "o/r", agent: "claude", checkout: null },
    { repo: "o/r", agent: "codex", checkout: null },
  ]);
});

test("the Note menu keeps a card on top, puts it back, and refuses a card in a popup", () => {
  const { scope: state, shell, window } = loadComponent("wbConsoleMenus");
  const calls = [];
  window.WBNotes = {
    putBack: () => calls.push("putBack"),
    keepOnTop: (id) => calls.push("keepOnTop:" + id),
    list: () => [{ id: "a", onTop: false, away: false }],
  };
  state.$nextTick = (fn) => fn();
  shell.$store.tabs.active = "consoles";
  state.noteMenu = true;

  // Putting back keeps the menu open and redraws the rows.
  state.toggleOnTop({ id: "a", onTop: true, away: false });
  assert.deepEqual(calls, ["putBack"]);
  assert.equal(state.noteMenu, true);
  assert.deepEqual(state.noteItems, [{ id: "a", onTop: false, away: false }]);

  // Keeping on top closes the menu, so the card is in view.
  state.toggleOnTop({ id: "a", onTop: false, away: false });
  assert.deepEqual(calls, ["putBack", "keepOnTop:a"]);
  assert.equal(state.noteMenu, false);

  // A card in a detached popup: nothing happens.
  state.toggleOnTop({ id: "b", onTop: false, away: true });
  assert.equal(calls.length, 2);
});

test("Alt+Shift+R from another tab opens the Consoles tab and its menu", () => {
  const { scope: state, shell } = loadComponent("wbConsoleMenus");
  const calls = [];
  state.$nextTick = () => {};
  shell.$store.tabs.activate = (tab) => {
    calls.push("activate:" + tab);
    shell.$store.tabs.active = tab;
  };
  shell.$store.tabs.active = "code";

  state.openConsoleRunMenu();
  // The menu sits in the Consoles tab's toolbar: on another tab it is hidden.
  assert.deepEqual(calls, ["activate:consoles"]);
  assert.equal(state.agentMenu, true);
  assert.equal(state.consoleRunOpen, true);
});

test("keeping a card on top from another tab opens the Consoles tab first", () => {
  const { scope: state, shell, window } = loadComponent("wbConsoleMenus");
  const calls = [];
  window.WBNotes = { keepOnTop: (id) => calls.push("keepOnTop:" + id) };
  const ticks = [];
  state.$nextTick = (fn) => ticks.push(fn);
  shell.$store.tabs.activate = (tab) => {
    calls.push("activate:" + tab);
    shell.$store.tabs.active = tab;
  };
  shell.$store.tabs.active = "code";

  state.toggleOnTop({ id: "a", onTop: false, away: false });
  // The card is placed only after the tab is shown: a hidden tab measures 0×0.
  assert.deepEqual(calls, ["activate:consoles"]);
  ticks.forEach((fn) => fn());
  assert.deepEqual(calls, ["activate:consoles", "keepOnTop:a"]);
});
