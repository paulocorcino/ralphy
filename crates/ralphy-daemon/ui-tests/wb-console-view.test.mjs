// Unit tests for assets/ui/wb-console-view.ts — the landing and the stored
// offset, the reveal, the slide and the plane's wheel. `createView` is driven
// with fake `deps`, a fake viewport and a fake stage, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createView } from "../assets/ui/wb-console-view.ts";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";

// Something on the page that keeps its listeners, so a test can fire them.
function listening(fields) {
  const listeners = {};
  return {
    listeners,
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
    ...fields,
  };
}

// The viewport, the stage and the deps. `wins` are the windows on the stage,
// each `{ _deskId, rect }`; `fences` maps a fence id to its element.
function harness({ width = 1000, height = 700, stored = null, settled = false } = {}) {
  const ws = listening({ clientWidth: width, clientHeight: height, scrollLeft: 0, scrollTop: 0, isConnected: true });
  const wins = [];
  const st = listening({
    offsetWidth: 3000,
    offsetHeight: 2000,
    classList: { add() {}, remove() {} },
    querySelectorAll: (sel) => (sel === ".session-window" ? wins : []),
    querySelector: () => null,
  });
  const fences = {};
  const patches = [];
  const focused = [];
  const calls = { syncMaxPin: 0 };
  const state = { stored, settled, focusedFence: null };
  const doc = listening({});
  const view = createView({
    window: listening({ matchMedia: () => ({ matches: false }) }),
    document: doc,
    workspace: () => ws,
    stage: () => st,
    viewStore: {
      read: () => (state.stored ? { off: state.stored } : {}),
      patch: (p) => patches.push(p),
    },
    restoreRect: (el) => el.rect,
    focusWin: (el) => focused.push(el),
    syncFullState() {},
    syncMaxPin: () => {
      calls.syncMaxPin += 1;
    },
    fenceEl: (id) => fences[id] || null,
    focusFence: (id) => {
      state.focusedFence = id;
    },
    clearFenceFocus: () => {
      state.focusedFence = null;
    },
    focusedFence: () => state.focusedFence,
    isDeskSettled: () => state.settled,
  });
  return { view, ws, st, doc, wins, fences, patches, focused, state, calls };
}

// Frames are queued by a stub `requestAnimationFrame` and run by hand; the
// ids `cancelAnimationFrame` took are kept.
function withFrames(body) {
  const queued = [];
  const cancelled = [];
  const saved = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  globalThis.requestAnimationFrame = (cb) => queued.push(cb);
  globalThis.cancelAnimationFrame = (id) => cancelled.push(id);
  try {
    body({ queued, cancelled });
  } finally {
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = saved;
  }
}

test("the landing waits for a stage that holds a window, then applies once", () => {
  const h = harness();
  // An empty stage and a desk not settled: the landing runs but does not latch.
  h.view.applyLanding();
  h.wins.push({ _deskId: "w1", rect: { left: 2000, top: 1200, width: 400, height: 300 } });
  h.view.applyLanding();
  const want = WBGeometry.viewLanding(
    null,
    h.wins.map((w) => w.rect),
    { width: 1000, height: 700 },
    { width: 3000, height: 2000 },
  );
  assert.deepEqual({ left: h.ws.scrollLeft, top: h.ws.scrollTop }, want);
  // The operator pans; with no stored offset a later refit does not re-centre.
  h.ws.scrollLeft = 5;
  h.view.applyLanding();
  assert.equal(h.ws.scrollLeft, 5, "the landing must latch once the stage holds a window");
});

test("the scroll listeners keep their order, and the page-hide flush writes the pending offset", () => {
  const h = harness({ settled: true });
  h.view.applyLanding();
  h.view.wireStage();
  assert.equal(h.ws.listeners.scroll.length, 2);
  const [first, saveOffset] = h.ws.listeners.scroll;
  // The maximize pin first, then the debounced offset.
  first();
  assert.equal(h.calls.syncMaxPin, 1);
  assert.equal(h.patches.length, 0);
  h.ws.scrollLeft = 120;
  h.ws.scrollTop = 80;
  saveOffset();
  assert.equal(h.patches.length, 0, "the offset write is debounced");
  h.view.flushPendingOffset();
  assert.deepEqual(h.patches, [{ off: { left: 120, top: 80 } }]);
  // Nothing is left to flush.
  h.view.flushPendingOffset();
  assert.equal(h.patches.length, 1);
});

test("a reveal on a hidden viewport is parked and wins over the stored offset", () => {
  const h = harness({ width: 0, height: 0, stored: { left: 0, top: 0 }, settled: true });
  const win = { _deskId: "w1", rect: { left: 2200, top: 1500, width: 400, height: 300 }, classList: { contains: () => false } };
  h.wins.push(win);
  assert.equal(h.view.reveal("w1"), win);
  assert.deepEqual(h.focused, [win]);
  assert.equal(h.ws.scrollLeft, 0, "a viewport that measures 0 is not scrolled");
  // The tab shows: the first landing that can measure centres the window.
  h.ws.clientWidth = 1000;
  h.ws.clientHeight = 700;
  h.view.applyLanding();
  const want = WBGeometry.bringIntoView(win.rect, { width: 1000, height: 700 }, { width: 3000, height: 2000 });
  assert.deepEqual({ left: h.ws.scrollLeft, top: h.ws.scrollTop }, want);
  assert.deepEqual(h.patches, [{ off: want }], "the reveal is the new stored view");
});

test("a new slide cancels the slide in flight", () => {
  withFrames(({ queued, cancelled }) => {
    const h = harness({ settled: true });
    h.fences.a = { rect: { left: 1500, top: 900, width: 400, height: 300 } };
    h.fences.b = { rect: { left: 300, top: 1200, width: 400, height: 300 } };
    assert.equal(h.view.jumpToFence("a"), h.fences.a);
    assert.equal(queued.length, 1);
    h.view.jumpToFence("b");
    assert.deepEqual(cancelled, [1], "the first slide's frame must be cancelled");
    assert.equal(h.state.focusedFence, "b");
    // The second slide runs to its end.
    queued[1](performance.now() + 10_000);
    const want = WBGeometry.anchorIntoView(h.fences.b.rect, { width: 1000, height: 700 }, { width: 3000, height: 2000 });
    assert.deepEqual({ left: h.ws.scrollLeft, top: h.ws.scrollTop }, want);
  });
});

test("a shift-wheel in lines pans the plane sideways by lines", () => {
  const h = harness();
  h.view.wireStage();
  const [onWheel] = h.ws.listeners.wheel;
  let prevented = false;
  const wheel = (fields) => ({
    target: { closest: () => null },
    shiftKey: true,
    deltaX: 0,
    preventDefault() {
      prevented = true;
    },
    ...fields,
  });
  onWheel(wheel({ deltaY: 3, deltaMode: 1 }));
  assert.equal(h.ws.scrollLeft, 48, "three lines of 16 pixels");
  assert.ok(prevented);
  onWheel(wheel({ deltaY: 10, deltaMode: 0 }));
  assert.equal(h.ws.scrollLeft, 58, "pixels pass as they are");
  // The terminal owns its own wheel.
  onWheel(wheel({ deltaY: 3, deltaMode: 1, target: { closest: () => ({}) } }));
  assert.equal(h.ws.scrollLeft, 58);
});
