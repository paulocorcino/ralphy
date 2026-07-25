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
    mark: "R",
    cls: "st-renamed",
    title: "old.txt → new.txt",
  });
  assert.equal(folded.entries[0].originalPath, null);
});

test("an empty change set is zero, not absent", () => {
  const folded = load().fold({ status: "ok", changes: { changes: [] } });
  assert.equal(folded.count, 0);
  assert.deepEqual(folded.entries, []);
});

test("the list model carries a status marker per row", () => {
  const reply = {
    status: "ok",
    changes: {
      changes: [
        { path: "README.md", original_path: null, status: "modified" },
        { path: "added.txt", original_path: null, status: "added" },
        { path: "gone.txt", original_path: null, status: "deleted" },
        { path: "new name.txt", original_path: "old name.txt", status: "renamed" },
        { path: "docs/café.md", original_path: null, status: "untracked" },
        { path: "merge.txt", original_path: null, status: "conflicted" },
        { path: "weird.txt", original_path: null, status: "vaporized" },
      ],
    },
  };
  const folded = load().fold(reply);
  assert.equal(folded.count, 7);
  assert.deepEqual(
    folded.entries.map((e) => e.mark),
    ["M", "A", "D", "R", "U", "!", "?"],
  );
  assert.deepEqual(folded.entries[3], {
    path: "new name.txt",
    originalPath: "old name.txt",
    status: "renamed",
    mark: "R",
    cls: "st-renamed",
    title: "old name.txt → new name.txt",
  });
  assert.equal(folded.entries[6].cls, "st-unknown");
  assert.equal(folded.entries[0].title, "README.md");
});

test("an error, absent or malformed reply reads as zero — never a stale count", () => {
  const fold = load().fold;
  for (const reply of [
    { status: "error", message: "query read failed" },
    // An error frame that DOES carry rows: folding its body would report a
    // count the daemon never confirmed — status wins over shape.
    { status: "error", message: "boom", changes: { changes: REPLY.changes.changes } },
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

test("shouldReload fires only on a changes.dirty naming the OPEN repo (#310)", () => {
  const shouldReload = load().shouldReload;
  const dirty = (repo) => ({ verb: "changes.dirty", payload: { repo } });
  assert.equal(shouldReload(dirty("owner/a"), "owner/a"), true);
  assert.equal(shouldReload(dirty("owner/b"), "owner/a"), false);
  // The socket carries the watcher's pushes too — only OUR verb reloads.
  assert.equal(
    shouldReload({ verb: "tree.dirty", payload: { repo: "owner/a", path: "" } }, "owner/a"),
    false,
  );
  // No project open: a nudge must not fire a read for a null slug.
  assert.equal(shouldReload(dirty("owner/a"), null), false);
});

test("shouldReload answers false — never throws — on a malformed frame (#310)", () => {
  const shouldReload = load().shouldReload;
  // The daemon is the only sender today, but a throw here dies inside the
  // socket's `onmessage` and kills the nudge path for that connection.
  for (const frame of [
    null,
    undefined,
    {},
    { verb: "changes.dirty" },
    { verb: "changes.dirty", payload: {} },
    { verb: "changes.dirty", payload: { repo: "" } },
  ]) {
    assert.equal(shouldReload(frame, "owner/a"), false, `frame ${JSON.stringify(frame)}`);
  }
  // …and an empty open slug never matches an empty repo.
  assert.equal(shouldReload({ verb: "changes.dirty", payload: { repo: "" } }, ""), false);
});
