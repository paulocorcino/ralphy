// Unit tests for assets/ui/wb-stage-columns.ts — the column paint.
// `createStageColumns` is driven with fake `deps`: a viewport of a given
// width and one window with the title-bar button, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createStageColumns } from "../assets/ui/wb-stage-columns.ts";

// The title-bar button that opens the list of consoles.
function fakeButton() {
  const attrs = {};
  return {
    hidden: true,
    title: "Slice",
    innerHTML: '<i class="bi bi-arrows-expand-vertical"></i>',
    setAttribute: (k, v) => (attrs[k] = v),
    getAttribute: (k) => attrs[k] ?? null,
  };
}

// A maximized window with that button, on a viewport `width` px wide.
function paint({ width, autoBoot = true }) {
  const btn = fakeButton();
  const win = { _deskId: "a", _colBtn: btn, classList: { contains: (c) => c === "maximized" } };
  const noop = () => {};
  const cols = createStageColumns({
    OPTS: { autoBoot },
    wins: new Set([win]),
    workspace: () => ({ clientWidth: width }),
    stage: () => null,
    popups: { entries: () => [] },
    desk: () => [],
    focusWin: noop,
    setMax: noop,
    paintMaxButton: noop,
    syncMaxLock: noop,
    syncMaxPin: noop,
    applyExtent: noop,
    list: () => [],
    paintFenceColumns: noop,
    fenceList: () => [],
    readFenceRects: () => [],
    readWindowRects: () => [],
    findWindow: () => null,
    tearDownMember: noop,
  });
  cols.applyColumns([]);
  return btn;
}

// A phone paints one console, so the list there switches the maximized
// console: the button shows with its own name and icon (#599).
test("a maximized console shows the list button as Switch on a phone and as Slice wider", () => {
  const phone = paint({ width: 400 });
  assert.equal(phone.hidden, false, "the button shows on a phone");
  assert.deepEqual([phone.title, phone.getAttribute("aria-label")], ["Switch", "Switch"]);
  assert.match(phone.innerHTML, /bi-arrow-down-up/);
  const wide = paint({ width: 1200 });
  assert.equal(wide.hidden, false);
  assert.deepEqual([wide.title, wide.getAttribute("aria-label")], ["Slice", "Slice"]);
  assert.match(wide.innerHTML, /bi-arrows-expand-vertical/);
});

test("the detached-fence popup never shows the list button", () => {
  for (const width of [400, 1200]) assert.equal(paint({ width, autoBoot: false }).hidden, true, `${width}px`);
});

// A window with a class list and an inline style the paint can write and read.
function fakeWindow(id, classes = []) {
  const set = new Set(classes);
  const style = new Map();
  return {
    _deskId: id,
    classList: {
      add: (c) => set.add(c),
      remove: (c) => set.delete(c),
      contains: (c) => set.has(c),
    },
    style: {
      setProperty: (k, v) => style.set(k, v),
      removeProperty: (k) => style.delete(k),
    },
    classes: set,
    vars: style,
  };
}

// The columns `wins`, painted with `painted`, each window found by its id.
function paintWindows(wins, painted) {
  const noop = () => {};
  const byId = new Map(wins.map((w) => [w._deskId, w]));
  const setMax = [];
  const cols = createStageColumns({
    OPTS: { autoBoot: true },
    wins: new Set(wins),
    workspace: () => ({ clientWidth: 1200 }),
    stage: () => null,
    popups: { entries: () => [] },
    desk: () => [],
    focusWin: noop,
    setMax: (win, on) => {
      setMax.push([win._deskId, on]);
      if (on) win.classList.add("maximized");
      else win.classList.remove("maximized");
    },
    paintMaxButton: noop,
    syncMaxLock: noop,
    syncMaxPin: noop,
    applyExtent: noop,
    list: () => [],
    paintFenceColumns: noop,
    fenceList: () => [],
    readFenceRects: () => [],
    readWindowRects: () => [],
    findWindow: (id) => byId.get(id) ?? null,
    tearDownMember: noop,
  });
  cols.applyColumns(painted);
  return setMax;
}

