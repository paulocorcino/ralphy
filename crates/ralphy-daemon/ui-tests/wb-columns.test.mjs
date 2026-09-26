// Unit tests for assets/ui/wb-columns.js — the columns beside a maximized
// console (ADR-0051 §5). Runs the real source against an empty window: the
// module is pure and touches nothing at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-columns.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBColumns;
}

test("cap: 80 cells per column, floor of 1", () => {
  const C = load();
  assert.equal(C.cap(1280, 8), 2, "exactly two 80-cell columns fit");
  assert.equal(C.cap(1279, 8), 1, "one pixel short of two");
  assert.equal(C.cap(390, 7), 1, "a phone");
  assert.equal(C.cap(0, 8), 1);
  assert.equal(C.cap(1280, 0), 1);
  assert.equal(C.cap(NaN, 8), 1);
});

test("cap: a larger font gives a smaller cap", () => {
  const C = load();
  assert.equal(C.cap(1800, 7), 3);
  assert.equal(C.cap(1800, 8), 2);
  assert.equal(C.cap(1800, 12), 1);
});

test("open: the new column goes right of the caller, the others keep order", () => {
  const C = load();
  const mid = ["a", "b", "c"];
  assert.deepEqual(C.open(mid, "b", "x", 4), { ok: true, columns: ["a", "b", "x", "c"] });
  assert.deepEqual(mid, ["a", "b", "c"], "input untouched");
  const end = ["a", "b"];
  assert.deepEqual(C.open(end, "b", "x", 3).columns, ["a", "b", "x"]);
  assert.deepEqual(end, ["a", "b"]);
  const none = [];
  assert.deepEqual(C.open(none, "a", "x", 2).columns, ["a", "x"], "from a lone maximize");
  assert.deepEqual(none, []);
});

test("open: refusals carry a reason", () => {
  const C = load();
  assert.deepEqual(C.open(["a", "b"], "a", "x", 2), {
    ok: false,
    reason: "No room for another column",
  });
  assert.deepEqual(C.open(["a", "b"], "a", "b", 3), {
    ok: false,
    reason: "Already in a column",
  });
  assert.deepEqual(C.open([], "a", "x", 1), { ok: false, reason: "No room for another column" });
  assert.equal(C.open(["a", "b"], "z", "x", 3).ok, false, "caller not in the list");
});

test("restore: middle, leftmost, last-but-one", () => {
  const C = load();
  const mid = C.restore(["a", "b", "c"], "b");
  assert.deepEqual(mid.columns, ["a", "c"]);
  assert.equal(mid.unmax, null);
  assert.equal(mid.ended, false);
  assert.equal(mid.maximized, "a");

  const left = C.restore(["a", "b", "c"], "a");
  assert.deepEqual(left.columns, ["b", "c"]);
  assert.equal(left.maximized, "b");
  assert.equal(left.unmax, "a");
  assert.equal(left.ended, false);

  const last = C.restore(["a", "b"], "b");
  assert.deepEqual(last.columns, ["a"]);
  assert.equal(last.ended, true);
  assert.equal(last.maximized, "a");
  assert.equal(last.unmax, null);

  const absent = C.restore(["a", "b"], "z");
  assert.deepEqual(absent.columns, ["a", "b"]);
  assert.equal(absent.unmax, null);
});

test("painted: only as many as the cap, the list is kept", () => {
  const C = load();
  assert.deepEqual(C.painted(["a", "b", "c"], 3), [
    { id: "a", index: 0, count: 3 },
    { id: "b", index: 1, count: 3 },
    { id: "c", index: 2, count: 3 },
  ]);
  const kept = ["a", "b", "c"];
  assert.deepEqual(C.painted(kept, 2), [
    { id: "a", index: 0, count: 2 },
    { id: "b", index: 1, count: 2 },
  ]);
  assert.deepEqual(kept, ["a", "b", "c"]);
  assert.deepEqual(C.painted(["a"], 1), [{ id: "a", index: 0, count: 1 }]);
  assert.deepEqual(C.painted(["a", "b"], 0), [{ id: "a", index: 0, count: 1 }]);
});

function roster() {
  const row = (id, extra = {}) => ({
    id,
    agent: "claude",
    repo: "C:/r",
    kind: "agent",
    running: true,
    state: null,
    ...extra,
  });
  return {
    rows: [
      row("L1"),
      row("F1a"),
      row("L2", { running: false }),
      row("F2a", { state: "working" }),
      row("W", { state: "working" }),
    ],
    // f2 is listed FIRST, though f1 comes first in `membership`.
    fences: [
      { id: "f2", name: "Two", locked: false },
      { id: "f1", name: "One", locked: true },
      { id: "f3", name: "Empty", locked: false },
      { id: "f4", name: "Away", locked: false },
    ],
    membership: { f1: ["F1a"], f2: ["F2a"], f3: [], f4: [] },
    detached: { f4: [{ id: "D1", agent: "codex", repo: null, kind: "agent" }] },
    columns: ["L1", "F2a"],
    maximized: "L1",
  };
}

