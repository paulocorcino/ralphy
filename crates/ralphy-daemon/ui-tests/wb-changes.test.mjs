// Unit tests for assets/ui/wb-changes.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-changes.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBChanges;
}

const REPLY = {
  status: "ok",
  changes: {
    changes: [
      { path: "README.md", original_path: null, status: "modified" },
      { path: "added.txt", original_path: null, status: "added" },
      { path: "new.txt", original_path: "old.txt", status: "renamed" },
    ],
  },
};

test("a change set folds to its count and one entry per row", () => {
  const folded = load().fold(REPLY);
  assert.equal(folded.count, 3);
  assert.deepEqual(
    folded.entries.map((e) => e.path),
    ["README.md", "added.txt", "new.txt"],
  );
  assert.deepEqual(folded.entries[2], {
    path: "new.txt",
    originalPath: "old.txt",
    status: "renamed",
  });
  assert.equal(folded.entries[0].originalPath, null);
});

test("an empty change set is zero, not absent", () => {
  const folded = load().fold({ status: "ok", changes: { changes: [] } });
  assert.equal(folded.count, 0);
  assert.deepEqual(folded.entries, []);
});

test("an error, absent or malformed reply reads as zero — never a stale count", () => {
  const fold = load().fold;
  for (const reply of [
    { status: "error", message: "query read failed" },
    undefined,
    null,
    {},
    { status: "ok", changes: {} },
    { status: "ok", changes: { changes: "not-an-array" } },
  ]) {
    const folded = fold(reply);
    assert.equal(folded.count, 0, `count for ${JSON.stringify(reply)}`);
    assert.deepEqual(folded.entries, []);
  }
});
