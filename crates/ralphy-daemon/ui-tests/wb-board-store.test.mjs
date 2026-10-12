// Unit tests for assets/ui/wb-board-store.ts, the Alpine store `board`: the
// board's issues and labels of each project and the state of their read. It
// is built here without Alpine (ADR-0073 D3, D6), as `main.ts` builds it for
// `Alpine.store`, on a page stub that gives it the `projects` and
// `connection` stores.
import { test } from "node:test";
import assert from "node:assert/strict";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";
import { boardStore } from "../assets/ui/wb-board-store.ts";

function page() {
  const projects = projectsStore();
  projects.setOpen("o/r");
  const connection = { fmtClock: () => "14:02" };
  const stores = { projects, connection };
  globalThis.window = { Alpine: { store: (name) => stores[name] } };
  const store = boardStore({ messages: { flash() {} } });
  return { store, projects };
}

const row = (number, more = {}) => ({ number, title: `#${number}`, labels: [], body: "", comments: [], blockedBy: [], ...more });

test("a good read holds the issues and the labels, and clears the failure", () => {
  const { store } = page();
  store.boardReadFailed("o/r", "gh not authed");
  const labels = new Map([["bug", "#d73a4a"]]);
  store.boardReadOk("o/r", [row(7)], labels);
  assert.deepEqual(store.boardIssues["o/r"], [row(7)]);
  assert.equal(store.boardLabels["o/r"], labels);
  assert.equal(store.boardError["o/r"], null);
  assert.equal(store.boardRead["o/r"].current, true);
});

test("a failed read before any good read shows no cards and the message", () => {
  const { store } = page();
  store.boardReadFailed("o/r", "gh not authed");
  assert.deepEqual(store.boardIssues["o/r"], []);
  assert.equal(store.boardError["o/r"], "gh not authed");
  assert.equal(store.boardRead["o/r"].current, false);
});

test("a failed read after a good read keeps the cards, marked not current", () => {
  const { store } = page();
  store.boardReadOk("o/r", [row(7)], new Map());
  store.boardReadFailed("o/r", "gh not authed");
  assert.deepEqual(store.boardIssues["o/r"], [row(7)], "the cards stay");
  assert.match(store.boardError["o/r"], /Not current/);
  assert.match(store.boardError["o/r"], /14:02/);
  assert.equal(store.boardRead["o/r"].current, false);
});

test("the detail of an issue merges only the fields of the right type", () => {
  const { store } = page();
  store.boardReadOk("o/r", [row(7)], new Map());
  assert.equal(store.issueDetailRead("o/r", 8, { body: "x" }), false, "no such row");
  assert.equal(store.issueDetailRead("o/r", 7, { body: "the body", comments: [{ body: "c" }], blocked_by: [3] }), true);
  assert.deepEqual(store.boardIssues["o/r"][0], row(7, { body: "the body", comments: [{ body: "c" }], blockedBy: [3] }));
  store.issueDetailRead("o/r", 7, { body: 5, comments: "no", blocked_by: { a: 1 } });
  assert.deepEqual(store.boardIssues["o/r"][0], row(7, { body: "the body", comments: [{ body: "c" }], blockedBy: [3] }));
});

test("the labels of one issue are set, and another issue keeps its own", () => {
  const { store } = page();
  store.boardReadOk("o/r", [row(7), row(8, { labels: ["bug"] })], new Map());
  store.issueLabelsSet("o/r", 7, ["ready"]);
  assert.deepEqual(store.boardIssues["o/r"][0].labels, ["ready"]);
  assert.deepEqual(store.boardIssues["o/r"][1].labels, ["bug"]);
});

test("openIssues follows the open project", () => {
  const { store, projects } = page();
  store.boardReadOk("o/r", [row(7)], new Map());
  store.boardReadOk("o/x", [row(9)], new Map());
  assert.deepEqual(store.openIssues().map((i) => i.number), [7]);
  projects.setOpen("o/x");
  assert.deepEqual(store.openIssues().map((i) => i.number), [9]);
  projects.setOpen("o/none");
  assert.deepEqual(store.openIssues(), []);
});
