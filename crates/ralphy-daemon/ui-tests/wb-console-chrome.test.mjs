// Unit tests for assets/ui/wb-console-chrome.ts — the console's window chrome
// and the owner of the elements under a gesture. `createChrome` is driven with
// fake `deps` and a fake document, with no console and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createChrome, createGestures } from "../assets/ui/wb-console-chrome.ts";

// An element with the few DOM members the chrome uses. Listeners are kept so a
// test can fire them.
function fakeElement(tag = "div") {
  const listeners = {};
  return {
    tagName: tag,
    className: "",
    style: {},
    hidden: false,
    title: "",
    innerHTML: "",
    textContent: "",
    children: [],
    offsetLeft: 10,
    offsetTop: 10,
    offsetWidth: 400,
    offsetHeight: 300,
    listeners,
    classList: {
      contains: () => false,
      toggle() {},
      add() {},
      remove() {},
    },
    setAttribute() {},
    append(...kids) {
      this.children.push(...kids);
    },
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    querySelector: () => null,
    closest: () => null,
    getBoundingClientRect: () => ({ left: 10, top: 10, width: 400, height: 300 }),
  };
}

// The page's document: elements, and listeners a test fires by type.
function fakeDocument() {
  const listeners = {};
  return {
    fullscreenEnabled: false,
    createElement: (tag) => fakeElement(tag),
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
    dispatchEvent() {},
    fire(type, ev = {}) {
      for (const fn of [...(listeners[type] || [])]) fn(ev);
    },
  };
}

// Every member of `ChromeDeps`; `setWin` records its calls.
function fakeDeps(more = {}) {
  const writes = [];
  const document = fakeDocument();
  const plane = fakeElement();
  plane.offsetWidth = 4000;
  plane.offsetHeight = 3000;
  let ids = 0;
  const deps = {
    window: { addEventListener() {}, removeEventListener() {} },
    document,
    OPTS: {},
    gestures: createGestures(),
    stage: () => plane,
    workspace: () => ({ scrollLeft: 0, scrollTop: 0, clientWidth: 1200, clientHeight: 800 }),
    fences: () => [],
    focusedFence: () => null,
    applyExtent() {},
    applyLock() {},
    isLocked: () => false,
    toggleLock() {},
    autoPan: () => ({ follow() {}, stop() {} }),
    consolePrefix: () => "console",
    takenNames: () => [],
    canRename: () => true,
    startRename() {},
    fenceEl: () => null,
    fenceLocked: () => false,
    focusWin() {},
    reveal() {},
    isFull: () => false,
    toggleFull() {},
    setMax() {},
    toggleMax() {},
    newDeskId: () => "d" + ++ids,
    restoreRect: (win) => ({ left: win.offsetLeft, top: win.offsetTop }),
    sessionPresentation: () => ({ tooltip: "" }),
    renderTitle() {},
    setWin: (win, fields) => writes.push({ win, fields }),
    ...more,
  };
  return { deps, writes, document };
}

// A primary press at a point, for a pointer `pointerId`.
function press(x, y, more = {}) {
  return {
    button: 0,
    isPrimary: true,
    pointerId: 1,
    pointerType: "mouse",
    clientX: x,
    clientY: y,
    target: { closest: () => null },
    preventDefault() {},
    stopPropagation() {},
    ...more,
  };
}

test("a gesture owner tracks each element from begin to end, one element at a time", () => {
  const gestures = createGestures();
  const win = {};
  const fence = {};
  gestures.begin(win);
  gestures.begin(fence);
  assert.equal(gestures.active(win), true);
  assert.equal(gestures.active(fence), true);
  gestures.end(fence);
  assert.equal(gestures.active(fence), false);
  assert.equal(gestures.active(win), true, "ending one gesture leaves the other");
  gestures.end(win);
  assert.equal(gestures.active(win), false);
});

test("a titlebar drag is a gesture from the press to the release, and only a moved drag writes", () => {
  const { deps, writes, document } = fakeDeps();
  const { makeDraggable } = createChrome(deps);
  const win = fakeElement();
  const handle = fakeElement();
  makeDraggable(win, handle);

  // A tap: a gesture while pressed, and nothing written.
  handle.listeners.pointerdown[0](press(100, 100));
  assert.equal(deps.gestures.active(win), true);
  document.fire("pointerup", {});
  assert.equal(deps.gestures.active(win), false);
  assert.deepEqual(writes, []);

  // A drag past the threshold writes the rect once, at the release.
  handle.listeners.pointerdown[0](press(100, 100));
  document.fire("pointermove", { pointerId: 1, buttons: 1, clientX: 160, clientY: 140 });
  document.fire("pointerup", {});
  assert.equal(deps.gestures.active(win), false);
  assert.equal(writes.length, 1);
  assert.deepEqual(Object.keys(writes[0].fields), ["rect"]);
});

test("a resize band is a gesture too, and a tap on it writes nothing", () => {
  const { deps, writes, document } = fakeDeps();
  const { startResize } = createChrome(deps);
  const win = fakeElement();
  const onDown = startResize(win, "se");

  onDown(press(50, 50));
  assert.equal(deps.gestures.active(win), true);
  document.fire("pointerup", {});
  assert.equal(deps.gestures.active(win), false);
  assert.deepEqual(writes, [], "a tap on a band changed nothing");

  onDown(press(50, 50));
  document.fire("pointermove", { pointerId: 1, clientX: 90, clientY: 80 });
  assert.equal(win.style.width, "440px");
  document.fire("pointerup", {});
  assert.equal(writes.length, 1);
});

test("each new window without a record takes the next place in the free cascade", () => {
  const { deps } = fakeDeps();
  const { buildChrome } = createChrome(deps);
  const lefts = [];
  for (let i = 0; i < 3; i++) {
    const { win } = buildChrome("shell", "~", null, "console");
    lefts.push(parseInt(win.style.left, 10));
  }
  assert.ok(lefts[0] < lefts[1] && lefts[1] < lefts[2], `cascade lefts ${lefts}`);
  // A record keeps its own rect and does not take a cascade place.
  const kept = buildChrome("shell", "~", { kind: "console", rect: { left: 7, top: 8, width: 300, height: 200 } }, "console");
  assert.equal(kept.win.style.left, "7px");
  const { win } = buildChrome("shell", "~", null, "console");
  assert.ok(parseInt(win.style.left, 10) > lefts[2]);
});

test("a deps member left out fails the call that needs it", () => {
  const { deps } = fakeDeps();
  delete deps.focusWin;
  const { makeDraggable } = createChrome(deps);
  const handle = fakeElement();
  makeDraggable(fakeElement(), handle);
  assert.throws(() => handle.listeners.pointerdown[0](press(100, 100)), TypeError);
});
