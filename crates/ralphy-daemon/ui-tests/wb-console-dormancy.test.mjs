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
