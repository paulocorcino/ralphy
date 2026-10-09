// Unit tests for assets/ui/wb-console-fences.ts — the console's fences.
// `createFences` is driven with fake `deps`, a fake document and the real
// gestures owner and popup registry, with no console and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFences } from "../assets/ui/wb-console-fences.ts";
import { createGestures } from "../assets/ui/wb-console-chrome.ts";
import { createPopupRegistry } from "../assets/ui/wb-console-popups.ts";

const RECT = { left: 100, top: 100, width: 600, height: 400 };

// An element with the few DOM members the fences use.
function fakeElement() {
  const classes = new Set();
  return {
    className: "",
    dataset: {},
    style: {},
    children: [],
    classList: {
      contains: (c) => classes.has(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
    },
    setAttribute() {},
    append(...kids) {
      this.children.push(...kids);
    },
    addEventListener() {},
    querySelector: () => null,
  };
}

// The page's document: elements, and listeners a test fires by type.
function fakeDocument() {
  const listeners = {};
  return {
    events: [],
    createElement: () => fakeElement(),
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
    dispatchEvent(ev) {
      this.events.push(ev.type);
    },
    fire(type, ev = {}) {
      for (const fn of [...(listeners[type] || [])]) fn(ev);
    },
    listening: (type) => (listeners[type] || []).length,
  };
}

// Every member of `FenceDeps`, with one fence `f` on an empty stage. The
// calls that change the desk are recorded in `calls`.
function fakeDeps(more = {}) {
  const calls = [];
  const plane = fakeElement();
  plane.querySelectorAll = () => [];
  plane.getBoundingClientRect = () => ({ left: 0, top: 0 });
  plane.offsetWidth = 4000;
  plane.offsetHeight = 3000;
  const record = (name) => (...args) => calls.push([name, ...args]);
  const deps = {
    window: { addEventListener() {}, removeEventListener() {}, open: () => ({ closed: false }) },
    document: fakeDocument(),
    gestures: createGestures(),
    popups: createPopupRegistry({ link: { writeRegistry() {} }, startBeat() {}, stopBeat() {} }),
    link: { tab: "t1", post: record("post") },
    wins: new Set(),
    fences: () => [{ id: "f", name: "A", rect: RECT }],
    stage: () => plane,
    applyExtent() {},
    askConfirm: async () => true,
    autoPan: () => ({ follow() {}, stop() {} }),
    clearFenceFlash() {},
    fenceEl: () => null,
    fenceLocked: () => false,
    fenceNotice: record("fenceNotice"),
    fenceSnapshot: () => [{ id: "w1", repo: "r" }],
    focusWin() {},
    glyphClick() {},
    showDetachGlyph() {},
    newPid: () => "p1",
    readFenceRects: () => [{ id: "f", name: "A", rect: RECT }],
    readWindowRects: () => [],
    reattachFence: record("reattachFence"),
    refreshFenceChrome() {},
    removeFence: record("removeFence"),
    renameFence: record("renameFence"),
    setFenceLock: record("setFenceLock"),
    saveFences: record("saveFences"),
    renderFences() {},
    renderNotes() {},
    restoreRect: () => ({ ...RECT }),
    setWin: record("setWin"),
    tearDownMember: record("tearDownMember"),
    ...more,
  };
  return { deps, calls };
}

// `detachFence` announces itself through the page's `WB`.
function withWB(t) {
  const real = globalThis.WB;
  const emitted = [];
  globalThis.WB = { emit: (type, detail) => emitted.push({ type, detail }) };
  t.after(() => {
    globalThis.WB = real;
  });
  return emitted;
}

test("a fence drag begins a gesture through the owner at the press and ends it at the release", () => {
  const { deps, calls } = fakeDeps();
  const { startFenceMove } = createFences(deps);
  const el = fakeElement();
  startFenceMove(el, { id: "f" })({
    button: 0,
    isPrimary: true,
    pointerId: 1,
    pointerType: "mouse",
    clientX: 150,
    clientY: 110,
    preventDefault() {},
    stopPropagation() {},
  });
  assert.equal(deps.gestures.active(el), true);
  deps.document.fire("pointerup", { pointerId: 1 });
  assert.equal(deps.gestures.active(el), false);
  assert.equal(deps.document.listening("pointermove"), 0);
  // A press with no movement is a click: it writes nothing.
  assert.deepEqual(calls, []);
});

test("a fence resize begins and ends a gesture through the owner", () => {
  const { deps } = fakeDeps();
  const { startFenceResize } = createFences(deps);
  const el = fakeElement();
  startFenceResize(el, { id: "f" }, "se")({
    button: 0,
    isPrimary: true,
    pointerId: 1,
    pointerType: "mouse",
    clientX: 700,
    clientY: 500,
    preventDefault() {},
    stopPropagation() {},
  });
  assert.equal(deps.gestures.active(el), true);
  deps.document.fire("pointercancel", { pointerId: 1 });
  assert.equal(deps.gestures.active(el), false);
});

test("a detach adds one popup entry, commits the fence as detached and takes its consoles off the stage", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const emitted = withWB(t);
  const win = { _deskId: "w1" };
  const { deps, calls } = fakeDeps({ wins: new Set([win]) });
  const { detachFence } = createFences(deps);
  detachFence("f");
  const { popups } = deps;
  assert.equal(popups.size(), 1);
  assert.equal(popups.isDetached("f"), true);
  const entry = popups.entry("f");
  assert.equal(entry.pid, "p1");
  assert.deepEqual(entry.memberIds, ["w1"]);
  assert.deepEqual(entry.fence, { id: "f", name: "A", rect: RECT });
  assert.deepEqual(
    calls.filter(([name]) => name === "tearDownMember"),
    [["tearDownMember", win, "reconnect"]],
  );
  assert.deepEqual(deps.document.events, ["workbench:columns-leave"]);
  assert.deepEqual(emitted.at(-1), { type: "fence-detach", detail: { fence: "f" } });
});

test("a blocked popup leaves the registry and the stage as they were", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  const emitted = withWB(t);
  const { deps, calls } = fakeDeps({
    window: { addEventListener() {}, removeEventListener() {}, open: () => null },
  });
  const { detachFence } = createFences(deps);
  detachFence("f");
  assert.equal(deps.popups.size(), 0);
  assert.equal(deps.popups.isDetached("f"), false);
  assert.deepEqual(calls, [["fenceNotice", "f", "Could not detach: pop-up blocked"]]);
  assert.deepEqual(emitted, [{ type: "fence-detach-blocked", detail: { fence: "f" } }]);
});

test("tiling a detached fence changes no console", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const win = fakeElement();
  win._deskId = "w1";
  const fence = fakeElement();
  const { deps, calls } = fakeDeps({ fenceEl: () => fence });
  deps.stage().querySelectorAll = (sel) => (sel === ".session-window" ? [win] : []);
  const { arrangeFence } = createFences(deps);
  deps.popups.put("f", deps.popups.newPopupEntry());
  deps.popups.commitDetached(["f"]);
  arrangeFence("f");
  assert.deepEqual(calls, []);
  // The same fence, attached, tiles its console.
  deps.popups.remove("f");
  deps.popups.commitDetached([]);
  arrangeFence("f");
  assert.deepEqual(
    calls.map(([name]) => name),
    ["setWin"],
  );
});
