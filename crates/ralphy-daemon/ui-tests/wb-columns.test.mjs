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
// The label builder the title uses; the column row must call the same one.
const NAME_SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-console-name.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBColumns;
}

// A grid of one row per column, from a flat list of ids.
const cols = (...ids) => ids.map((id) => [id]);

test("cap: no limit wider than a phone, one console at a phone width", () => {
  const C = load();
  assert.equal(C.cap(561, 560), Infinity, "one pixel wider than a phone");
  assert.equal(C.cap(1366, 560), Infinity, "a notebook");
  assert.equal(C.cap(560, 560), 1, "exactly a phone");
  assert.equal(C.cap(390, 560), 1, "a phone");
  assert.equal(C.cap(0, 560), 1, "a viewport not measured yet");
  assert.equal(C.cap(NaN, 560), 1);
  // With no limit, every console in the grid is painted.
  const many = cols("a", "b", "c", "d", "e", "f", "g", "h", "i");
  assert.equal(C.painted(many, C.cap(1366, 560)).length, 9);
  assert.equal(C.open(many, "i", "x", C.cap(1366, 560), "right").ok, true);
});

test("open right: a new column right of the caller's column, the others keep order", () => {
  const C = load();
  const mid = cols("a", "b", "c");
  assert.deepEqual(C.open(mid, "b", "x", 4, "right"), { ok: true, columns: cols("a", "b", "x", "c") });
  assert.deepEqual(mid, cols("a", "b", "c"), "input untouched");
  assert.deepEqual(C.open(cols("a", "b"), "b", "x", 3, "right").columns, cols("a", "b", "x"));
  const none = [];
  assert.deepEqual(C.open(none, "a", "x", 2, "right").columns, cols("a", "x"), "from a lone maximize");
  assert.deepEqual(none, []);
  // From a lower row, the new column still goes right of the whole column.
  assert.deepEqual(C.open([["a", "b"], ["c"]], "b", "x", 9, "right").columns, [["a", "b"], ["x"], ["c"]]);
});

test("open down: a new row directly below the caller, in its column", () => {
  const C = load();
  const g = [["a", "b"], ["c"]];
  assert.deepEqual(C.open(g, "a", "x", 9, "down").columns, [["a", "x", "b"], ["c"]]);
  assert.deepEqual(g, [["a", "b"], ["c"]], "input untouched");
  assert.deepEqual(C.open(g, "c", "x", 9, "down").columns, [["a", "b"], ["c", "x"]]);
  assert.deepEqual(C.open([], "a", "x", 2, "down").columns, [["a", "x"]], "from a lone maximize");
});

test("open: refusals carry a reason", () => {
  const C = load();
  assert.deepEqual(C.open(cols("a", "b"), "a", "x", 2, "right"), {
    ok: false,
    reason: "No room for another console",
  });
  assert.deepEqual(C.open([["a", "b"]], "a", "b", 3, "right"), {
    ok: false,
    reason: "Already in a column",
  });
  assert.deepEqual(C.open([], "a", "x", 1, "down"), { ok: false, reason: "No room for another console" });
  assert.equal(C.open(cols("a", "b"), "z", "x", 3, "right").ok, false, "caller not in the grid");
});

test("restore: middle, first, last-but-one", () => {
  const C = load();
  const mid = C.restore(cols("a", "b", "c"), "b");
  assert.deepEqual(mid.columns, cols("a", "c"));
  assert.equal(mid.unmax, null);
  assert.equal(mid.ended, false);
  assert.equal(mid.maximized, "a");

  const left = C.restore(cols("a", "b", "c"), "a");
  assert.deepEqual(left.columns, cols("b", "c"));
  assert.equal(left.maximized, "b");
  assert.equal(left.unmax, "a");
  assert.equal(left.ended, false);

  const last = C.restore(cols("a", "b"), "b");
  assert.deepEqual(last.columns, cols("a"));
  assert.equal(last.ended, true);
  assert.equal(last.maximized, "a");
  assert.equal(last.unmax, null);

  const absent = C.restore(cols("a", "b"), "z");
  assert.deepEqual(absent.columns, cols("a", "b"));
  assert.equal(absent.unmax, null);
});

