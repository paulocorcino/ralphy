// Unit tests for assets/ui/wb-stage-fences.ts — the console's fences.
// `createFences` is driven with fake `deps`, a fake document and the real
// gestures owner and popup registry, with no console and no browser. The move
// and resize gestures are `wb-stage-fence-gestures.ts`, built inside the fences.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFences } from "../assets/ui/wb-stage-fences.ts";
import { createGestures } from "../assets/ui/wb-stage-stack.ts";
import { createPopupRegistry } from "../assets/ui/wb-desk-popups.ts";

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
    listeners: {},
    setAttribute() {},
    append(...kids) {
      this.children.push(...kids);
    },
    addEventListener(type, fn) {
      this.listeners[type] = fn;
    },
    querySelector: () => null,
  };
}

// The element of `root`'s tree with the class name `name`.
function findByClass(root, name) {
  if (root.className === name) return root;
  for (const kid of root.children) {
    const hit = findByClass(kid, name);
    if (hit) return hit;
  }
  return null;
}

// The page's document: elements, and listeners a test fires by type.
function fakeDocument() {
  const listeners = {};
  return {
    events: [],
    details: [],
    createElement: () => fakeElement(),
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
    dispatchEvent(ev) {
      this.events.push(ev.type);
      this.details.push(ev.detail);
    },
    fire(type, ev = {}) {
      for (const fn of [...(listeners[type] || [])]) fn(ev);
    },
    listening: (type) => (listeners[type] || []).length,
  };
}

// Every member of `FenceDeps`, with one fence `f` on an empty stage. The
// calls that change the desk are recorded in `calls`. `floor` replaces
// members of the fake fence list.
function fakeDeps(more = {}, floor = {}) {
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
    fenceFloor: {
      clearFenceFlash() {},
      fenceEl: () => null,
      fenceLocked: () => false,
      fenceNotice: record("fenceNotice"),
      showDetachGlyph() {},
      readFenceRects: () => [{ id: "f", name: "A", rect: RECT }],
      fenceWindows: () => [],
      readWindowRects: () => [],
      refreshFenceChrome() {},
      removeFence: record("removeFence"),
      renameFence: record("renameFence"),
      setFenceLock: record("setFenceLock"),
      renderFences() {},
      renderNotes() {},
      ...floor,
    },
    stage: () => plane,
    applyExtent() {},
    askConfirm: async () => true,
    autoPan: () => ({ follow() {}, stop() {} }),
    fenceSnapshot: () => [{ id: "w1", repo: "r" }],
    focusWin() {},
    glyphClick() {},
    newPid: () => "p1",
    reattachFence: record("reattachFence"),
    saveFences: record("saveFences"),
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

// The console sits in the pane of the tabs, not on the stage: the fence still
// holds it by its desk rect, carries it and writes its new rect.
test("a fence drag carries a console that is in its own tab", () => {
  const win = fakeElement();
  win._deskId = "w1";
  win.classList.add("in-tab");
  win._tabShadow = { style: {} };
  const { deps, calls } = fakeDeps({}, { fenceWindows: () => [win] });
  const { startFenceMove } = createFences(deps);
  startFenceMove(fakeElement(), { id: "f" })({
    button: 0,
    isPrimary: true,
    pointerId: 1,
    pointerType: "mouse",
    clientX: 150,
    clientY: 110,
    preventDefault() {},
    stopPropagation() {},
  });
  deps.document.fire("pointermove", { pointerId: 1, buttons: 1, clientX: 250, clientY: 110 });
  deps.document.fire("pointerup", { pointerId: 1 });
  const wrote = calls.filter((c) => c[0] === "setWin");
  assert.equal(wrote.length, 1);
  assert.equal(wrote[0][1], win);
  assert.equal(wrote[0][2].rect.left, RECT.left + 100);
  assert.equal(win._tabShadow.style.left, `${RECT.left + 100}px`, "its shadow goes with it");
});

test("a fence opens as columns without the console that is in its own tab", () => {
  const windows = [
    { id: "w1", rect: RECT, inTab: true },
    { id: "w2", rect: RECT },
  ];
  const { deps } = fakeDeps({}, { readWindowRects: () => windows });
  const { buildFence } = createFences(deps);
  const fence = buildFence({ id: "f", name: "A", rect: RECT });
  findByClass(fence, "fence-columns").listeners.click();
  const at = deps.document.events.indexOf("workbench:fence-columns");
  assert.deepEqual(deps.document.details[at].items.map((w) => w.id), ["w2"]);
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
  const { deps, calls } = fakeDeps({}, { fenceEl: () => fence });
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

// A fence already detached, with a popup entry `more` describes.
function detachedFence(more) {
  const { deps, calls } = fakeDeps();
  deps.popups.put("f", { ...deps.popups.newPopupEntry(), ...more });
  deps.popups.commitDetached(["f"]);
  return { deps, calls, detachFence: createFences(deps).detachFence };
}

test("a second detach of a fence whose popup is open raises that popup", (t) => {
  const emitted = withWB(t);
  const raised = [];
  const { deps, calls, detachFence } = detachedFence({ handle: { closed: false, focus: () => raised.push("f") }, pid: "p9" });
  detachFence("f");
  assert.deepEqual(raised, ["f"]);
  assert.deepEqual(calls, [], "no message goes through the channel");
  assert.equal(deps.popups.size(), 1);
  assert.deepEqual(emitted, [{ type: "fence-focus", detail: { fence: "f" } }]);
});

test("a second detach of a fence whose popup handle is closed asks the popup through the channel", (t) => {
  const emitted = withWB(t);
  const raised = [];
  const { calls, detachFence } = detachedFence({ handle: { closed: true, focus: () => raised.push("f") }, pid: "p9" });
  detachFence("f");
  assert.deepEqual(raised, []);
  assert.deepEqual(calls, [["post", { type: "origin-focus", tab: "t1", fenceId: "f", pid: "p9" }]]);
  assert.deepEqual(emitted, [{ type: "fence-focus", detail: { fence: "f" } }]);
});

test("a detached popup that is closed later brings its fence home at the next beat", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  withWB(t);
  const handle = { closed: false };
  const { deps, calls } = fakeDeps({ window: { open: () => handle } });
  const { detachFence } = createFences(deps);
  detachFence("f");
  t.mock.timers.tick(500);
  assert.deepEqual(calls.filter(([name]) => name === "reattachFence"), [], "an open popup is left alone");
  handle.closed = true;
  t.mock.timers.tick(500);
  assert.deepEqual(calls.filter(([name]) => name === "reattachFence"), [["reattachFence", "f"]]);
});

test("a popup that never greets is brought home after five seconds, and one that did is left", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "setTimeout"] });
  withWB(t);
  const { deps, calls } = fakeDeps({ window: { open: () => ({ closed: false }) } });
  const { detachFence } = createFences(deps);
  detachFence("f");
  t.mock.timers.tick(4999);
  assert.deepEqual(calls.filter(([name]) => name === "reattachFence"), []);
  t.mock.timers.tick(1);
  assert.deepEqual(calls.filter(([name]) => name === "reattachFence"), [["reattachFence", "f"]]);
  const greeted = fakeDeps({ window: { open: () => ({ closed: false }) } });
  createFences(greeted.deps).detachFence("f");
  greeted.deps.popups.entry("f").greeted = true;
  t.mock.timers.tick(5000);
  assert.deepEqual(greeted.calls.filter(([name]) => name === "reattachFence"), []);
});
