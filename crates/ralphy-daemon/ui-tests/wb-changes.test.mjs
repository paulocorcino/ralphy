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
  // …and each of its two rows is marked by ITS OWN side: `A` staged, `M` under
  // Changes. The derived status (`added`) must not leak into the unstaged row.
  assert.equal(folded.staged[0].mark, "A");
  assert.equal(folded.unstaged[0].mark, "M");
  assert.equal(folded.unstaged[0].cls, "st-modified");
  // The flat `entries` model keeps the derived status untouched.
  assert.equal(folded.entries[0].mark, "A");
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

// ---- foldSync (#316) --------------------------------------------------------

const NOW = Date.parse("2026-07-25T12:00:00Z");
const syncReply = (body) => ({ status: "ok", sync: { sync: body } });
const tracking = (ahead, behind, lastFetch = null) =>
  syncReply({
    head: { kind: "branch", name: "main" },
    tracking: { upstream: "origin/main", ahead, behind },
    last_fetch: lastFetch,
  });

test("foldSync reads the branch, its upstream and the counts (#316)", () => {
  const s = load().foldSync(tracking(1, 2), NOW);
  assert.equal(s.state, "tracking");
  assert.equal(s.branch, "main");
  assert.equal(s.upstream, "origin/main");
  assert.equal(s.ahead, 1);
  assert.equal(s.behind, 2);
  assert.equal(s.counts, "↑1 ↓2");
  assert.equal(s.note, "", "a tracking branch needs no note");
});

test("foldSync marks a branch with no upstream (#316)", () => {
  // The whole point: an absent upstream must never render as `↑0 ↓0`, which
  // would claim the branch is in sync with something it does not track.
  const s = load().foldSync(
    syncReply({ head: { kind: "branch", name: "wip" }, tracking: null, last_fetch: null }),
    NOW,
  );
  assert.equal(s.state, "no-upstream");
  assert.equal(s.branch, "wip");
  assert.equal(s.note, "no upstream");
  assert.equal(s.counts, "", "no counts without an upstream");
});

test("foldSync marks a detached HEAD (#316)", () => {
  const s = load().foldSync(
    syncReply({ head: { kind: "detached", sha: "0018522" }, tracking: null, last_fetch: null }),
    NOW,
  );
  assert.equal(s.state, "detached");
  assert.equal(s.branch, "0018522");
  assert.equal(s.note, "detached HEAD");
  assert.equal(s.counts, "");
});

test("foldSync never throws on a malformed or failed frame (#316)", () => {
  const foldSync = load().foldSync;
  for (const reply of [
    null,
    undefined,
    {},
    { status: "error", message: "refused" },
    { status: "ok" },
    { status: "ok", sync: {} },
    { status: "ok", sync: { sync: {} } },
    { status: "ok", sync: { sync: { head: "main" } } },
  ]) {
    const s = foldSync(reply, NOW);
    assert.equal(s.state, "unknown", `frame ${JSON.stringify(reply)}`);
    assert.equal(s.counts, "");
    assert.equal(s.note, "sync unavailable");
    assert.equal(s.fetched, "");
  }
});

test("foldSync labels how stale the counts are (#316)", () => {
  const foldSync = load().foldSync;
  const at = (ms) => new Date(NOW - ms).toISOString();
  const cases = [
    [null, "never fetched"],
    ["not-a-date", "fetch time unknown"],
    [at(30 * 1000), "fetched just now"],
    [at(5 * 60 * 1000), "fetched 5m ago"],
    [at(2 * 3600 * 1000), "fetched 2h ago"],
    [at(3 * 86400 * 1000), "fetched 3d ago"],
  ];
  for (const [stamp, expected] of cases) {
    assert.equal(foldSync(tracking(0, 0, stamp), NOW).fetched, expected, `stamp ${stamp}`);
  }
});

test("foldSync stays pure (#316)", () => {
  const reply = tracking(1, 2, "2026-07-25T11:00:00Z");
  const before = structuredClone(reply);
  load().foldSync(reply, NOW);
  assert.deepEqual(reply, before, "foldSync stays pure — no DOM, no fetch, no mutation");
});

// #317 — the Projects-view per-project indicator. One slug in, one badge out:
// an aggregate over every registered repo is structurally impossible here.
test("projectBadge hides itself for a slug nobody read (#317)", () => {
  // `text: ""`, never absent — `x-text` writes `undefined` into the DOM verbatim.
  assert.deepEqual(load().projectBadge({}, {}, "a"), {
    show: false,
    text: "",
    zero: false,
    title: "",
  });
});

