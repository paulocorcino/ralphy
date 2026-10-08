// Unit tests for assets/ui/wb-console-gpu.ts — the console's dormancy watch and
// GPU budget. `gpuHolders` is pure; `createGpuBudget` is driven with fake
// `deps` (windows, observer, microtask queue), with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DORMANT_MARGIN_PX,
  GPU_BUDGET,
  createGpuBudget,
  gpuHolders,
} from "../assets/ui/wb-console-gpu.ts";

// --- gpuHolders: which windows hold one of the page's WebGL contexts ---------
// Chrome keeps 16 live contexts per renderer process and drops the oldest past
// that, so a cascade of 20 seen consoles lost four. The page hands out a
// budget, to the seen, uncovered windows on top.

test("gpuHolders gives the budget to the seen, uncovered windows on top", () => {
  const win = (z, more = {}) => ({ seen: true, covered: false, hasTerminal: true, z, ...more });
  // [case, windows, budget, expected indexes]
  const rows = [
    ["the highest z first, cut at the budget", [win(1), win(5), win(3), win(4)], 2, [1, 3]],
    ["fewer candidates than the budget: all of them", [win(1), win(2)], 12, [1, 0]],
    [
      "a window not seen, covered or without a terminal never holds one, even on top",
      [win(9, { seen: false }), win(8, { covered: true }), win(7, { hasTerminal: false }), win(1)],
      2,
      [3],
    ],
    ["ties keep the input order", [win(2), win(2), win(2)], 2, [0, 1]],
  ];
  for (const [name, windows, budget, want] of rows) {
    assert.deepEqual(gpuHolders(windows, budget), want, name);
  }
});

// --- createGpuBudget: the watch and the coalesced rebalance -------------------

// A console window as the budget sees it: a z-index and a terminal that
// records whether it holds a context.
function fakeWindow(z) {
  return {
    style: { zIndex: String(z) },
    _term: {
      gpu: null,
      useGpu() {
        this.gpu = true;
      },
      dropGpu() {
        this.gpu = false;
      },
    },
  };
}

// Every member of `GpuBudgetDeps`, with a microtask queue the test runs by hand.
function fakeDeps(more = {}) {
  const tasks = [];
  const deps = {
    wins: new Set(),
    workspace: () => ({ id: "workspace" }),
    isCovered: () => false,
    applyDormancy: () => "stay",
    IntersectionObserver: undefined,
    queueMicrotask: (task) => tasks.push(task),
    ...more,
  };
  const runTasks = () => {
    while (tasks.length) tasks.shift()();
  };
  return { deps, tasks, runTasks };
}

// An `IntersectionObserver` that keeps its callback, options and targets.
function fakeObserverClass() {
  const made = [];
  class FakeObserver {
    constructor(callback, options) {
      this.callback = callback;
      this.options = options;
      this.targets = new Set();
      made.push(this);
    }
    observe(el) {
      this.targets.add(el);
    }
    unobserve(el) {
      this.targets.delete(el);
    }
  }
  return { FakeObserver, made };
}

test("a tracked window is observed from the viewport, and a report wakes the dormancy rule", () => {
  const { FakeObserver, made } = fakeObserverClass();
  const root = { id: "workspace" };
  const asked = [];
  const { deps } = fakeDeps({
    IntersectionObserver: FakeObserver,
    workspace: () => root,
    applyDormancy: (win) => asked.push(win),
  });
  const budget = createGpuBudget(deps);
  const win = fakeWindow(1);
  deps.wins.add(win);
  budget.trackDormancy(win);

  assert.equal(made.length, 1, "one observer for the console");
  const [observer] = made;
  assert.ok(observer.targets.has(win), "the window is observed");
  assert.equal(observer.options.root, root);
  assert.equal(observer.options.rootMargin, `${DORMANT_MARGIN_PX}px`);
  assert.equal(budget.dormancyWatch(), observer, "the watch is built once");

  observer.callback([{ target: win, isIntersecting: true }]);
  assert.equal(win._visible, true);
  assert.deepEqual(asked, [win], "the report asks the dormancy rule");

  observer.callback([{ target: win, isIntersecting: false }]);
  assert.equal(win._visible, false, "a report that leaves the viewport marks it unseen");
  assert.deepEqual(asked, [win, win], "that report asks the dormancy rule too");

  budget.untrackDormancy(win);
  assert.ok(!observer.targets.has(win), "an untracked window is no longer held");
});

