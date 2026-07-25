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
    indexStatus: null,
    worktreeStatus: null,
    name: "new.txt",
    dir: "",
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
    indexStatus: null,
    worktreeStatus: null,
    name: "new name.txt",
    dir: "",
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
    // The groups (#315) must go empty with the entries — a stale group would
    // render rows under a headline while the badge already reads `—`.
    assert.deepEqual(folded.staged, [], `staged for ${JSON.stringify(reply)}`);
    assert.deepEqual(folded.unstaged, [], `unstaged for ${JSON.stringify(reply)}`);
  }
});

test("a file staged and then modified again is in both groups (#315)", () => {
  const folded = load().fold({
    status: "ok",
    changes: {
      changes: [
        {
          path: "both.txt",
          original_path: null,
          status: "added",
          index_status: "added",
          worktree_status: "modified",
        },
        {
          path: "staged.txt",
          original_path: null,
          status: "added",
          index_status: "added",
          worktree_status: null,
        },
        {
          path: "dirty.txt",
          original_path: null,
          status: "modified",
          index_status: null,
          worktree_status: "modified",
        },
      ],
    },
  });
  assert.deepEqual(
    folded.staged.map((e) => e.path),
    ["both.txt", "staged.txt"],
  );
  assert.deepEqual(
    folded.unstaged.map((e) => e.path),
    ["both.txt", "dirty.txt"],
  );
  // The badge counts PATHS, not rows: `both.txt` renders twice and counts once.
  assert.equal(folded.count, 3);
});

test("a row splits into base name and dimmed directory (#315)", () => {
  const folded = load().fold({
    status: "ok",
    changes: {
      changes: [
        { path: "docs/deep/nested/readme.md", original_path: null, status: "modified" },
        { path: "README.md", original_path: null, status: "modified" },
      ],
    },
  });
  // An `indexOf` split would leave `deep/nested/readme.md` as the name.
  assert.equal(folded.entries[0].name, "readme.md");
  assert.equal(folded.entries[0].dir, "docs/deep/nested");
  assert.equal(folded.entries[1].name, "README.md");
  assert.equal(folded.entries[1].dir, "");
  // The full repo-relative path stays the hover title.
  assert.equal(folded.entries[0].title, "docs/deep/nested/readme.md");
});

test("an out-of-vocabulary side status still groups and still marks ? (#315)", () => {
  const folded = load().fold({
    status: "ok",
    changes: {
      changes: [
        {
          path: "weird.txt",
          original_path: null,
          status: "vaporized",
          index_status: "vaporized",
          worktree_status: null,
        },
      ],
    },
  });
  assert.equal(folded.entries[0].mark, "?");
  assert.equal(folded.entries[0].cls, "st-unknown");
  assert.deepEqual(
    folded.staged.map((e) => e.path),
    ["weird.txt"],
  );
  assert.deepEqual(folded.unstaged, []);
});

test("an entry carrying neither side field still renders, under Changes (#315)", () => {
  // An older daemon (or a payload that lost the fields) must not make a row
  // vanish from a list whose badge still counts it.
  const folded = load().fold({
    status: "ok",
    changes: { changes: [{ path: "legacy.txt", original_path: null, status: "modified" }] },
  });
  assert.equal(folded.count, 1);
  assert.deepEqual(folded.staged, []);
  assert.deepEqual(
    folded.unstaged.map((e) => e.path),
    ["legacy.txt"],
  );
});

test("a conflict groups under Changes, never under Staged (#315)", () => {
  // The producer's decision, mirrored here: an unresolved conflict is worktree
  // work, so it must never read as "what a commit would contain".
  const folded = load().fold({
    status: "ok",
    changes: {
      changes: [
        {
          path: "merge.txt",
          original_path: null,
          status: "conflicted",
          index_status: null,
          worktree_status: "conflicted",
        },
      ],
    },
  });
  assert.deepEqual(folded.staged, []);
  assert.deepEqual(
    folded.unstaged.map((e) => e.path),
    ["merge.txt"],
  );
  assert.equal(folded.unstaged[0].mark, "!");
});

test("fold does not mutate the reply it is given", () => {
  const reply = {
    status: "ok",
    changes: {
      changes: [
        {
          path: "docs/a.txt",
          original_path: null,
          status: "added",
          index_status: "added",
          worktree_status: "modified",
        },
      ],
    },
  };
  const before = structuredClone(reply);
  load().fold(reply);
  assert.deepEqual(reply, before, "fold stays pure — no DOM, no fetch, no mutation");
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

test("diffTarget resolves both sides of one changes entry (#311)", () => {
  const diffTarget = load().diffTarget;
  const P = "owner/repo";

  // A rename diffs the OLD path at HEAD against the NEW path on disk: reading
  // `entry.path` at HEAD would report the whole file as added.
  const renamed = diffTarget(
    { path: "new.txt", originalPath: "old.txt", status: "renamed" },
    P,
  );
  assert.equal(renamed.headPath, "old.txt");
  assert.equal(renamed.workingPath, "new.txt");
  assert.equal(renamed.headAbsent, false);
  assert.equal(renamed.workingAbsent, false);

  // Added / untracked: nothing at HEAD, so the original side is empty.
  for (const status of ["added", "untracked"]) {
    const t = diffTarget({ path: "brand-new.txt", originalPath: null, status }, P);
    assert.equal(t.headAbsent, true, `${status} has no HEAD side`);
    assert.equal(t.workingAbsent, false);
    assert.equal(t.headPath, "brand-new.txt");
  }

  // Deleted: still at HEAD, gone from the working tree.
  const deleted = diffTarget({ path: "gone.txt", originalPath: null, status: "deleted" }, P);
  assert.equal(deleted.workingAbsent, true);
  assert.equal(deleted.headAbsent, false);

  // The id is stable per project+path — that is what makes a second click on the
  // same row activate the open tab instead of stacking a duplicate.
  for (const entry of [
    { path: "new.txt", originalPath: "old.txt", status: "renamed" },
    { path: "src/deep/mod.rs", originalPath: null, status: "modified" },
  ]) {
    const t = diffTarget(entry, P);
    assert.equal(t.id, `diff:${P}:${entry.path}`);
    assert.equal(t.title, entry.path.split("/").pop() + " ↔ HEAD");
  }
});