test("listFold: loose first, then fences in the Fence menu order, empty fences left out", () => {
  const C = load();
  const groups = C.listFold(roster());
  assert.deepEqual(
    groups.map((g) => (g.fence ? g.fence.id : null)),
    [null, "f2", "f1", "f4"],
  );
  assert.deepEqual(groups[2].fence, { id: "f1", name: "One", locked: true });
  const ids = groups.flatMap((g) => g.rows.map((r) => r.id));
  assert.ok(!ids.includes("L1"), "the maximized console is left out");
  assert.deepEqual(groups[0].rows.map((r) => r.id), ["L2", "W"]);
});

test("listFold: rows carry agent, repo, state and the reason they are disabled", () => {
  const C = load();
  const rows = new Map(C.listFold(roster()).flatMap((g) => g.rows).map((r) => [r.id, r]));
  const f2a = rows.get("F2a");
  assert.equal(f2a.enabled, false);
  assert.equal(f2a.reason, "Already in a column");
  assert.equal(f2a.state, "working", "state is carried through");
  const l2 = rows.get("L2");
  assert.equal(l2.enabled, true, "a console that is not running can be opened");
  assert.equal(l2.running, false);
  const f1a = rows.get("F1a");
  assert.equal(f1a.enabled, true, "a locked fence does not stop a column");
  assert.equal(f1a.reason, null);
  assert.equal(f1a.agent, "claude");
  assert.equal(f1a.repo, "C:/r");
  const d1 = rows.get("D1");
  assert.deepEqual(
    { enabled: d1.enabled, reason: d1.reason, running: d1.running, state: d1.state },
    { enabled: false, reason: "In a detached fence", running: true, state: null },
  );
  assert.equal(d1.agent, "codex");
});

test("listFold: a detached fence the fence list does not name adds no group", () => {
  const C = load();
  const r = roster();
  r.detached = { gone: [{ id: "Z", agent: "a", repo: null, kind: "agent" }] };
  const ids = C.listFold(r).flatMap((g) => g.rows.map((x) => x.id));
  assert.ok(!ids.includes("Z"));
});

test("toStored: a list of two or more, as a copy; below two, nothing", () => {
  const C = load();
  assert.equal(C.toStored([]), null);
  assert.equal(C.toStored(["a"]), null);
  const list = ["a", "b"];
  const stored = C.toStored(list);
  assert.deepEqual(stored, ["a", "b"]);
  assert.notEqual(stored, list, "not the same array");
});

test("fromStored: dead ids drop, and the first id must be the desk's maximized console", () => {
  const C = load();
  const aMax = [
    { id: "a", max: true },
    { id: "b", max: false },
  ];
  assert.deepEqual(C.fromStored(["a", "gone", "b"], aMax), ["a", "b"]);
  assert.deepEqual(
    C.fromStored(["a", "b"], [
      { id: "a", max: false },
      { id: "b", max: true },
    ]),
    [],
    "the first id is not maximized in the desk",
  );
  assert.deepEqual(C.fromStored(["gone", "a", "b"], aMax), [], "a dead first id is not maximized");
  assert.deepEqual(C.fromStored(["a", "gone"], aMax), [], "one survivor is not a column list");
  assert.deepEqual(C.fromStored("x", aMax), []);
  assert.deepEqual(C.fromStored(null, aMax), []);
  assert.deepEqual(C.fromStored([1, "a"], aMax), [], "only strings are kept");
  assert.deepEqual(C.fromStored(["a", "a", "b"], aMax), ["a", "b"], "duplicates drop");
});

test("painted: a falling cap hides the extra columns, a rising cap brings them back", () => {
  const C = load();
  const list = ["a", "b", "c"];
  const low = C.painted(list, 2);
  assert.deepEqual(
    low.map((p) => p.id),
    ["a", "b"],
  );
  assert.ok(low.every((p) => p.count === 2));
  const high = C.painted(list, 3);
  assert.deepEqual(
    high.map((p) => p.id),
    ["a", "b", "c"],
  );
  assert.ok(high.every((p) => p.count === 3));
  assert.deepEqual(list, ["a", "b", "c"], "the kept list is unchanged");
});

test("focusAfter: the focus stays on a painted column, else the rightmost painted", () => {
  const C = load();
  assert.equal(C.focusAfter(["a", "b"], "c"), "b");
  assert.equal(C.focusAfter(["a", "b"], "a"), "a");
  assert.equal(C.focusAfter([], "a"), null);
});
