// Unit tests for assets/ui/wb-view.ts — the per-client view record's read
// normalisation. Runs the real source against a `localStorage` that holds one
// string: the module touches nothing else at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBView } from "../assets/ui/wb-view.ts";

// The module reads the bare `localStorage` when `read` or `patch` runs, so
// the fake is installed on `globalThis` for the calls that follow it.
function useStorage(storage) {
  Object.defineProperty(globalThis, "localStorage", { value: storage, configurable: true, writable: true });
}

function load(raw) {
  useStorage({
    getItem: () => raw,
    setItem() {},
  });
  return WBView;
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

// The column list (ADR-0051 §8, columns amendment) is window ids only: any
// other entry drops, and a value that is not a list is "no columns".
test("columns reads as a list of strings, or null", () => {
  assert.deepEqual(load(JSON.stringify({ v: 1, columns: ["a", 2, "b"] })).read().columns, ["a", "b"]);
  assert.equal(load(JSON.stringify({ v: 1, columns: "x" })).read().columns, null);
  assert.equal(load(JSON.stringify({ v: 1 })).read().columns, null, "absent is null, not undefined");
});

// Rows (ADR-0051 §8, rows amendment): a column is a list of ids; its other
// entries drop, and a column that is neither a list nor an id drops.
test("columns reads a grid of ids, next to the flat list stored before rows", () => {
  const read = (columns) => load(JSON.stringify({ v: 1, columns })).read().columns;
  assert.deepEqual(read([["a", 1, "b"], ["c"]]), [["a", "b"], ["c"]]);
  assert.deepEqual(read(["a", ["b", "c"], 3, { x: 1 }]), ["a", ["b", "c"]]);
});

test("columnDir reads right or down, anything else is null", () => {
  const read = (columnDir) => load(JSON.stringify({ v: 1, columnDir })).read().columnDir;
  assert.equal(read("down"), "down");
  assert.equal(read("right"), "right");
  assert.equal(read("up"), null);
  assert.equal(load(JSON.stringify({ v: 1 })).read().columnDir, null, "absent is null, not undefined");
});

// The startup-command setting is gone: an old record's `command` is not read,
// and the next write leaves it out.
test("a legacy command is dropped on read and on the next patch", () => {
  let written = null;
  useStorage({
    getItem: () => JSON.stringify({ v: 1, command: "htop", keys: "on" }),
    setItem: (_key, value) => {
      written = JSON.parse(value);
    },
  });
  assert.equal("command" in WBView.read(), false);
  WBView.patch({ relaunch: true });
  assert.equal("command" in written, false);
  assert.equal(written.keys, "on", "the other fields survive");
});