test("projectBadge cannot aggregate across repos (#317)", () => {
  const projectBadge = load().projectBadge;
  const counts = { a: 2, b: 3 };
  // The anti-aggregate property as a unit assertion: a many-entry map must still
  // answer per slug. A regression to `sum(Object.values(counts))` reads 5 here.
  assert.equal(projectBadge(counts, {}, "a").text, "2");
  assert.equal(projectBadge(counts, {}, "b").text, "3");
  // …and a slug absent from a POPULATED map still claims nothing.
  assert.equal(projectBadge(counts, {}, "c").show, false);
});

test("projectBadge shows an em dash, never a zero, for a failed read (#317)", () => {
  const badge = load().projectBadge({ a: null }, { a: "could not read changes" }, "a");
  assert.equal(badge.show, true);
  assert.equal(badge.text, "—");
  assert.equal(badge.zero, false);
  assert.equal(badge.title, "could not read changes");
});

test("projectBadge marks a clean tree as a quiet zero (#317)", () => {
  const badge = load().projectBadge({ a: 0 }, {}, "a");
  assert.equal(badge.show, true);
  assert.equal(badge.text, "0");
  assert.equal(badge.zero, true);
});

test("projectBadge prints the count of a dirty tree (#317)", () => {
  const badge = load().projectBadge({ a: 3 }, {}, "a");
  assert.equal(badge.show, true);
  assert.equal(badge.text, "3");
  assert.equal(badge.zero, false);
});

test("groupPaths emits both sides of a rename, de-duplicated (#318)", () => {
  const { fold, groupPaths } = load();
  const folded = fold({
    status: "ok",
    changes: {
      changes: [
        { path: "new.txt", original_path: "old.txt", status: "renamed", index_status: "renamed" },
        { path: "both.txt", status: "added", index_status: "added", worktree_status: "modified" },
      ],
    },
  });
  // UNSTAGE asks for both paths: `git restore --staged` needs the old one to
  // undo the deletion half of a staged rename.
  assert.deepEqual(groupPaths(folded.staged, true), ["new.txt", "old.txt", "both.txt"]);
  // `both.txt` sits in BOTH groups, so folding the two lists together must not
  // send it twice.
  assert.deepEqual(groupPaths([...folded.staged, ...folded.unstaged], true), [
    "new.txt",
    "old.txt",
    "both.txt",
  ]);
});

test("groupPaths never sends a rename's old path on the STAGE direction (#318)", () => {
  const { fold, groupPaths } = load();
  // `RM`: renamed in the index, then edited on disk — so it lands in the
  // UNSTAGED group carrying `originalPath`. After `git mv a b` the old path is
  // in neither the index nor the worktree, so `git add a` is fatal AND aborts
  // the whole invocation, which made the group's `+` stage nothing at all.
  const folded = fold({
    status: "ok",
    changes: {
      changes: [
        {
          path: "renamed.txt",
          original_path: "old.txt",
          status: "renamed",
          index_status: "renamed",
          worktree_status: "modified",
        },
        { path: "other.txt", status: "modified", worktree_status: "modified" },
      ],
    },
  });
  assert.equal(folded.unstaged.length, 2);
  // The default (stage) omits the original path entirely…
  assert.deepEqual(groupPaths(folded.unstaged), ["renamed.txt", "other.txt"]);
  assert.deepEqual(groupPaths(folded.unstaged, false), ["renamed.txt", "other.txt"]);
  // …and only an explicit opt-in brings it back.
  assert.deepEqual(groupPaths(folded.unstaged, true), ["renamed.txt", "old.txt", "other.txt"]);
});

test("groupPaths can only name paths it was given (#318)", () => {
  const groupPaths = load().groupPaths;
  assert.deepEqual(groupPaths([]), []);
  assert.deepEqual(groupPaths(undefined), []);
  assert.deepEqual(groupPaths(null), []);
  // Nothing is synthesised: no "." , no "*", no "-A".
  assert.deepEqual(groupPaths([{ path: "a.txt", originalPath: null }]), ["a.txt"]);
  // A malformed entry contributes nothing rather than an `undefined` token.
  assert.deepEqual(groupPaths([{}, { path: 7 }, { path: "" }, { path: "ok" }]), ["ok"]);
});

test("commitTarget names the branch the commit lands on (#318)", () => {
  const commitTarget = load().commitTarget;
  const tracking = commitTarget({ state: "tracking", branch: "main" });
  assert.equal(tracking.label, "Commit to main");
  assert.equal(tracking.branch, "main");
  assert.equal(tracking.detached, false);
  // A branch with no upstream still has a name to land on.
  assert.equal(commitTarget({ state: "no-upstream", branch: "feat/x" }).label, "Commit to feat/x");
});