test("restore: a row leaves its column; an emptied column goes; the next in reading order is maximized", () => {
  const C = load();
  const low = C.restore([["a", "b"], ["c"]], "b");
  assert.deepEqual(low.columns, cols("a", "c"));
  assert.equal(low.unmax, null);
  const lone = C.restore([["a", "b"], ["c"]], "c");
  assert.deepEqual(lone.columns, [["a", "b"]], "an empty column is removed");
  assert.equal(lone.ended, false);
  const head = C.restore([["a", "b"], ["c"]], "a");
  assert.deepEqual(head.columns, cols("b", "c"));
  assert.equal(head.maximized, "b", "the row below takes the maximize, not the next column");
  assert.equal(head.unmax, "a");
});

test("painted: only as many as the cap, the grid is kept", () => {
  const C = load();
  const one = (id, index, count) => ({ id, index, count, row: 0, rows: 1 });
  assert.deepEqual(C.painted(cols("a", "b", "c"), 3), [one("a", 0, 3), one("b", 1, 3), one("c", 2, 3)]);
  const kept = cols("a", "b", "c");
  assert.deepEqual(C.painted(kept, 2), [one("a", 0, 2), one("b", 1, 2)]);
  assert.deepEqual(kept, cols("a", "b", "c"));
  assert.deepEqual(C.painted(cols("a"), 1), [one("a", 0, 1)]);
  assert.deepEqual(C.painted(cols("a", "b"), 0), [one("a", 0, 1)]);
});

test("painted: rows carry their position in the column", () => {
  const C = load();
  assert.deepEqual(C.painted([["a", "b"], ["c"]], Infinity), [
    { id: "a", index: 0, count: 2, row: 0, rows: 2 },
    { id: "b", index: 0, count: 2, row: 1, rows: 2 },
    { id: "c", index: 1, count: 2, row: 0, rows: 1 },
  ]);
  // At a phone width only the first console, as a single one.
  assert.deepEqual(C.painted([["a", "b"], ["c"]], 1), [{ id: "a", index: 0, count: 1, row: 0, rows: 1 }]);
});

test("keep: dead ids drop, and a column left empty goes", () => {
  const C = load();
  assert.deepEqual(C.keep([["a", "b"], ["c"]], new Set(["a", "b"])), [["a", "b"]]);
  assert.deepEqual(C.keep([["a", "b"], ["c"]], new Set(["b", "c"])), cols("b", "c"));
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
      row("L2", { running: false, name: "backend" }),
      row("F2a", { state: "working" }),
      row("W", { state: "working", name: "W name" }),
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
  const C = load();
  const groups = C.listFold(roster());
  assert.deepEqual(
    groups.map((g) => (g.fence ? g.fence.id : null)),
    [null, "f2", "f1", "f4"],
  );
  assert.deepEqual(groups[2].fence, { id: "f1", name: "One" });
  const ids = groups.flatMap((g) => g.rows.map((r) => r.id));
  assert.ok(!ids.includes("L1"), "the column that opened the list is left out");
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
  assert.equal(d1.name, "home #1", "a detached row carries its console name");
  assert.equal(rows.get("W").name, "W name", "a loose row carries its console name");
});

test("listFold: a detached fence the fence list does not name adds no group", () => {
  const C = load();
  const r = roster();
  r.detached = { gone: [{ id: "Z", agent: "a", repo: null, kind: "agent" }] };
  const ids = C.listFold(r).flatMap((g) => g.rows.map((x) => x.id));
  assert.ok(!ids.includes("Z"));
});

// ADR-0066 §4: every row names its console, so no group head prints a repo,
// even when every row of the group shares one.
test("listFold: a group is its fence and its rows, with no shared repo", () => {
  const C = load();
  for (const g of C.listFold(roster())) {
    assert.deepEqual(Object.keys(g).sort(), ["fence", "rows"]);
    assert.ok(!("shared" in g) && !("repo" in g));
  }
});

