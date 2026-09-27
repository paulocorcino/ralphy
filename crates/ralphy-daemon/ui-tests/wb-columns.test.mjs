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
      { id: "f2", name: "Two" },
      { id: "f1", name: "One" },
      { id: "f3", name: "Empty" },
      { id: "f4", name: "Away" },
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
  assert.deepEqual(groups[2].fence, { id: "f1", name: "One" });
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
});

test("listFold: a detached fence the fence list does not name adds no group", () => {
  const C = load();
  const r = roster();
  r.detached = { gone: [{ id: "Z", agent: "a", repo: null, kind: "agent" }] };
  const ids = C.listFold(r).flatMap((g) => g.rows.map((x) => x.id));
  assert.ok(!ids.includes("Z"));
});

test("listFold: a group whose rows share one repo carries it once", () => {
  const C = load();
  const groups = C.listFold(roster());
  const loose = groups.find((g) => !g.fence);
  assert.deepEqual({ shared: loose.shared, repo: loose.repo }, { shared: true, repo: "C:/r" });
  const mixed = roster();
  mixed.rows[2] = { ...mixed.rows[2], repo: "C:/other" };
  const m = C.listFold(mixed).find((g) => !g.fence);
  assert.deepEqual({ shared: m.shared, repo: m.repo }, { shared: false, repo: null });
  // `null` is the home directory, and two home rows share it.
  const home = roster();
  home.rows = home.rows.map((r) => ({ ...r, repo: null }));
  const h = C.listFold(home).find((g) => !g.fence);
  assert.deepEqual({ shared: h.shared, repo: h.repo }, { shared: true, repo: null });
});

test("rowLabel: the repo only when the group head does not print it", () => {
  const C = load();
  const label = (ref) => `<${ref}>`;
  const row = { agent: "claude", repo: "o/r" };
  assert.equal(C.rowLabel(row, { shared: true }, label), "claude");
  assert.equal(C.rowLabel(row, { shared: false }, label), "claude · <o/r>");
  assert.equal(C.rowLabel({ agent: "codex", repo: null }, { shared: false }, label), "codex · home");
});

test("filterGroups: agent, repo text and fence name match; empty groups drop", () => {
  const C = load();
  const label = (ref) => (ref === "C:/r" ? "owner/ralphy · WSL: Ubuntu" : ref);
  const groups = C.listFold(roster());
  assert.equal(C.filterGroups(groups, "", label), groups, "no query: the same list");
  assert.equal(C.filterGroups(groups, "   ", label), groups);
  const ids = (gs) => gs.flatMap((g) => g.rows.map((r) => r.id));
  assert.deepEqual(ids(C.filterGroups(groups, "CODEX", label)), ["D1"], "agent, case-insensitive");
  assert.deepEqual(ids(C.filterGroups(groups, "wsl", label)).sort(), ["F1a", "F2a", "L2", "W"], "repo text");
  const byName = C.filterGroups(groups, "two", label);
  assert.deepEqual(byName.map((g) => g.fence?.id), ["f2"], "a fence name keeps its whole group");
  assert.deepEqual(C.filterGroups(groups, "nothing", label), []);
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

test("focusStep: walks the painted columns and wraps at both ends", () => {
  const C = load();
  assert.equal(C.focusStep(["a", "b", "c"], "c", 1), "a");
  assert.equal(C.focusStep(["a", "b", "c"], "a", -1), "c");
  assert.equal(C.focusStep(["a", "b", "c"], "a", 1), "b");
  assert.equal(C.focusStep(["a", "b"], "x", 1), "a", "focus outside: the first");
  assert.equal(C.focusStep(["a", "b"], "x", -1), "b", "focus outside: the last");
  assert.equal(C.focusStep([], "a", 1), null);
});

test("external: an end, a remote maximize and a remote move leave the columns alone", () => {
  const C = load();
  for (const event of [
    { type: "ended", id: "b" },
    { type: "maximized", id: "b" },
    { type: "moved", id: "b" },
  ]) {
    const r = C.external(["a", "b", "c"], event);
    assert.equal(r.changed, false, event.type);
    assert.deepEqual(r.columns, ["a", "b", "c"], event.type);
  }
});

test("external: a console closed elsewhere leaves the columns and is never unmaximized", () => {
  const C = load();
  const mid = C.external(["a", "b", "c"], { type: "closed", ids: ["b"] });
  assert.deepEqual(mid.columns, ["a", "c"]);
  assert.equal(mid.changed, true);
  assert.equal(mid.unmax, null);
  assert.equal(mid.ended, false);
  const head = C.external(["a", "b"], { type: "closed", ids: ["a"] });
  assert.deepEqual(head.columns, ["b"]);
  assert.equal(head.ended, true);
  assert.equal(head.maximized, "b");
  assert.equal(head.unmax, null, "a closed console must not pass through setMax(false)");
});

test("external: a detach removes the fence's consoles; the old leftmost is unmaximized", () => {
  const C = load();
  const r = C.external(["a", "b", "c"], { type: "detached", ids: ["a", "z"] });
  assert.deepEqual(r.columns, ["b", "c"]);
  assert.equal(r.changed, true);
  assert.equal(r.unmax, "a");
  assert.equal(r.maximized, "b");
  assert.equal(r.ended, false);
  const none = C.external(["a", "b", "c"], { type: "detached", ids: ["z"] });
  assert.equal(none.changed, false);
  assert.deepEqual(none.columns, ["a", "b", "c"]);
});
