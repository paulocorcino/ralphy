// The list beside a maximized console in assets/ui/wb-columns.ts: its fold of
// consoles and fences, its labels and its filter.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBColumns } from "../assets/ui/wb-columns.ts";
// The label builder the title uses; the column row must call the same one.
import { WBConsoleName } from "../assets/ui/wb-console-name.ts";

const columnRow = (id, extra = {}) => ({
  id,
  agent: "claude",
  repo: "C:/r",
  kind: "agent",
  running: true,
  state: null,
  ...extra,
});

function roster() {
  return {
    rows: [
      columnRow("L1"),
      columnRow("F1a"),
      columnRow("L2", { running: false, name: "backend" }),
      columnRow("F2a", { state: "working" }),
      columnRow("W", { state: "working", name: "W name", detail: "No hook event came" }),
    ],
    // f2 is listed FIRST, though f1 comes first in `membership`.
    fences: [
      { id: "f2", name: "Two" },
      { id: "f1", name: "One" },
      { id: "f3", name: "Empty" },
      { id: "f4", name: "Away" },
    ],
    membership: { f1: ["F1a"], f2: ["F2a"], f3: [], f4: [] },
    detached: { f4: [{ id: "D1", agent: "codex", name: "home #1", repo: null, kind: "agent" }] },
    columns: ["L1", "F2a"],
    from: "L1",
  };
}

test("listFold: loose first, then fences in the Fence menu order, empty fences left out", () => {
  const C = WBColumns;
  const groups = C.listFold(roster());
  assert.deepEqual(
    groups.map((g) => (g.fence ? g.fence.id : null)),
    [null, "f2", "f1", "f4"],
  );
  assert.deepEqual(groups[2].fence, { id: "f1", name: "One" });
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["L1", "L2", "W"]);
});

// The maximized console opens the list: without its row it looked gone.
test("listFold: the console that opened the list is a row that cannot be opened or swapped", () => {
  const C = WBColumns;
  const self = C.listFold(roster()).flatMap((g) => g.rows).find((r) => r.id === "L1");
  assert.deepEqual(
    { enabled: self.enabled, reason: self.reason, swappable: self.swappable },
    { enabled: false, reason: C.REASON_SELF, swappable: false },
  );
  assert.equal(C.REASON_SELF, "This console");
});

test("listFold: rows carry agent, repo, state and the reason they are disabled", () => {
  const C = WBColumns;
  const rows = new Map(C.listFold(roster()).flatMap((g) => g.rows).map((r) => [r.id, r]));
  const f2a = rows.get("F2a");
  assert.equal(f2a.enabled, false);
  assert.equal(f2a.reason, "Already in a column");
  assert.equal(f2a.state, "working", "state is carried through");
  const l2 = rows.get("L2");
  assert.equal(l2.enabled, true, "a console that is not running can be opened");
  assert.equal(l2.running, false);
  const f1a = rows.get("F1a");
  assert.equal(f1a.enabled, true);
  assert.equal(f1a.reason, null);
  assert.equal(f1a.agent, "claude");
  assert.equal(f1a.repo, "C:/r");
  const d1 = rows.get("D1");
  assert.deepEqual(
    { enabled: d1.enabled, reason: d1.reason, running: d1.running, state: d1.state },
    { enabled: false, reason: "In a detached fence", running: true, state: null },
  );
  assert.equal(d1.agent, "codex");
  assert.equal(d1.name, "home #1", "a detached row carries its console name");
  assert.equal(rows.get("W").name, "W name", "a loose row carries its console name");
  assert.equal(rows.get("W").detail, "No hook event came", "a row carries the detail of its dot");
  assert.equal(f1a.detail, null);
  assert.equal(d1.detail, null);
});

test("listFold: a detached fence the fence list does not name adds no group", () => {
  const C = WBColumns;
  const r = roster();
  r.detached = { gone: [{ id: "Z", agent: "a", repo: null, kind: "agent" }] };
  const ids = C.listFold(r).flatMap((g) => g.rows.map((x) => x.id));
  assert.ok(!ids.includes("Z"));
});

// ADR-0066 §4: every row names its console, so no group head prints a repo,
// even when every row of the group shares one.
test("listFold: a group is its fence and its rows, with no shared repo", () => {
  const C = WBColumns;
  for (const g of C.listFold(roster())) {
    assert.deepEqual(Object.keys(g).toSorted(), ["fence", "rows"]);
    assert.ok(!("shared" in g) && !("repo" in g));
  }
});

