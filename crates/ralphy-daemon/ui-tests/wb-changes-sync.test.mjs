// Unit tests for the sync fold of assets/ui/wb-changes.ts and the HEAD it reads.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBChanges } from "../assets/ui/wb-changes.ts";

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
  const s = WBChanges.foldSync(tracking(1, 2), NOW);
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
  const s = WBChanges.foldSync(
    syncReply({ head: { kind: "branch", name: "wip" }, tracking: null, last_fetch: null }),
    NOW,
  );
  assert.equal(s.state, "no-upstream");
  assert.equal(s.branch, "wip");
  assert.equal(s.note, "No upstream");
  assert.equal(s.counts, "", "no counts without an upstream");
});

test("foldSync marks a detached HEAD (#316)", () => {
  const s = WBChanges.foldSync(
    syncReply({ head: { kind: "detached", sha: "0018522" }, tracking: null, last_fetch: null }),
    NOW,
  );
  assert.equal(s.state, "detached");
  assert.equal(s.branch, "0018522");
  assert.equal(s.note, "Detached HEAD");
  assert.equal(s.counts, "");
});

test("foldSync reads a tracking that is not an object as no upstream", () => {
  const s = WBChanges.foldSync(syncReply({ head: { kind: "branch", name: "wip" }, tracking: "origin/wip" }), NOW);
  assert.equal(s.state, "no-upstream");
  assert.equal(s.upstream, "");
  assert.equal(s.counts, "");
});

test("foldSync reads a count that is not a number as 0", () => {
  const s = WBChanges.foldSync(
    syncReply({
      head: { kind: "branch", name: "main" },
      tracking: { upstream: "origin/main", ahead: "many", behind: null },
    }),
    NOW,
  );
  assert.equal(s.state, "tracking");
  assert.equal(s.ahead, 0);
  assert.equal(s.behind, 0);
  assert.equal(s.counts, "↑0 ↓0");
});

test("foldSync never throws on a malformed or failed frame (#316)", () => {
  const foldSync = WBChanges.foldSync;
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
    assert.equal(s.note, "Sync status unknown");
    assert.equal(s.fetched, "");
  }
});

test("headBranch gives the project row the HEAD a sync read saw", () => {
  const { foldSync, headBranch } = WBChanges;
  assert.equal(headBranch(foldSync(tracking(0, 0), NOW)), "main");
  assert.equal(
    headBranch(foldSync(syncReply({ head: { kind: "branch", name: "wip" }, tracking: null }), NOW)),
    "wip",
  );
  // Same shape as `/api/repos`: a detached HEAD has no branch.
  assert.equal(
    headBranch(foldSync(syncReply({ head: { kind: "detached", sha: "0018522" }, tracking: null }), NOW)),
    "",
  );
  // A failed read says nothing about HEAD: the row keeps what it holds.
  assert.equal(headBranch(foldSync(null, NOW)), null);
  assert.equal(headBranch(null), null);
});

test("headOf gives the project row the head a sync read saw, in the /api/repos shape", () => {
  const { foldSync, headOf } = WBChanges;
  assert.deepEqual(
    headOf(foldSync(syncReply({ head: { kind: "detached", sha: "abc1234" }, tracking: null }), NOW)),
    { kind: "detached", sha: "abc1234" },
  );
  assert.deepEqual(headOf(foldSync(tracking(0, 0), NOW)), { kind: "branch", name: "main" });
  assert.equal(headOf(foldSync(null, NOW)), null);
});

const at = (ms) => new Date(NOW - ms).toISOString();

test("foldSync labels how stale the counts are (#316)", () => {
  const foldSync = WBChanges.foldSync;
  const cases = [
    [null, "Never fetched"],
    ["not-a-date", "Fetch time unknown"],
    [at(30 * 1000), "Fetched a moment ago"],
    [at(5 * 60 * 1000), "Fetched 5m ago"],
    [at(2 * 3600 * 1000), "Fetched 2h ago"],
    [at(3 * 86400 * 1000), "Fetched 3d ago"],
  ];
  for (const [stamp, expected] of cases) {
    assert.equal(foldSync(tracking(0, 0, stamp), NOW).fetched, expected, `stamp ${stamp}`);
  }
});
