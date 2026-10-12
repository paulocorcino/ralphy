// Unit tests for assets/ui/wb-console-fit.ts — the size rule of one console's
// terminal. `createFit` is driven with a fake animation frame, a fake resize
// observer, a fake terminal and a fake socket; the test counts the `resize`
// commands the socket sends.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createFit } from "../assets/ui/wb-console-fit.ts";

// The page globals `createFit` reaches, and a console built on them. `fit`
// of the fake addon grows the grid by one column, as a new body size would.
function fakeConsole({ covered = false, focused = false, watching = false, width = 800, height = 600 } = {}) {
  const frames = [];
  const observers = [];
  const globals = {
    requestAnimationFrame: (cb) => frames.push(cb),
    cancelAnimationFrame: () => {},
    ResizeObserver: class {
      constructor(cb) {
        this.cb = cb;
        observers.push(this);
      }
      observe() {}
      disconnect() {}
    },
    WebSocket: { OPEN: 1 },
  };
  const saved = Object.fromEntries(Object.keys(globals).map((k) => [k, globalThis[k]]));
  Object.assign(globalThis, globals);
  const state = { covered, focused, watching };
  const sent = [];
  const socket = { readyState: 1, send: (data) => sent.push(data) };
  let onResize = () => {};
  // Each `term.resize(cols, rows)` call, as `[cols, rows]`.
  const termResizes = [];
  const term = {
    rows: 24,
    cols: 80,
    onResize: (cb) => {
      onResize = cb;
    },
    resize(cols, rows) {
      termResizes.push([cols, rows]);
      term.cols = cols;
      term.rows = rows;
      onResize({ rows, cols });
    },
  };
  const fitAddon = {
    fit() {
      term.cols += 1;
      onResize({ rows: term.rows, cols: term.cols });
    },
  };
  const classes = new Set();
  const body = {
    clientWidth: width,
    clientHeight: height,
    classList: {
      toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)),
    },
  };
  const fit = createFit({
    term,
    body,
    fitAddon,
    socket: () => socket,
    covered: () => state.covered,
    focused: () => state.focused,
    watching: () => state.watching,
  });
  // Runs the frames requested so far, as the browser does on its next frame.
  const nextFrame = () => {
    for (const cb of frames.splice(0)) cb(0);
  };
  const resizes = () =>
    sent.filter((d) => new TextDecoder().decode(d.subarray(1)).includes('"verb":"resize"')).length;
  const restore = () => Object.assign(globalThis, saved);
  return { fit, state, body, classes, observers, nextFrame, resizes, termResizes, restore };
}

test("a covered console without the focus sends no size on any fit request", () => {
  const c = fakeConsole({ covered: true });
  try {
    c.fit.fit.fit();
    c.observers[0].cb();
    c.nextFrame();
    assert.equal(c.resizes(), 0);
  } finally {
    c.restore();
  }
});

test("a swap that covers the console in the same task sends no size", () => {
  const c = fakeConsole();
  try {
    // The swap un-maximizes this console first: it asks for a fit while it
    // is still uncovered, then the new console covers it.
    c.fit.fit.fit();
    c.state.covered = true;
    c.nextFrame();
    assert.equal(c.resizes(), 0);
  } finally {
    c.restore();
  }
});

test("the cover going sends the kept size exactly once", () => {
  const c = fakeConsole({ covered: true });
  try {
    c.fit.fit.fit();
    c.observers[0].cb();
    c.nextFrame();
    c.state.covered = false;
    c.fit.uncovered();
    c.fit.uncovered();
    c.nextFrame();
    assert.equal(c.resizes(), 1);
  } finally {
    c.restore();
  }
});

test("a body that measures 0×0 sends no size", () => {
  const c = fakeConsole({ width: 0, height: 0 });
  try {
    c.fit.fit.fit();
    c.nextFrame();
    c.fit.opened();
    assert.equal(c.resizes(), 0);
  } finally {
    c.restore();
  }
});

test("a covered console that has the focus still fits and sends its size", () => {
  const c = fakeConsole({ covered: true, focused: true });
  try {
    c.fit.fit.fit();
    c.nextFrame();
    assert.equal(c.resizes(), 1);
  } finally {
    c.restore();
  }
});

test("a watcher takes the session size and sends no size on later fit requests", () => {
  const c = fakeConsole({ watching: true });
  try {
    c.fit.sessionSize(30, 100);
    assert.deepEqual(c.termResizes, [[100, 30]]);
    assert.ok(c.classes.has("following"), "the body scrolls");
    c.fit.fit.fit();
    c.observers[0].cb();
    c.nextFrame();
    c.fit.opened();
    assert.equal(c.resizes(), 0);
    assert.deepEqual(c.termResizes, [[100, 30]]);
  } finally {
    c.restore();
  }
});

test("the writer ignores the session size and keeps its own fit", () => {
  const c = fakeConsole();
  try {
    c.fit.sessionSize(30, 100);
    assert.deepEqual(c.termResizes, []);
    c.fit.fit.fit();
    c.nextFrame();
    assert.equal(c.resizes(), 1);
  } finally {
    c.restore();
  }
});

test("a watcher that takes over fits and sends its size again", () => {
  const c = fakeConsole({ watching: true });
  try {
    c.fit.sessionSize(30, 100);
    c.state.watching = false;
    c.fit.opened();
    assert.equal(c.resizes(), 1);
    assert.ok(!c.classes.has("following"), "the body no longer scrolls");
  } finally {
    c.restore();
  }
});

test("a watcher that gets no session size fits as before", () => {
  const c = fakeConsole({ watching: true });
  try {
    c.fit.fit.fit();
    c.nextFrame();
    assert.equal(c.resizes(), 1);
  } finally {
    c.restore();
  }
});