test("a painted column carries its place as CSS variables, with one row when the grid names none", () => {
  const a = fakeWindow("a");
  const b = fakeWindow("b");
  const setMax = paintWindows([a, b], [
    { id: "a", index: 0, count: 2 },
    { id: "b", index: 1, count: 2, row: 1, rows: 3 },
  ]);
  assert.deepEqual(Object.fromEntries(a.vars), {
    "--col-index": "0",
    "--col-count": "2",
    "--row-index": "0",
    "--row-count": "1",
  });
  assert.deepEqual(Object.fromEntries(b.vars), {
    "--col-index": "1",
    "--col-count": "2",
    "--row-index": "1",
    "--row-count": "3",
  });
  assert.ok(a.classes.has("column") && b.classes.has("column"));
  assert.deepEqual(setMax, [["a", true]], "the first console is the maximized one");
});

test("a window the paint no longer names leaves the columns and drops its variables", () => {
  const a = fakeWindow("a");
  const b = fakeWindow("b");
  const stale = fakeWindow("z", ["column"]);
  stale.style.setProperty("--col-index", "2");
  stale.style.setProperty("--max-left", "10px");
  paintWindows([a, b, stale], [
    { id: "a", index: 0, count: 2 },
    { id: "b", index: 1, count: 2 },
  ]);
  assert.equal(stale.classes.has("column"), false);
  assert.deepEqual([...stale.vars.keys()], [], "a window that is not maximized drops its maximize box too");
});

// The columns `wins` painted with `raise`, each window found by its id; the
// windows `focusWin` raised are answered in order.
function raiseWindows(wins, painted, raise) {
  const noop = () => {};
  const byId = new Map(wins.map((w) => [w._deskId, w]));
  const raised = [];
  const cols = createStageColumns({
    OPTS: { autoBoot: true },
    wins: new Set(wins),
    workspace: () => ({ clientWidth: 1200 }),
    stage: () => null,
    popups: { entries: () => [] },
    desk: () => [],
    focusWin: (win) => raised.push(win._deskId),
    setMax: noop,
    paintMaxButton: noop,
    syncMaxLock: noop,
    syncMaxPin: noop,
    applyExtent: noop,
    list: () => [],
    paintFenceColumns: noop,
    fenceList: () => [],
    readFenceRects: () => [],
    readWindowRects: () => [],
    findWindow: (id) => byId.get(id) ?? null,
    tearDownMember: noop,
  });
  cols.applyColumns(painted, { raise });
  return raised;
}

test("a paint that raises focuses the columns in reading order, and a window whose fit throws does not stop the next", () => {
  const fitted = [];
  const a = fakeWindow("a");
  const b = fakeWindow("b");
  const c = fakeWindow("c");
  a._term = { fit: { fit: () => fitted.push("a") } };
  b._term = {
    fit: {
      fit: () => {
        fitted.push("b");
        throw new Error("a terminal that cannot be measured");
      },
    },
  };
  c._term = { fit: { fit: () => fitted.push("c") } };
  const painted = [
    { id: "a", index: 0, count: 3 },
    { id: "b", index: 1, count: 3 },
    { id: "c", index: 2, count: 3 },
  ];
  assert.deepEqual(raiseWindows([c, b, a], painted, true), ["a", "b", "c"]);
  assert.deepEqual(fitted, ["a", "b", "c"], "every window is fitted, also after the one that threw");
});

test("a paint that does not ask to raise focuses nothing but still fits the columns", () => {
  const fitted = [];
  const a = fakeWindow("a");
  a._term = { fit: { fit: () => fitted.push("a") } };
  assert.deepEqual(raiseWindows([a], [{ id: "a", index: 0, count: 1 }], false), []);
  assert.deepEqual(fitted, ["a"]);
});
