// Unit tests for assets/ui/wb-console-dormancy.ts — sleep, wake, the cover
// rule and the resume. `createDormancy` is driven with stubs and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDormancy } from "../assets/ui/wb-console-dormancy.ts";

// A page that records every listener added to it, as `<target>:<type>`.
function page() {
  const added = [];
  const handlers = {};
  const target = (name) => ({
    addEventListener(type, handler) {
      added.push(`${name}:${type}`);
      handlers[`${name}:${type}`] = handler;
    },
  });
  return { added, handlers, document: target("document"), window: target("window") };
}

function dormancy(p, wins = new Set()) {
  return createDormancy({
    window: p.window,
    document: p.document,
    wins,
    stack: { setFocusHook() {} },
    sync: { hasPending: () => false },
    staleProbeNow: () => null,
    applyKeyBar() {},
    budget: () => ({ scheduleGpu() {} }),
    fillsViewport: () => false,
    isFull: () => false,
    windowShown: () => true,
    attachTerminal: () => null,
    scheduleDeskFlush() {},
    revivePlaceholders: async () => {},
  });
}

test("the dormancy registers no listener until listen(), then the page listeners in order", () => {
  const p = page();
  const d = dormancy(p);
  assert.deepEqual(p.added, []);
  d.listen();
  // A page without `visualViewport` (the popup, this harness) gets no inset
  // publisher.
  assert.deepEqual(p.added, [
    "document:visibilitychange",
    "window:pagehide",
    "window:online",
    "document:workbench:action",
    "document:workbench:tab-shown",
  ]);
});

test("a hidden page tells each console, and a visible one takes it back before the resume", () => {
  const p = page();
  const said = [];
  const win = { _term: { notePage: (hidden) => said.push(hidden ? "hidden" : "shown"), resume: () => said.push("resume") } };
  dormancy(p, new Set([win])).listen();
  p.document.visibilityState = "hidden";
  p.handlers["document:visibilitychange"]();
  p.document.visibilityState = "visible";
  p.handlers["document:visibilitychange"]();
  p.handlers["window:pagehide"]();
  assert.deepEqual(said, ["hidden", "shown", "resume", "hidden"]);
});

// A file tab hides the Consoles tab with `visibility: hidden`, so the observer
// still calls every console visible. The tab change itself re-asks the rule.
test("hiding the Consoles tab puts its consoles to sleep, and showing it wakes them", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page();
  const disposed = [];
  const attached = [];
  const classes = new Set();
  const body = { replaceWith() {} };
  const win = {
    _visible: true,
    _termWiring: {},
    _term: { sessionId: 7, watching: false, dispose: (why) => disposed.push(why) },
    classList: { contains: (c) => classes.has(c), add: (c) => classes.add(c), remove: (c) => classes.delete(c) },
    querySelector: () => body,
  };
  let shown = true;
  const d = createDormancy({
    window: p.window,
    document: { ...p.document, createElement: () => ({}) },
    wins: new Set([win]),
    stack: { setFocusHook() {} },
    sync: { hasPending: () => false },
    staleProbeNow: () => null,
    applyKeyBar() {},
    budget: () => ({ scheduleGpu() {} }),
    fillsViewport: () => false,
    isFull: () => false,
    windowShown: () => shown,
    attachTerminal: (_body, opts) => {
      attached.push(opts.id);
      return { sessionId: opts.id };
    },
    scheduleDeskFlush() {},
    revivePlaceholders: async () => {},
  });
  d.listen();
  shown = false;
  p.handlers["document:workbench:tab-shown"]({ detail: { id: "file:a.md" } });
  t.mock.timers.tick(15000);
  assert.equal(win._dormant, true);
  assert.deepEqual(disposed, ["dormant"]);
  shown = true;
  p.handlers["document:workbench:tab-shown"]({ detail: { id: "consoles" } });
  assert.deepEqual(attached, [7]);
  assert.equal(win._dormant, false);
});
