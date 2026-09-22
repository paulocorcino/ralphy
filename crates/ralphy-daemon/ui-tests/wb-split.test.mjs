// Unit tests for assets/ui/wb-split.js — the secondary pane's decision table
// (ADR-0037 §3c). Runs the real source against an empty window: the module is
// pure and touches nothing at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-split.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBSplit;
}

const A = "file:p:a.js";
const B = "file:p:b.js";
const MD = "file:p:README.md";
const TABS = [
  { id: "consoles", kind: "consoles", closable: false },
  { id: A, kind: "code", closable: true, project: "p", path: "a.js", checkout: null },
  { id: B, kind: "code", closable: true, project: "p", path: "b.js", checkout: null },
  { id: MD, kind: "markdown", closable: true, project: "p", path: "README.md", checkout: null },
  { id: "diff:p:x", kind: "diff", closable: true, project: "p", path: "x" },
];
const WIDE = 1280;
const base = (over) => ({ active: A, slot: null, tabs: TABS, lastLeft: null, width: WIDE, paneless: false, ...over });

test("no slot, or a paneless tab, is the single pane the canvas always had", () => {
  const { resolve } = load();
  assert.deepEqual(resolve(base()), { left: A, right: null, mirror: false, focus: "left" });
  assert.deepEqual(resolve(base({ active: "consoles", paneless: true, slot: { kind: "pin", id: B } })), {
    left: null,
    right: null,
    mirror: false,
    focus: "left",
  });
});

test("a pin puts the pinned tab on the right of whatever is active", () => {
  const { resolve } = load();
  assert.deepEqual(resolve(base({ slot: { kind: "pin", id: B } })), {
    left: A,
    right: B,
    mirror: false,
    focus: "left",
  });
  assert.deepEqual(resolve(base({ active: MD, slot: { kind: "pin", id: B } })), {
    left: MD,
    right: B,
    mirror: false,
    focus: "left",
  });
});

test("activating the pinned tab focuses the right pane; the left keeps the last tab read there", () => {
  const { resolve } = load();
  assert.deepEqual(resolve(base({ active: B, slot: { kind: "pin", id: B }, lastLeft: MD })), {
    left: MD,
    right: B,
    mirror: false,
    focus: "right",
  });
  // No memory of a left tab: the nearest other tab with a pane.
  assert.deepEqual(resolve(base({ active: B, slot: { kind: "pin", id: B } })), {
    left: A,
    right: B,
    mirror: false,
    focus: "right",
  });
  // The pinned tab is the ONLY tab: nothing to split against.
  const only = TABS.filter((t) => t.id === "consoles" || t.id === B);
  assert.deepEqual(resolve(base({ active: B, slot: { kind: "pin", id: B }, tabs: only })), {
    left: B,
    right: null,
    mirror: false,
    focus: "left",
  });
});

test("a pin of a tab that is gone paints single; the state is the caller's to clear", () => {
  const { resolve, afterClose } = load();
  assert.equal(resolve(base({ slot: { kind: "pin", id: "file:p:gone" } })).right, null);
  assert.equal(afterClose({ kind: "pin", id: B }, B), null);
  assert.deepEqual(afterClose({ kind: "pin", id: B }, A), { kind: "pin", id: B });
  assert.deepEqual(afterClose({ kind: "mirror" }, A), { kind: "mirror" });
  assert.equal(afterClose(null, A), null);
});

test("a mirror doubles the active CODE tab and steps aside for anything else", () => {
  const { resolve } = load();
  assert.deepEqual(resolve(base({ slot: { kind: "mirror" } })), {
    left: A,
    right: A,
    mirror: true,
    focus: "left",
  });
  assert.deepEqual(resolve(base({ active: MD, slot: { kind: "mirror" } })), {
    left: MD,
    right: null,
    mirror: false,
    focus: "left",
  });
  assert.equal(resolve(base({ active: "diff:p:x", slot: { kind: "mirror" } })).mirror, false);
});

test("below the width floor the split is unavailable, whatever the slot says", () => {
  const { resolve, available, MIN_WIDTH } = load();
  assert.equal(available(MIN_WIDTH), true);
  assert.equal(available(MIN_WIDTH - 1), false);
  assert.equal(available(undefined), false);
  assert.deepEqual(resolve(base({ slot: { kind: "pin", id: B }, width: 800 })), {
    left: A,
    right: null,
    mirror: false,
    focus: "left",
  });
  assert.equal(resolve(base({ slot: { kind: "mirror" }, width: 0 })).mirror, false);
});

test("the divider ratio is the pointer's share, held so neither pane drops under the pane floor", () => {
  const { clampRatio, MIN_PANE, DEFAULT_RATIO } = load();
  assert.equal(clampRatio(500, 1000), 0.5);
  assert.equal(clampRatio(10, 1000), MIN_PANE / 1000);
  assert.equal(clampRatio(990, 1000), 1 - MIN_PANE / 1000);
  assert.equal(clampRatio(100, 400), 0.5); // floor and ceiling meet: a 400px canvas splits only in the middle
  assert.equal(clampRatio(100, 4000), 0.2); // a wide canvas still keeps a fifth: the range the store accepts back
  assert.equal(clampRatio(100, 0), DEFAULT_RATIO);
});

test("a pin stores as the file the stored tabs spell, and comes back as that tab", () => {
  const { toStored, fromStored } = load();
  const stored = toStored({ kind: "pin", id: B }, 0.6, TABS);
  assert.deepEqual(stored, { kind: "pin", project: "p", path: "b.js", checkout: null, ratio: 0.6 });
  assert.deepEqual(fromStored(stored, TABS), { kind: "pin", id: B });
  assert.deepEqual(toStored({ kind: "mirror" }, null, TABS), { kind: "mirror", ratio: null });
  assert.deepEqual(fromStored({ kind: "mirror", ratio: 0.4 }, TABS), { kind: "mirror" });
});

test("a pin of a diff, of a missing tab, or of nothing stores as null — and restores as null", () => {
  const { toStored, fromStored } = load();
  assert.equal(toStored({ kind: "pin", id: "diff:p:x" }, 0.5, TABS), null);
  assert.equal(toStored({ kind: "pin", id: "file:p:gone" }, 0.5, TABS), null);
  assert.equal(toStored(null, 0.5, TABS), null);
  assert.equal(fromStored({ kind: "pin", project: "p", path: "gone.js", checkout: null }, TABS), null);
  assert.equal(fromStored({ kind: "pin", project: "p", path: "b.js", checkout: "wt" }, TABS), null);
  assert.equal(fromStored({ kind: "nope" }, TABS), null);
  assert.equal(fromStored(null, TABS), null);
});