test("rowLabel: the console label builder, as in the title", () => {
  const C = load();
  const window = {};
  new Function("window", NAME_SRC)(window);
  const N = window.WBConsoleName;
  assert.equal(C.rowLabel({ name: "fincal #1", agent: "claude", repo: "o/r" }, N.consoleLabel), "fincal #1 (claude)");
  assert.equal(C.rowLabel({ name: "home #2", agent: "console", repo: null }, N.consoleLabel), "home #2 (console)");
});

test("filterGroups: console name, agent, repo text and fence name match; empty groups drop", () => {
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
  assert.deepEqual(ids(C.filterGroups(groups, "back", label)), ["L2"], "console name");
  assert.deepEqual(C.filterGroups(groups, "nothing", label), []);
});

test("swap: replace the row's console; the old one leaves the grid", () => {
  const C = load();
  assert.deepEqual(C.swap(cols("a", "b", "c"), "b", "x"), {
    ok: true,
    columns: cols("a", "x", "c"),
    ended: false,
    unmax: null,
  });
  // The first console is swapped out: it stops being the maximized console.
  assert.deepEqual(C.swap(cols("a", "b"), "a", "x"), {
    ok: true,
    columns: cols("x", "b"),
    ended: false,
    unmax: "a",
  });
  assert.deepEqual(C.swap([["a", "b"]], "b", "x").columns, [["a", "x"]], "a lower row");
});

test("swap: a console already in a row changes places", () => {
  const C = load();
  assert.deepEqual(C.swap(cols("a", "b", "c"), "c", "b").columns, cols("a", "c", "b"));
  assert.equal(C.swap(cols("a", "b", "c"), "c", "b").unmax, null);
  // With the first console, either way: it stays in the grid, so the paint
  // moves the maximize, not `unmax`.
  const r = C.swap(cols("a", "b", "c"), "a", "c");
  assert.deepEqual(r.columns, cols("c", "b", "a"));
  assert.equal(r.unmax, null);
  const l = C.swap(cols("a", "b"), "b", "a");
  assert.deepEqual(l.columns, cols("b", "a"));
  assert.equal(l.unmax, null);
  // Across columns, between rows.
  assert.deepEqual(C.swap([["a", "b"], ["c"]], "b", "c").columns, [["a", "c"], ["b"]]);
});

test("swap: a lone maximized console is swapped for another", () => {
  const C = load();
  assert.deepEqual(C.swap([], "a", "x"), { ok: true, columns: cols("x"), ended: true, unmax: "a" });
});

test("swap: itself, or a caller not in the grid, changes nothing", () => {
  const C = load();
  assert.equal(C.swap(cols("a", "b"), "b", "b").ok, false);
  assert.equal(C.swap(cols("a", "b"), "z", "x").ok, false);
});

