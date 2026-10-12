// Unit tests for assets/ui/wb-console-dormancy.ts — sleep, wake, the cover
// rule and the resume. `createDormancy` is driven with stubs and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDormancy } from "../assets/ui/wb-console-dormancy.ts";

// A page that records every listener added to it, as `<target>:<type>`.
function page() {
  const added = [];
  const target = (name) => ({
    addEventListener(type) {
      added.push(`${name}:${type}`);
    },
  });
  return { added, document: target("document"), window: target("window") };
}

function dormancy(p) {
  return createDormancy({
    window: p.window,
    document: p.document,
    wins: new Set(),
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
  assert.deepEqual(p.added, ["document:visibilitychange", "window:online", "document:workbench:action"]);
});
