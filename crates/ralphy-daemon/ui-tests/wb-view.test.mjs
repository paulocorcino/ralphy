// Unit tests for assets/ui/wb-view.js — the per-client view record's read
// normalisation. Runs the real source against a `localStorage` that holds one
// string: the module touches nothing else at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-view.js"),
  "utf8",
);

function load(raw) {
  const window = {};
  const localStorage = {
    getItem: () => raw,
    setItem() {},
  };
  new Function("window", "localStorage", SRC)(window, localStorage);
  return window.WBView;
}
const record = (split) => JSON.stringify({ v: 1, split });

// The slot (ADR-0037 §3c) is read STRICTLY: the shapes `toStored` writes come
// back as they are, and anything else is "no slot" — a hand-edited or
// half-written record must not reach `--wb-split` or name a tab.
test("a stored pin comes back with its file and ratio; a mirror with its ratio only", () => {
  assert.deepEqual(load(record({ kind: "pin", project: "o/r", path: "b.js", checkout: null, ratio: 0.6 })).read().split, {
    kind: "pin",
    project: "o/r",
    path: "b.js",
    checkout: null,
    ratio: 0.6,
  });
  assert.deepEqual(load(record({ kind: "pin", project: "o/r", path: "b.js", checkout: "wt", ratio: null })).read().split, {
    kind: "pin",
    project: "o/r",
    path: "b.js",
    checkout: "wt",
    ratio: null,
  });
  assert.deepEqual(load(record({ kind: "mirror", ratio: 0.35 })).read().split, { kind: "mirror", ratio: 0.35 });
});

test("a ratio outside the divider's range, or not a number, is dropped — the kind survives", () => {
  assert.equal(load(record({ kind: "mirror", ratio: 5 })).read().split.ratio, null);
  assert.equal(load(record({ kind: "mirror", ratio: 0.19 })).read().split.ratio, null);
  assert.equal(load(record({ kind: "mirror", ratio: 0.81 })).read().split.ratio, null);
  assert.equal(load(record({ kind: "mirror", ratio: "0.5" })).read().split.ratio, null);
  assert.equal(load(record({ kind: "mirror", ratio: 0.2 })).read().split.ratio, 0.2);
  assert.equal(load(record({ kind: "mirror", ratio: 0.8 })).read().split.ratio, 0.8);
});

test("a pin without a file, an unknown kind, or a non-object is no slot", () => {
  for (const bad of [
    { kind: "pin", path: "b.js" },
    { kind: "pin", project: "o/r" },
    { kind: "pin", project: 1, path: "b.js" },
    { kind: "group", id: "x" },
    [{ kind: "mirror" }],
    "mirror",
    7,
    null,
  ]) {
    assert.equal(load(record(bad)).read().split, null, JSON.stringify(bad));
  }
  assert.equal(load(JSON.stringify({ v: 1 })).read().split, null, "absent is null, not undefined");
});

test("a non-string checkout on a pin reads as the primary", () => {
  assert.equal(load(record({ kind: "pin", project: "o/r", path: "b.js", checkout: 3 })).read().split.checkout, null);
});
