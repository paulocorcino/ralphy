// Unit tests for the badge, group, commit and discard rules of assets/ui/wb-changes.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBChanges } from "../assets/ui/wb-changes.ts";

// #317 — the Projects-view per-project indicator. One slug in, one badge out:
// an aggregate over every registered repo is structurally impossible here.
test("projectBadge shows the count of a dirty tree, and nothing otherwise (#317)", () => {
  const hidden = { show: false, text: "", title: "" };
  // [case, counts, expected] — a hidden badge is `text: ""`, never absent:
  // `x-text` writes `undefined` into the DOM verbatim.
  const rows = [
    ["a slug nobody read", {}, hidden],
    ["a failed read shows nothing, never a zero", { a: null }, hidden],
    ["a clean tree", { a: 0 }, hidden],
    ["a dirty tree", { a: 3 }, { show: true, text: "3" }],
  ];
  for (const [name, counts, want] of rows) {
    const badge = WBChanges.projectBadge(counts, "a");
    const got = Object.fromEntries(Object.keys(want).map((k) => [k, badge[k]]));
    assert.deepEqual(got, want, name);
  }
});

test("projectBadge cannot aggregate across repos (#317)", () => {
  const projectBadge = WBChanges.projectBadge;
  const counts = { a: 2, b: 3 };
  // The anti-aggregate property as a unit assertion: a many-entry map must still
  // answer per slug. A regression to `sum(Object.values(counts))` reads 5 here.
  assert.equal(projectBadge(counts, "a").text, "2");
  assert.equal(projectBadge(counts, "b").text, "3");
  // …and a slug absent from a POPULATED map still claims nothing.
  assert.equal(projectBadge(counts, "c").show, false);
});

test("groupPaths emits both sides of a rename, de-duplicated (#318)", () => {
  const { fold, groupPaths } = WBChanges;
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
  const { fold, groupPaths } = WBChanges;
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
  const groupPaths = WBChanges.groupPaths;
  assert.deepEqual(groupPaths([]), []);
  assert.deepEqual(groupPaths(), []);
  assert.deepEqual(groupPaths(null), []);
  // Nothing is synthesised: no "." , no "*", no "-A".
  assert.deepEqual(groupPaths([{ path: "a.txt", originalPath: null }]), ["a.txt"]);
  // A malformed entry contributes nothing rather than an `undefined` token.
  assert.deepEqual(groupPaths([{}, { path: 7 }, { path: "" }, { path: "ok" }]), ["ok"]);
});

test("commitTarget names the branch the commit lands on (#318)", () => {
  const commitTarget = WBChanges.commitTarget;
  const landing = commitTarget({ state: "tracking", branch: "main" });
  assert.equal(landing.label, "Commit to main");
  assert.equal(landing.branch, "main");
  assert.equal(landing.detached, false);
  // A branch with no upstream still has a name to land on.
  assert.equal(commitTarget({ state: "no-upstream", branch: "feat/x" }).label, "Commit to feat/x");
});

test("commitTarget refuses to name a detached HEAD as a branch (#318)", () => {
  const commitTarget = WBChanges.commitTarget;
  const detached = commitTarget({ state: "detached", branch: "abc1234" });
  assert.equal(detached.label, "Commit (detached HEAD)");
  assert.equal(detached.detached, true);
  // An unknown fold says the least it can rather than inventing a target.
  for (const bad of [null, undefined, {}, { state: "unknown" }]) {
    assert.equal(commitTarget(bad).label, "Commit");
    assert.equal(commitTarget(bad).detached, false);
  }
});

test("pushAct offers to publish a branch with no upstream", () => {
  const pushAct = WBChanges.pushAct;
  assert.equal(pushAct({ state: "no-upstream", branch: "feat/x" }).label, "Publish branch");
  assert.match(pushAct({ state: "no-upstream", branch: "feat/x" }).title, /track it/u);
  for (const s of [{ state: "tracking", branch: "main" }, { state: "detached" }, { state: "unknown" }, null, undefined]) {
    assert.equal(pushAct(s).label, "Push");
    assert.equal(pushAct(s).title, "Push this branch to the remote");
  }
});

