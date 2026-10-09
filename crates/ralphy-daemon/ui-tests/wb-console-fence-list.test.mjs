// Unit tests for assets/ui/wb-console-fence-list.ts — the fence records'
// chrome, the fence verbs and the focused fence. `createFenceList` is driven
// with fake `deps`, a fake stage and the real popup registry, with no console
// and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFenceList } from "../assets/ui/wb-console-fence-list.ts";
import { createPopupRegistry } from "../assets/ui/wb-console-popups.ts";

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
