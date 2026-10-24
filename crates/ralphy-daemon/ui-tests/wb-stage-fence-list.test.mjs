// Unit tests for assets/ui/wb-stage-fence-list.ts — the fence records'
// chrome, the fence verbs and the focused fence. `createFenceList` is driven
// with fake `deps`, a fake stage and the real popup registry, with no console
// and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFenceList } from "../assets/ui/wb-stage-fence-list.ts";
import { createPopupRegistry } from "../assets/ui/wb-desk-popups.ts";

// A fence element with the few DOM members the fence list uses. `parts`
// holds its children by selector (`.fence-name`, `.fence-notice`, ...).
function fakeFence(id, rect) {
  const classes = new Set();
  const parts = {
    ".fence-name": { value: "" },
    ".fence-notice": { textContent: "" },
  };
  return {
    dataset: { fenceId: id },
    rect,
    style: {},
    removed: false,
    classList: {
      contains: (c) => classes.has(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
    },
    querySelector: (sel) => parts[sel] || null,
    remove() {
      this.removed = true;
    },
  };
}

// The stage holds `els`, the fence elements; `state.fences` is the record
// list the console holds, read through the getter at each use. The calls
// that change the desk or the view are recorded in `calls`.
function harness() {
  const els = [];
  const calls = [];
  const state = { fences: [], jumpAnswer: true };
  const record = (name) => (...args) => calls.push([name, ...args]);
  // `offsetWidth` 0: a stage that cannot measure, so the render leaves the
  // count readouts alone and the tests read only the records' DOM.
  const st = {
    offsetWidth: 0,
    offsetHeight: 0,
    querySelectorAll: (sel) => (sel === ".fence" ? [...els] : []),
  };
  const popups = createPopupRegistry({ link: { writeRegistry() {} }, startBeat() {}, stopBeat() {} });
  const list = createFenceList({
    window: {},
    document: { activeElement: null },
    OPTS: {},
    popups,
    fences: () => state.fences,
    notes: () => [],
    stage: () => st,
    workspace: () => null,
    tabsPane: () => null,
    restoreRect: (el) => el.rect,
    applyExtent: record("applyExtent"),
    columnMeasure: () => ({ viewport: 1600 }),
    inGesture: () => false,
    newFenceId: () => "new",
    paintLockGlyph() {},
    saveFences: (next) => {
      calls.push(["saveFences", next]);
      state.fences = next;
    },
    jumpToFence: (id) => {
      calls.push(["jumpToFence", id]);
      return state.jumpAnswer;
    },
    buildFence: (f) => {
      calls.push(["buildFence", f.id]);
      const el = fakeFence(f.id, f.rect);
      els.push(el);
      return el;
    },
  });
  return { list, els, calls, state, popups };
}

const RECT = { left: 100, top: 100, width: 600, height: 400 };

// `removeFence` announces a refusal through the page's `WB`.
function withWB(t) {
  const real = globalThis.WB;
  const emitted = [];
  globalThis.WB = { emit: (type, detail) => emitted.push({ type, detail }) };
  t.after(() => {
    globalThis.WB = real;
  });
  return emitted;
}

test("a render builds a fence the stage lacks, places it, and drops one the records no longer hold", () => {
  const { list, els, calls, state } = harness();
  state.fences = [
    { id: "a", name: "A", rect: RECT },
    { id: "b", name: "B", rect: { left: 900, top: 100, width: 300, height: 200 } },
  ];
  list.renderFences();
  assert.deepEqual(
    calls.filter(([name]) => name === "buildFence"),
    [
      ["buildFence", "a"],
      ["buildFence", "b"],
    ],
  );
  assert.equal(els[1].style.left, "900px");
  assert.equal(els[1].querySelector(".fence-name").value, "B");
  // The console reassigns its record list; the next render reads the new one.
  state.fences = [state.fences[0]];
  list.renderFences();
  assert.equal(els[0].removed, false);
  assert.equal(els[1].removed, true);
});

test("the focused fence is marked on its element, read back, and cleared when its fence is gone", () => {
  const { list, els, state } = harness();
  state.fences = [
    { id: "a", name: "A", rect: RECT },
    { id: "b", name: "B", rect: RECT },
  ];
  list.renderFences();
  list.focusFence("b");
  assert.equal(list.focusedFenceId(), "b");
  assert.equal(els[1].classList.contains("is-focused"), true);
  assert.equal(els[0].classList.contains("is-focused"), false);
  state.fences = [state.fences[0]];
  list.renderFences();
  assert.equal(list.focusedFenceId(), null);
});

test("a fence name is cut at 60 characters", () => {
  const { list, state } = harness();
  state.fences = [{ id: "a", name: "A", rect: RECT }];
  list.renameFence("a", "x".repeat(80));
  assert.equal(state.fences[0].name, "x".repeat(60));
});

test("a detached fence is not removed, and the refusal is said on the fence", (t) => {
  const emitted = withWB(t);
  const { list, els, calls, state, popups } = harness();
  state.fences = [{ id: "a", name: "A", rect: RECT }];
  list.renderFences();
  popups.put("a", popups.newPopupEntry());
  popups.commitDetached(["a"]);
  t.mock.timers.enable({ apis: ["setTimeout"] });
  list.removeFence("a");
  assert.deepEqual(state.fences.map((f) => f.id), ["a"]);
  assert.equal(calls.some(([name]) => name === "saveFences"), false);
  assert.equal(els[0].querySelector(".fence-notice").textContent, "Return this fence's consoles to this window first");
  assert.deepEqual(emitted, [{ type: "fence-remove-refused", detail: { fence: "a", reason: "detached" } }]);
});

test("a step lands on the fence the view jumped to, and on nothing when the jump fails", () => {
  const { list, calls, state } = harness();
  state.fences = [{ id: "a", name: "A", rect: RECT }];
  list.renderFences();
  assert.equal(list.stepFence(1), "a");
  assert.deepEqual(calls.at(-1), ["jumpToFence", "a"]);
  state.jumpAnswer = false;
  assert.equal(list.stepFence(1), null);
});

// An element with a class list and children by selector.
function fakeEl(rect, parts = {}, data = {}) {
  const classes = new Set();
  return {
    rect,
    dataset: data,
    classList: {
      contains: (c) => classes.has(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
    },
    querySelector: (sel) => parts[sel] || null,
    classes,
  };
}

// A fence with the count and the columns button `refreshFenceChrome` writes.
function chromeFence(id, rect) {
  return fakeEl(rect, { ".fence-name": { value: id }, ".fence-count": { textContent: "" }, ".fence-columns": { disabled: false } }, { fenceId: id });
}

// Fence `a` is locked and holds one console and one card, `b` holds two
// consoles, and `c` went to a popup with three.
// `tabbed`: the console of `a` is in its own tab, in the pane of the tabs.
function chromeHarness({ autoBoot, measured = true, ownLocks = false, tabbed = false } = {}) {
  const rect = (left, top) => ({ left, top, width: 400, height: 400 });
  const at = (left, top) => ({ left, top, width: 100, height: 100 });
  const fences = [
    { id: "a", name: "A", rect: rect(0, 0), locked: true },
    { id: "b", name: "B", rect: rect(500, 0) },
    { id: "c", name: "C", rect: rect(0, 500) },
  ];
  const fenceEls = fences.map((f) => chromeFence(f.id, f.rect));
  const wins = [fakeEl(at(10, 10)), fakeEl(at(510, 10)), fakeEl(at(610, 110))];
  wins.forEach((w, i) => (w._deskId = `w${i}`));
  wins[0]._deskLocked = ownLocks;
  const card = fakeEl(at(150, 150), {}, { noteId: "n1" });
  if (tabbed) wins[0].classes.add("in-tab");
  const onStage = tabbed ? wins.slice(1) : wins;
  const inPane = tabbed ? [wins[0]] : [];
  const st = {
    offsetWidth: measured ? 1200 : 0,
    offsetHeight: measured ? 900 : 0,
    querySelectorAll: (sel) => ({ ".fence": fenceEls, ".session-window": onStage, ".note-card": [card] })[sel] || [],
  };
  const pane = { querySelectorAll: (sel) => (sel === ".session-window" ? inPane : []) };
  const locks = [];
  const popups = createPopupRegistry({ link: { writeRegistry() {} }, startBeat() {}, stopBeat() {} });
  popups.put("c", popups.newPopupEntry(["x", "y", "z"]));
  popups.commitDetached(["c"]);
  const list = createFenceList({
    window: { WBNotes: { applyLock: (el, on) => locks.push([el.dataset.noteId, on]) } },
    document: { activeElement: null },
    OPTS: autoBoot === undefined ? {} : { autoBoot },
    popups,
    fences: () => fences,
    notes: () => [{ id: "n1", locked: ownLocks }],
    stage: () => st,
    workspace: () => null,
    tabsPane: () => pane,
    restoreRect: (el) => el.rect,
    applyExtent() {},
    columnMeasure: () => ({ viewport: 1600 }),
    inGesture: () => false,
    newFenceId: () => "new",
    paintLockGlyph() {},
    saveFences() {},
    jumpToFence: () => null,
    buildFence: () => null,
  });
  const count = (i) => fenceEls[i].querySelector(".fence-count").textContent;
  const columns = (i) => fenceEls[i].querySelector(".fence-columns").disabled;
  return { list, wins, card, locks, count, columns };
}

test("the fence chrome counts each fence's consoles, and a detached fence's from the popup", () => {
  const { list, count, columns } = chromeHarness();
  list.refreshFenceChrome();
  assert.deepEqual([count(0), count(1), count(2)], ["(1 console)", "(2 consoles)", "(3 consoles)"]);
  assert.deepEqual([columns(0), columns(1), columns(2)], [false, false, true], "a fence with no console on the stage has no columns");
});

test("a console in its own tab still counts for its fence, which then has no columns", () => {
  const { list, count, columns } = chromeHarness({ tabbed: true });
  list.refreshFenceChrome();
  assert.equal(count(0), "(1 console)", "the console in the pane of the tabs is a member");
  assert.equal(columns(0), true, "a console in its own tab does not open as a column");
  assert.deepEqual([count(1), columns(1)], ["(2 consoles)", false]);
});

test("the fence chrome writes nothing while the stage cannot measure", () => {
  const { list, wins, card, locks, count } = chromeHarness({ measured: false });
  list.refreshFenceChrome();
  assert.deepEqual([count(0), count(1), count(2)], ["", "", ""]);
  assert.equal(wins[0].classes.has("held"), false);
  assert.deepEqual([card.classes.has("held"), locks], [false, []]);
});

test("a locked fence holds its consoles and its note cards", () => {
  const { list, wins, card, locks } = chromeHarness();
  list.refreshFenceChrome();
  assert.deepEqual(wins.map((w) => w.classes.has("held")), [true, false, false]);
  assert.equal(card.classes.has("held"), true);
  assert.deepEqual(locks, [["n1", true]]);
});

test("in the detached-fence popup a locked fence holds no console and no card", () => {
  const { list, wins, card, locks } = chromeHarness({ autoBoot: false });
  list.refreshFenceChrome();
  assert.deepEqual(wins.map((w) => w.classes.has("held")), [false, false, false]);
  assert.deepEqual([card.classes.has("held"), locks], [false, []]);
});

// A console or a card with its own lock wears that lock, not the fence's.
test("a console or a card locked on its own is not held by its locked fence", () => {
  const { list, wins, card, locks } = chromeHarness({ ownLocks: true });
  list.refreshFenceChrome();
  assert.equal(wins[0].classes.has("held"), false);
  assert.equal(card.classes.has("held"), false);
  assert.deepEqual(locks, [["n1", true]], "the card is still read-only");
});