test("pullBlocked closes Pull only for a branch with no upstream", () => {
  const pullBlocked = WBChanges.pullBlocked;
  assert.match(pullBlocked({ state: "no-upstream", branch: "feat/x" }), /no upstream/u);
  for (const s of [{ state: "tracking", branch: "main" }, { state: "detached" }, { state: "unknown" }, null, undefined]) {
    assert.equal(pullBlocked(s), "");
  }
});

test("writeLockReason speaks only when a run holds the lock (#318)", () => {
  const writeLockReason = WBChanges.writeLockReason;
  assert.equal(writeLockReason([]), "");
  assert.equal(writeLockReason(), "");
  assert.equal(writeLockReason(null), "");
  assert.equal(writeLockReason("nope"), "");
  // The subject is a parameter because the same lock closes the board's label
  // editor, which needs the same sentence about a different control.
  assert.match(
    writeLockReason([{ runid: "x" }], "You can edit labels again when it finishes."),
    /^A run is active in this project\. You can edit labels again when it finishes\.$/u,
  );
  assert.equal(writeLockReason([], "You can edit labels again when it finishes."), "");
  const held = writeLockReason([{ runid: "x" }]);
  assert.match(held, /A run is active in this project/u);
  assert.equal(writeLockReason([{ runid: "x" }, { runid: "y" }]), held);
});

test("discardConfirm names the file in a tracked discard (#319)", () => {
  const discardConfirm = WBChanges.discardConfirm;
  const c = discardConfirm({ path: "src/app.js", name: "app.js", status: "modified" });
  assert.match(c.message, /app\.js/u);
  assert.equal(c.danger, true);
  assert.equal(c.unrecoverable, false);
  // A tracked discard keeps the staged blob and the commits, so it must NOT
  // borrow the untracked case's unrecoverability.
  assert.doesNotMatch(c.message, /no commit and no reflog/u);
  assert.match(c.message, /Staged changes for this file are kept/u);
});

test("discardConfirm is more emphatic for the untracked case (#319)", () => {
  const discardConfirm = WBChanges.discardConfirm;
  const tracked = discardConfirm({ path: "a.txt", name: "a.txt", status: "modified" });
  const loose = discardConfirm({ path: "fresh.txt", name: "fresh.txt", status: "untracked" });
  assert.match(loose.message, /fresh\.txt/u);
  assert.match(loose.message, /never committed and cannot be recovered/u);
  assert.equal(loose.unrecoverable, true);
  // The emphasis is a RELATION between the two dialogs, not a literal look: a
  // build that collapsed them into one wording would red here.
  assert.notEqual(loose.title, tracked.title);
  assert.notEqual(loose.confirmLabel, tracked.confirmLabel);
  assert.equal(tracked.unrecoverable, false);
});

test("discardConfirm names an untracked directory entry (#319)", () => {
  const discardConfirm = WBChanges.discardConfirm;
  // git reports an untracked DIRECTORY as ONE entry `newdir/`, whose split name
  // is the empty string — the dialog must still name something.
  const c = discardConfirm({ path: "newdir/", name: "", status: "untracked" });
  assert.match(c.message, /newdir\//u);
  assert.match(c.message, /never committed and cannot be recovered/u);
});

test("groupDiscardNote states what each group's discard removes (#319)", () => {
  const groupDiscardNote = WBChanges.groupDiscardNote;
  const unstaged = groupDiscardNote("unstaged");
  const staged = groupDiscardNote("staged");
  assert.ok(unstaged.length > 0 && staged.length > 0);
  assert.notEqual(unstaged, staged);
  assert.match(unstaged, /Staged changes are kept/u);
  assert.match(staged, /Unstage first/u);
  assert.equal(groupDiscardNote("nope"), "");
  assert.equal(groupDiscardNote(), "");
});