test("with no observer a tracked window counts as seen and the budget rebalances", () => {
  // One more window than the budget, plus a covered one on top of them all.
  const windows = Array.from({ length: GPU_BUDGET + 1 }, (_, i) => fakeWindow(i + 1));
  const covered = fakeWindow(1000);
  const { deps, runTasks } = fakeDeps({ isCovered: (w) => w === covered });
  const budget = createGpuBudget(deps);
  assert.equal(budget.dormancyWatch(), null, "no observer: the watch is inert");
  for (const w of [...windows, covered]) {
    deps.wins.add(w);
    budget.trackDormancy(w);
  }
  runTasks();

  for (const w of [...windows, covered]) assert.equal(w._visible, true);
  assert.equal(windows[0]._term.gpu, false, "the lowest window draws with the DOM renderer");
  for (const w of windows.slice(1)) assert.equal(w._term.gpu, true, "the top windows hold one");
  assert.equal(covered._term.gpu, false, "a covered window never holds one");
});

test("many scheduleGpu calls in one turn give one rebalance", () => {
  const { deps, tasks, runTasks } = fakeDeps();
  let reads = 0;
  deps.isCovered = () => {
    reads += 1;
    return false;
  };
  const win = fakeWindow(1);
  win._visible = true;
  deps.wins.add(win);
  const budget = createGpuBudget(deps);

  budget.scheduleGpu();
  budget.scheduleGpu();
  budget.untrackDormancy(fakeWindow(2));
  assert.equal(tasks.length, 1, "one queued rebalance");
  runTasks();
  assert.equal(reads, 1, "one rebalance read the window once");

  budget.scheduleGpu();
  assert.equal(tasks.length, 1, "after it ran, the next call queues again");
});

// The page globals are the reads `tsc` cannot see; it guards every other read
// through the `GpuBudgetDeps` type.
test("a budget uses its deps, not the page's IntersectionObserver or queueMicrotask", () => {
  const savedObserver = globalThis.IntersectionObserver;
  const savedMicrotask = globalThis.queueMicrotask;
  let globalReads = 0;
  globalThis.IntersectionObserver = class {
    constructor() {
      globalReads += 1;
    }
  };
  globalThis.queueMicrotask = () => {
    globalReads += 1;
  };
  try {
    // The page's observer and queue exist, but these deps give none: the
    // budget must not reach for the page's.
    const { deps, tasks } = fakeDeps();
    const budget = createGpuBudget(deps);
    assert.equal(budget.dormancyWatch(), null);
    budget.scheduleGpu();
    assert.equal(tasks.length, 1, "the deps queue took the rebalance");
    assert.equal(globalReads, 0, "the page's observer and queue were not used");
  } finally {
    globalThis.IntersectionObserver = savedObserver;
    globalThis.queueMicrotask = savedMicrotask;
  }

  // A member left out fails the call that needs it.
  const missing = (name) => {
    const { deps, runTasks } = fakeDeps();
    deps.wins.add(Object.assign(fakeWindow(1), { _visible: true }));
    delete deps[name];
    const budget = createGpuBudget(deps);
    return () => {
      budget.scheduleGpu();
      runTasks();
    };
  };
  assert.throws(missing("queueMicrotask"), TypeError);
  assert.throws(missing("isCovered"), TypeError);
  assert.throws(missing("wins"), TypeError);
});
