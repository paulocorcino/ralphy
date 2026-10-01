// The real JS folds run on the replies Rust tests wrote to `fixtures/`
// (ADR-0070 Compliance, shared replies). A field renamed on either side fails
// here: the Rust test rewrites the file only with UPDATE_GOLDEN=1, and then
// this file reads the new name.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { loadShell } from "./harness.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

const NOW = Date.parse("2026-07-03T12:00:00Z");

test("changes.list: the change-set fold reads every row the CLI wrote", () => {
  const { window } = loadShell();
  const reply = fixture("changes.list");
  const folded = window.WBChanges.fold(reply);
  assert.ok(folded.count > 0, "the fixture holds a change");
  assert.equal(folded.count, reply.changes.changes.length);
  assert.deepEqual(
    folded.entries.map((e) => e.path),
    reply.changes.changes.map((r) => r.path),
  );
  assert.equal(folded.entries[0].indexStatus, reply.changes.changes[0].index_status);
  assert.equal(folded.entries[0].worktreeStatus, reply.changes.changes[0].worktree_status);
});

test("sync.status: the sync fold reads branch, upstream and counts", () => {
  const { window } = loadShell();
  const s = window.WBChanges.foldSync(fixture("sync.status"), NOW);
  assert.equal(s.state, "tracking");
  assert.equal(s.branch, "main");
  assert.equal(s.upstream, "origin/main");
  assert.equal(s.ahead, 0);
  assert.equal(s.behind, 1);
});

test("sync.status: a detached HEAD shows its sha", () => {
  const { window } = loadShell();
  const s = window.WBChanges.foldSync(fixture("sync.status--detached"), NOW);
  assert.equal(s.state, "detached");
  assert.equal(s.branch, "<sha>");
});

test("sync.status: no upstream is its own state", () => {
  const { window } = loadShell();
  const s = window.WBChanges.foldSync(fixture("sync.status--no-upstream"), NOW);
  assert.equal(s.state, "no-upstream");
  assert.equal(s.counts, "");
});

test("board.list: every board row keeps the fields the board shows", () => {
  const { state } = loadShell();
  const rows = fixture("board.list").board.issues;
  assert.ok(rows.some((r) => r.assignees.length > 0), "a row with assignees");
  assert.ok(rows.some((r) => r.blocked_by.length > 0), "a row with blockers");
  for (const row of rows) {
    const i = state.boardRowToIssue(row);
    assert.equal(i.number, row.number);
    assert.equal(i.title, row.title);
    assert.deepEqual(i.assignees, row.assignees);
    assert.deepEqual(i.blockedBy, row.blocked_by);
    assert.equal(i.created, row.created);
    assert.equal(i.updated, row.updated);
    assert.equal(i.state, row.state);
    assert.deepEqual(i.labels, row.labels);
  }
});

test("issue.show: the issue view takes the body and comments of the reply", async () => {
  const { state, window } = loadShell();
  const reply = fixture("issue.show");
  assert.ok(reply.issue.body.length > 0, "the fixture holds a body");
  window.WBDaemon.observe = async () => reply;
  window.WBMode.isDaemon = () => true;
  state._flashAction = () => {};
  state.openSlug = "o/r";
  state.kanbanSel = reply.issue.number;
  state.boardIssues["o/r"] = [state.boardRowToIssue({ number: reply.issue.number })];
  await state.loadIssueDetail(reply.issue.number);
  const row = state.boardIssues["o/r"][0];
  assert.equal(state.issueError, null);
  assert.equal(row.body, reply.issue.body);
  assert.deepEqual(row.comments, reply.issue.comments);
  assert.deepEqual(row.blockedBy, reply.issue.blocked_by);
});

test("host.aliases: picking an alias fills the host form from the reply", () => {
  const { window } = loadShell();
  const H = window.WBHosts;
  let s = H.next(H.initial(), { type: "aliases", aliases: fixture("host.aliases").aliases });
  s = H.next(s, { type: "pick", alias: "lab" });
  assert.equal(s.alias, "lab");
  assert.equal(s.address, "10.0.0.5");
  assert.equal(s.user, "deploy");
  assert.equal(s.port, "2222");
});

test("host.key: an unknown key moves the form to the identity step", () => {
  const { window } = loadShell();
  const H = window.WBHosts;
  const key = fixture("host.key").key;
  const s = H.next(H.initial(), { type: "key", key });
  assert.equal(s.step, "identity");
  assert.deepEqual(s.keys, key.keys);
  assert.ok(s.keys[0].fingerprint.startsWith("SHA256:"));
});

test("project.remove: the unknown-repo refusal has its own cause", () => {
  const { window } = loadShell();
  const said = window.WBFail.failed(
    fixture("project.remove--unknown-repo"),
    "Could not remove the project: the daemon gave no reason.",
  );
  assert.equal(said, "Could not remove the project: the project is not in the list.");
});

test("project.remove: the unknown-repo refusal means the project is already gone", async () => {
  const { state, window } = loadShell();
  window.WBDaemon.observe = async () => fixture("project.remove--unknown-repo");
  const flashed = [];
  state._flashAction = (m) => flashed.push(m);
  state.askConfirm = async () => true;
  state.loadRepos = () => {};
  const p = { slug: "nope/gone" };
  state.projects = [p, { slug: "o/r" }];
  await state.removeProject(p);
  assert.deepEqual(
    state.projects.map((x) => x.slug),
    ["o/r"],
  );
  assert.deepEqual(flashed, []);
});