test("rowLabel: the console label builder, as in the title", () => {
  const C = WBColumns;
  const N = WBConsoleName;
  assert.equal(C.rowLabel({ name: "fincal #1", agent: "claude", repo: "o/r" }, N.consoleLabel), "fincal #1 (claude)");
  assert.equal(C.rowLabel({ name: "home #2", agent: "console", repo: null }, N.consoleLabel), "home #2 (console)");
});

const ids = (gs) => gs.flatMap((g) => g.rows.map((r) => r.id));

const label = (ref) => (ref === "C:/r" ? "owner/ralphy · WSL: Ubuntu" : ref);

test("filterGroups: console name, agent, repo text and fence name match; empty groups drop", () => {
  const C = WBColumns;
  const groups = C.listFold(roster());
  assert.equal(C.filterGroups(groups, "", label), groups, "no query: the same list");
  assert.equal(C.filterGroups(groups, "   ", label), groups);
  assert.deepEqual(ids(C.filterGroups(groups, "CODEX", label)), ["D1"], "agent, case-insensitive");
  assert.deepEqual(ids(C.filterGroups(groups, "wsl", label)).toSorted(), ["F1a", "F2a", "L1", "L2", "W"], "repo text");
  const byName = C.filterGroups(groups, "two", label);
  assert.deepEqual(byName.map((g) => g.fence?.id), ["f2"], "a fence name keeps its whole group");
  assert.deepEqual(ids(C.filterGroups(groups, "back", label)), ["L2"], "console name");
  assert.deepEqual(C.filterGroups(groups, "nothing", label), []);
});

test("listFold: at the cap a row cannot open a column but can be swapped in", () => {
  const C = WBColumns;
  const r = roster();
  r.full = true;
  const rows = new Map(C.listFold(r).flatMap((g) => g.rows).map((x) => [x.id, x]));
  const l2 = rows.get("L2");
  assert.deepEqual(
    { enabled: l2.enabled, reason: l2.reason, swappable: l2.swappable },
    { enabled: false, reason: "No room for another console", swappable: true },
  );
  assert.equal(rows.get("F2a").reason, "Already in a column", "an open row keeps its own reason");
  assert.equal(rows.get("F2a").swappable, true, "a column can change places");
  assert.equal(rows.get("D1").swappable, false, "a detached fence's console cannot be swapped in");
});

test("listFold: the row that cannot be swapped is the column that opened the list, not the leftmost", () => {
  const C = WBColumns;
  const r = roster();
  r.from = "F2a";
  const rows = new Map(C.listFold(r).flatMap((g) => g.rows).map((x) => [x.id, x]));
  assert.equal(rows.get("F2a").reason, C.REASON_SELF);
  assert.equal(rows.get("L1").swappable, true, "the leftmost can be swapped with");
});

// Membership comes from a fold that gives each window one fence, but the list
// does not trust that: a window named by two fences is listed once, under the
// fence `membership` names first.
test("listFold: a window in two fences is listed once, under the first fence", () => {
  const C = WBColumns;
  const groups = C.listFold({
    rows: [{ id: "W", agent: "claude", kind: "agent" }],
    fences: [
      { id: "f2", name: "Two" },
      { id: "f1", name: "One" },
    ],
    membership: { f1: ["W"], f2: ["W"] },
  });
  assert.deepEqual(
    groups.map((g) => [g.fence?.id ?? null, g.rows.map((r) => r.id)]),
    [["f1", ["W"]]],
  );
});

test("listFold: missing lists fold to nothing, and a missing name, repo or state reads null", () => {
  const C = WBColumns;
  assert.deepEqual(C.listFold({ rows: null, fences: null, membership: null, detached: null, columns: null }), []);
  const groups = C.listFold({
    rows: [{ id: "A", agent: "claude", kind: "agent" }],
    fences: [{ id: "f", name: "F" }],
    detached: { f: [{ id: "D", agent: "codex", kind: "agent" }] },
  });
  const rows = new Map(groups.flatMap((g) => g.rows).map((r) => [r.id, r]));
  const a = rows.get("A");
  assert.deepEqual(
    { name: a.name, repo: a.repo, state: a.state, running: a.running, enabled: a.enabled },
    { name: null, repo: null, state: null, running: true, enabled: true },
  );
  const d = rows.get("D");
  assert.deepEqual({ name: d.name, repo: d.repo }, { name: null, repo: null }, "a detached member with no name");
});