test("commitTarget refuses to name a detached HEAD as a branch (#318)", () => {
  const commitTarget = load().commitTarget;
  const detached = commitTarget({ state: "detached", branch: "abc1234" });
  assert.equal(detached.label, "Commit (detached HEAD)");
  assert.equal(detached.detached, true);
  // An unknown fold says the least it can rather than inventing a target.
  for (const bad of [null, undefined, {}, { state: "unknown" }]) {
    assert.equal(commitTarget(bad).label, "Commit");
    assert.equal(commitTarget(bad).detached, false);
  }
});

test("writeLockReason speaks only when a run holds the lock (#318)", () => {
  const writeLockReason = load().writeLockReason;
  assert.equal(writeLockReason([]), "");
  assert.equal(writeLockReason(undefined), "");
  assert.equal(writeLockReason(null), "");
  assert.equal(writeLockReason("nope"), "");
  // The subject is a parameter because the same lock closes the board's label
  // editor, which needs the same sentence about a different control.
  assert.match(
    writeLockReason([{ runid: "x" }], "labels are read-only until it finishes"),
    /^a run holds this repo's lock — labels are read-only until it finishes$/,
  );
  assert.equal(writeLockReason([], "labels are read-only until it finishes"), "");
  const held = writeLockReason([{ runid: "x" }]);
  assert.match(held, /holds this repo's lock/);
  assert.equal(writeLockReason([{ runid: "x" }, { runid: "y" }]), held);
});

test("discardConfirm names the file in a tracked discard (#319)", () => {
  const discardConfirm = load().discardConfirm;
  const c = discardConfirm({ path: "src/app.js", name: "app.js", status: "modified" });
  assert.match(c.message, /app\.js/);
  assert.equal(c.danger, true);
  assert.equal(c.unrecoverable, false);
  // A tracked discard keeps the staged blob and the commits, so it must NOT
  // borrow the untracked case's unrecoverability.
  assert.doesNotMatch(c.message, /no commit and no reflog/);
  assert.match(c.message, /staged for this file is kept/);
});

test("discardConfirm is more emphatic for the untracked case (#319)", () => {
  const discardConfirm = load().discardConfirm;
  const tracked = discardConfirm({ path: "a.txt", name: "a.txt", status: "modified" });
  const loose = discardConfirm({ path: "fresh.txt", name: "fresh.txt", status: "untracked" });
  assert.match(loose.message, /fresh\.txt/);
  assert.match(loose.message, /no commit and no reflog can bring it back/);
  assert.equal(loose.unrecoverable, true);
  // The emphasis is a RELATION between the two dialogs, not a literal look: a
  // build that collapsed them into one wording would red here.
  assert.notEqual(loose.title, tracked.title);
  assert.notEqual(loose.confirmLabel, tracked.confirmLabel);
  assert.equal(tracked.unrecoverable, false);
});

test("discardConfirm names an untracked directory entry (#319)", () => {
  const discardConfirm = load().discardConfirm;
  // git reports an untracked DIRECTORY as ONE entry `newdir/`, whose split name
  // is the empty string — the dialog must still name something.
  const c = discardConfirm({ path: "newdir/", name: "", status: "untracked" });
  assert.match(c.message, /newdir\//);
  assert.match(c.message, /no commit and no reflog can bring it back/);
});

test("groupDiscardNote states what each group's discard removes (#319)", () => {
  const groupDiscardNote = load().groupDiscardNote;
  const unstaged = groupDiscardNote("unstaged");
  const staged = groupDiscardNote("staged");
  assert.ok(unstaged.length > 0 && staged.length > 0);
  assert.notEqual(unstaged, staged);
  assert.match(unstaged, /staged changes are kept/);
  assert.match(staged, /unstage first/);
  assert.equal(groupDiscardNote("nope"), "");
  assert.equal(groupDiscardNote(undefined), "");
});

test("the discard folds are pure (#319)", () => {
  const { discardConfirm, groupDiscardNote } = load();
  const entry = { path: "a.txt", name: "a.txt", status: "untracked" };
  const snapshot = JSON.stringify(entry);
  discardConfirm(entry);
  groupDiscardNote("unstaged");
  assert.equal(JSON.stringify(entry), snapshot, "the input entry is unmodified");
  // …and no input at all is answered, never thrown on.
  assert.equal(typeof discardConfirm(undefined).message, "string");
});