test("listFold: at the cap a row cannot open a column but can be swapped in", () => {
  const C = load();
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

test("listFold: the list leaves out the column that opened it, not the leftmost", () => {
  const C = load();
  const r = roster();
  r.from = "F2a";
  const ids = C.listFold(r).flatMap((g) => g.rows.map((x) => x.id));
  assert.ok(!ids.includes("F2a"));
  assert.ok(ids.includes("L1"), "the leftmost can be swapped with");
});

test("toStored: a grid of two or more, as a copy; below two, nothing", () => {
  const C = load();
  assert.equal(C.toStored([]), null);
  assert.equal(C.toStored(cols("a")), null);
  const grid = [["a", "b"]];
  const stored = C.toStored(grid);
  assert.deepEqual(stored, [["a", "b"]]);
  assert.notEqual(stored[0], grid[0], "not the same arrays");
});

test("fromStored: dead ids drop, and the first id must be the desk's maximized console", () => {
  const C = load();
  const aMax = [
    { id: "a", max: true },
    { id: "b", max: false },
    { id: "c", max: false },
  ];
  assert.deepEqual(C.fromStored([["a", "gone"], ["b"]], aMax), cols("a", "b"));
  assert.deepEqual(C.fromStored([["gone"], ["a"], ["b"]], aMax), [], "a dead first id is not maximized");
  assert.deepEqual(
    C.fromStored([["a"], ["b"]], [
      { id: "a", max: false },
      { id: "b", max: true },
    ]),
    [],
    "the first id is not maximized in the desk",
  );
  assert.deepEqual(C.fromStored([["a", "gone"]], aMax), [], "one survivor is not a grid");
  assert.deepEqual(C.fromStored("x", aMax), []);
  assert.deepEqual(C.fromStored(null, aMax), []);
  assert.deepEqual(C.fromStored([[1, "a"], ["b"]], aMax), cols("a", "b"), "only strings are kept");
  assert.deepEqual(C.fromStored([["a", "a"], ["b", "a"]], aMax), cols("a", "b"), "duplicates drop");
  assert.deepEqual(C.fromStored([["a", "c"], [], ["b"]], aMax), [["a", "c"], ["b"]], "empty columns drop");
});

test("fromStored: a flat list stored before rows reads as one row per column", () => {
  const C = load();
  const aMax = [
    { id: "a", max: true },
    { id: "b", max: false },
  ];
  assert.deepEqual(C.fromStored(["a", "gone", "b"], aMax), cols("a", "b"));
  assert.deepEqual(C.fromStored(["gone", "a", "b"], aMax), []);
});

test("dirOf: right or down; anything else is right", () => {
  const C = load();
  assert.equal(C.dirOf("down"), "down");
  assert.equal(C.dirOf("right"), "right");
  assert.equal(C.dirOf(null), "right");
  assert.equal(C.dirOf("up"), "right");
});

test("painted: a falling cap hides the extra consoles, a rising cap brings them back", () => {
  const C = load();
  const grid = cols("a", "b", "c");
  const low = C.painted(grid, 2);
  assert.deepEqual(
    low.map((p) => p.id),
    ["a", "b"],
  );
  assert.ok(low.every((p) => p.count === 2));
  const high = C.painted(grid, 3);
  assert.deepEqual(
    high.map((p) => p.id),
    ["a", "b", "c"],
  );
  assert.ok(high.every((p) => p.count === 3));
  assert.deepEqual(grid, cols("a", "b", "c"), "the kept grid is unchanged");
});

test("focusAfter: the focus stays on a painted console, else the last painted", () => {
  const C = load();
  assert.equal(C.focusAfter(["a", "b"], "c"), "b");
  assert.equal(C.focusAfter(["a", "b"], "a"), "a");
  assert.equal(C.focusAfter([], "a"), null);
});

test("focusMove x: walks the painted columns and wraps at both ends", () => {
  const C = load();
  const p = C.painted(cols("a", "b", "c"), Infinity);
  assert.equal(C.focusMove(p, "c", "x", 1), "a");
  assert.equal(C.focusMove(p, "a", "x", -1), "c");
  assert.equal(C.focusMove(p, "a", "x", 1), "b");
  const two = C.painted(cols("a", "b"), Infinity);
  assert.equal(C.focusMove(two, "x", "x", 1), "a", "focus outside: the first");
  assert.equal(C.focusMove(two, "x", "x", -1), "b", "focus outside: the last");
  assert.equal(C.focusMove([], "a", "x", 1), null);
});

test("focusMove x: the same row in the next column, or its last row", () => {
  const C = load();
  const p = C.painted([["a", "b", "c"], ["d", "e"], ["f"]], Infinity);
  assert.equal(C.focusMove(p, "b", "x", 1), "e", "same position");
  assert.equal(C.focusMove(p, "c", "x", 1), "e", "a shorter column: its last row");
  assert.equal(C.focusMove(p, "e", "x", 1), "f");
  assert.equal(C.focusMove(p, "f", "x", 1), "a", "wraps to the first column, row 0");
});

test("focusMove y: walks the rows of one column and wraps", () => {
  const C = load();
  const p = C.painted([["a", "b", "c"], ["d"]], Infinity);
  assert.equal(C.focusMove(p, "a", "y", 1), "b");
  assert.equal(C.focusMove(p, "c", "y", 1), "a");
  assert.equal(C.focusMove(p, "a", "y", -1), "c");
  assert.equal(C.focusMove(p, "d", "y", 1), "d", "a column of one row stays");
});

test("external: an end, a remote maximize and a remote move leave the grid alone", () => {
  const C = load();
  for (const event of [
    { type: "ended", id: "b" },
    { type: "maximized", id: "b" },
    { type: "moved", id: "b" },
  ]) {
    const r = C.external(cols("a", "b", "c"), event);
    assert.equal(r.changed, false, event.type);
    assert.deepEqual(r.columns, cols("a", "b", "c"), event.type);
  }
});

test("external: a console closed elsewhere leaves the grid and is never unmaximized", () => {
  const C = load();
  const mid = C.external(cols("a", "b", "c"), { type: "closed", ids: ["b"] });
  assert.deepEqual(mid.columns, cols("a", "c"));
  assert.equal(mid.changed, true);
  assert.equal(mid.unmax, null);
  assert.equal(mid.ended, false);
  const head = C.external(cols("a", "b"), { type: "closed", ids: ["a"] });
  assert.deepEqual(head.columns, cols("b"));
  assert.equal(head.ended, true);
  assert.equal(head.maximized, "b");
  assert.equal(head.unmax, null, "a closed console must not pass through setMax(false)");
  const row = C.external([["a"], ["b", "c"]], { type: "closed", ids: ["c"] });
  assert.deepEqual(row.columns, cols("a", "b"), "a row leaves its column");
});

test("external: a detach removes the fence's consoles; the old first console is unmaximized", () => {
  const C = load();
  const r = C.external(cols("a", "b", "c"), { type: "detached", ids: ["a", "z"] });
  assert.deepEqual(r.columns, cols("b", "c"));
  assert.equal(r.changed, true);
  assert.equal(r.unmax, "a");
  assert.equal(r.maximized, "b");
  assert.equal(r.ended, false);
  const none = C.external(cols("a", "b", "c"), { type: "detached", ids: ["z"] });
  assert.equal(none.changed, false);
  assert.deepEqual(none.columns, cols("a", "b", "c"));
});

// A window at `left, top`, 400 wide and 300 high unless said otherwise.
const win = (id, left, top, width = 400) => ({ id, rect: { left, top, width, height: 300 } });

test("fromRects: stacked windows are rows, windows side by side are columns", () => {
  const C = load();
  // A above B; C to the right; D overlaps C by a quarter of its width.
  const items = [win("a", 0, 0), win("b", 20, 320), win("c", 420, 0), win("d", 720, 320)];
  assert.deepEqual(C.fromRects(items), [["a", "b"], ["c"], ["d"]]);
  assert.deepEqual(items[1], win("b", 20, 320), "input untouched");
  // Any input order gives the same grid.
  assert.deepEqual(C.fromRects([...items].reverse()), [["a", "b"], ["c"], ["d"]]);
  // The first console is the top row of the leftmost column, even when a
  // lower window starts further left.
  assert.deepEqual(C.fromRects([win("low", 0, 400), win("high", 50, 0)]), [["high", "low"]]);
});

test("fromRects: a staircase is compared with each column's first window only", () => {
  const C = load();
  // Each step moves 150 px right: 250 of 400 overlap the step before, but the
  // third step overlaps the first by only 100.
  const stairs = [win("a", 0, 0), win("b", 150, 100), win("c", 300, 200), win("d", 450, 300)];
  assert.deepEqual(C.fromRects(stairs), [["a", "b"], ["c", "d"]]);
  // Exactly half of the narrower width is not enough.
  assert.deepEqual(C.fromRects([win("a", 0, 0), win("b", 200, 320)]), [["a"], ["b"]]);
  // A narrow window under a wide one joins it.
  assert.deepEqual(C.fromRects([win("a", 0, 0, 800), win("b", 500, 320, 200)]), [["a", "b"]]);
});

test("fromRects: no window, one window", () => {
  const C = load();
  assert.deepEqual(C.fromRects([]), []);
  assert.deepEqual(C.fromRects(undefined), []);
  assert.deepEqual(C.fromRects([win("a", 10, 10)]), [["a"]]);
});
