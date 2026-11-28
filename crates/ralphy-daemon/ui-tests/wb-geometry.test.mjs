// Unit tests for assets/ui/wb-geometry.ts: fence membership and tiling. The module
// needs no DOM, no socket and no siblings.
import { test } from "node:test";
import assert from "node:assert/strict";
import { fixture } from "./harness.mjs";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";
import { AB, EXISTING, FENCE_VIEW, FIT, ORIGIN, members } from "./wb-geometry-fixture.mjs";

const memberCount = (m) => Object.values(m).reduce((n, ids) => n + ids.length, 0);

const TB = [
  { id: "t", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "u", rect: { left: 0, top: 100, width: 100, height: 100 } },
];

// `tileIntoRect(rect, members)` is the old global Arrange generalised: target
// rect plus member list in, one rect per member out, in order. The grid is
// today's — `cols = ceil(sqrt(n))` — and aspect-independent on purpose: making
// it follow the rect's aspect is a second change hiding inside a move.
//
// The base rect has a NON-ZERO origin as a built-in negative control: an
// implementation that tiles from 0,0 and forgets `rect.left`/`rect.top` reds
// every row below.
const R = { left: 100, top: 200, width: 1000, height: 600 };

const TILES = [
  { name: "no members tile to nothing", rect: R, n: 0, want: [] },
  {
    name: "one member takes the whole rect, inset by the pad",
    rect: R,
    n: 1,
    want: [{ left: 112, top: 212, width: 976, height: 576 }],
  },
  {
    name: "two members split the rect into one row of two",
    rect: R,
    n: 2,
    want: [
      { left: 112, top: 212, width: 483, height: 576 },
      { left: 605, top: 212, width: 483, height: 576 },
    ],
  },
  {
    name: "three members take a 2x2 grid with the last row half empty",
    rect: R,
    n: 3,
    want: [
      { left: 112, top: 212, width: 483, height: 283 },
      { left: 605, top: 212, width: 483, height: 283 },
      { left: 112, top: 505, width: 483, height: 283 },
    ],
  },
  {
    name: "four members fill the same 2x2 grid",
    rect: R,
    n: 4,
    want: [
      { left: 112, top: 212, width: 483, height: 283 },
      { left: 605, top: 212, width: 483, height: 283 },
      { left: 112, top: 505, width: 483, height: 283 },
      { left: 605, top: 505, width: 483, height: 283 },
    ],
  },
  {
    name: "nine members take a 3x3 grid whose far edge lands exactly on the pad",
    rect: { left: 0, top: 0, width: 944, height: 584 },
    n: 9,
    want: [0, 1, 2, 3, 4, 5, 6, 7, 8].map((i) => ({
      left: [12, 322, 632][i % 3],
      top: [12, 202, 392][Math.floor(i / 3)],
      width: 300,
      height: 180,
    })),
  },
  {
    // Aspect pair, first half: a rect far taller than wide. Reds if the two
    // axes are swapped — the twin below would then answer this row's numbers.
    name: "a rect narrower than tall splits on X and keeps the full height",
    rect: { left: 0, top: 0, width: 200, height: 800 },
    n: 2,
    want: [
      { left: 12, top: 12, width: 83, height: 776 },
      { left: 105, top: 12, width: 83, height: 776 },
    ],
  },
  {
    name: "a rect wider than tall splits on X the same way, with the full width to share",
    rect: { left: 0, top: 0, width: 800, height: 200 },
    n: 2,
    want: [
      { left: 12, top: 12, width: 383, height: 176 },
      { left: 405, top: 12, width: 383, height: 176 },
    ],
  },
  {
    // NEGATIVE CONTROL for the pad/gap fallback: with the roomy geometry this
    // rect yields a NEGATIVE cell height (and a 5px width), so an
    // implementation without the fallback returns rects outside the rect.
    name: "a rect too small on BOTH axes drops pad and gap on both",
    rect: { left: 0, top: 0, width: 60, height: 30 },
    n: 9,
    want: [0, 1, 2, 3, 4, 5, 6, 7, 8].map((i) => ({
      left: [0, 20, 40][i % 3],
      top: [0, 10, 20][Math.floor(i / 3)],
      width: 20,
      height: 10,
    })),
  },
  {
    // NEGATIVE CONTROL for the fallback being PER AXIS: collapsing both axes
    // together deforms X (which still fits comfortably) down to 250-wide
    // columns starting at 0.
    name: "a rect too small on ONE axis keeps the pad on the axis that still fits",
    rect: { left: 0, top: 0, width: 1000, height: 30 },
    n: 4,
    want: [
      { left: 12, top: 0, width: 483, height: 15 },
      { left: 505, top: 0, width: 483, height: 15 },
      { left: 12, top: 15, width: 483, height: 15 },
      { left: 505, top: 15, width: 483, height: 15 },
    ],
  },
];

test("stageExtent falls back to the module's own margin when none is passed", () => {
  const rects = [{ left: 0, top: 0, width: 300, height: 300 }];
  const tiny = { width: 120, height: 90 };
  const withMargin = WBGeometry.stageExtent(rects, tiny, 200);
  const defaulted = WBGeometry.stageExtent(rects, tiny);
  const other = WBGeometry.stageExtent(rects, tiny, 900);
  assert.deepEqual(defaulted, withMargin);
  assert.notDeepEqual(defaulted, other);
});

const overlaps = (a, b) =>
  a.left < b.left + b.width &&
  a.left + a.width > b.left &&
  a.top < b.top + b.height &&
  a.top + a.height > b.top;

test("fenceSpawnRect: the first six spawn rects are pairwise disjoint", () => {
  const wb = WBGeometry;
  const rects = [0, 1, 2, 3, 4, 5].map((i) => wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, i));
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) {
      assert.ok(
        !overlaps(rects[i], rects[j]),
        `fences ${i} and ${j} overlap: ${JSON.stringify(rects[i])} vs ${JSON.stringify(rects[j])}`,
      );
    }
  }
});

// Each row: the fences, the windows, the map `fenceMembership` gives, and how
// many windows it places (every window belongs to at most one fence).
const MEMBERSHIP = [
  {
    name: "a window whose title bar is inside a fence is its member",
    fences: AB,
    windows: [{ id: "w1", rect: { left: 10, top: 10, width: 40, height: 40 } }],
    want: { a: ["w1"], b: [] },
  },
  {
    name: "a window whose title bar is outside every fence belongs nowhere",
    fences: AB,
    windows: [{ id: "w1", rect: { left: 400, top: 400, width: 40, height: 40 } }],
    want: { a: [], b: [] },
    count: 0,
  },
  {
    // Spans 60..120 across the shared edge at 100; title bar x = 90, inside A.
    name: "a window straddling the border belongs to the fence holding its title bar",
    fences: AB,
    windows: [{ id: "w1", rect: { left: 60, top: 20, width: 60, height: 40 } }],
    want: { a: ["w1"], b: [] },
    count: 1,
  },
  {
    // NEGATIVE CONTROL: a CLOSED containment test (`cx <= left + width`) hands
    // this point to A, the fence it is leaving, and a `break`-less fold lists
    // it under both. Half-open on the far edge is what makes "exactly one"
    // hold for abutting fences.
    name: "a title bar point exactly on the shared edge belongs to the RIGHT fence",
    fences: AB,
    windows: [{ id: "w1", rect: { left: 80, top: 20, width: 40, height: 40 } }],
    want: { a: [], b: ["w1"] },
    count: 1,
  },
  {
    // Title bar y = 84 + 16 = 100, the shared edge.
    name: "a title bar point exactly on the shared HORIZONTAL edge belongs to the LOWER fence",
    fences: TB,
    windows: [{ id: "w1", rect: { left: 20, top: 84, width: 40, height: 40 } }],
    want: { t: [], u: ["w1"] },
    count: 1,
  },
  {
    name: "a window straddling the horizontal border belongs to the fence holding its title bar",
    fences: TB,
    windows: [{ id: "w1", rect: { left: 20, top: 60, width: 40, height: 60 } }],
    want: { t: ["w1"], u: [] },
    count: 1,
  },
  {
    name: "no fences at all is an empty map, whatever the windows",
    fences: [],
    windows: [{ id: "w1", rect: { left: 0, top: 0, width: 10, height: 10 } }],
    want: {},
  },
  { name: "a fence with no members maps to an empty list", fences: AB, windows: [], want: { a: [], b: [] } },
];

test("fenceMembership places each window in the one fence holding its title bar", () => {
  for (const row of MEMBERSHIP) {
    const m = WBGeometry.fenceMembership(row.fences, row.windows);
    assert.deepEqual(m, row.want, row.name);
    if (row.count !== undefined) assert.equal(memberCount(m), row.count, `${row.name}: members`);
  }
});

// --- fenceOf: the one fence a rect belongs to ---------------------------------
test("fenceOf answers the fence holding the rect's title bar point, and null outside every fence", () => {
  const { fenceOf } = WBGeometry;
  assert.equal(fenceOf(AB, { left: 10, top: 10, width: 20, height: 20 })?.id, "a");
  assert.equal(fenceOf(AB, { left: 150, top: 10, width: 20, height: 20 })?.id, "b");
  assert.equal(fenceOf(AB, { left: 300, top: 300, width: 20, height: 20 }), null);
  assert.equal(fenceOf([], { left: 10, top: 10, width: 20, height: 20 }), null);
  assert.equal(fenceOf(undefined, { left: 10, top: 10, width: 20, height: 20 }), null);
});

// The table is the daemon's too (`desk::membership` tests): one rule in two
// languages, held by one file.
test("every shared membership case finds the fence the daemon finds", () => {
  const { fenceOf } = WBGeometry;
  const cases = fixture("api-desk--fence-membership-cases");
  assert.ok(cases.length >= 8, `the table lost cases: ${cases.length}`);
  for (const c of cases) {
    assert.equal(fenceOf(c.fences, c.rect)?.id ?? null, c.fence, c.name);
  }
});

test("fenceOf is half-open on the far edge, like fenceMembership: the shared edge is the RIGHT fence's", () => {
  const { fenceOf } = WBGeometry;
  assert.equal(fenceOf(AB, { left: 80, top: 20, width: 40, height: 40 })?.id, "b");
  assert.equal(fenceOf(TB, { left: 20, top: 84, width: 40, height: 40 })?.id, "u");
});

test("fenceOf never disagrees with fenceMembership", () => {
  const { fenceOf, fenceMembership } = WBGeometry;
  const rects = [
    { left: 10, top: 10, width: 20, height: 20 },
    { left: 80, top: 20, width: 40, height: 40 },
    { left: 150, top: 10, width: 20, height: 20 },
    { left: 300, top: 300, width: 20, height: 20 },
    { left: 90, top: 90, width: 20, height: 20 },
  ];
  for (const [i, rect] of rects.entries()) {
    const m = fenceMembership(AB, [{ id: "w", rect }]);
    const owner = Object.keys(m).find((k) => m[k].includes("w")) ?? null;
    assert.equal(fenceOf(AB, rect)?.id ?? null, owner, `rect #${i}`);
  }
});

// #743: the measured desk. The title bar is above the locked fence, the centre
// is 15 px inside it.
test("fenceOf: a window whose title bar is outside a fence is free, whatever its body covers", () => {
  const { fenceOf, fenceMembership, fenceHolds } = WBGeometry;
  const fence = { id: "cactus", rect: { left: 2479, top: 1200, width: 1130, height: 767 }, locked: true };
  const rect = { left: 3007, top: 1045, width: 560, height: 340 };
  assert.equal(fenceOf([fence], rect), null);
  assert.deepEqual(fenceMembership([fence], [{ id: "w", rect }]), { cactus: [] });
  assert.equal(fenceHolds([fence], rect, false), false);
});

test("fenceOf: a window whose title bar is inside a fence is its member, even with the body outside", () => {
  const { fenceOf } = WBGeometry;
  const fence = { id: "f", rect: { left: 0, top: 0, width: 400, height: 300 } };
  // Centre y = 450, below the fence; the title bar is at y 280..313.
  assert.equal(fenceOf([fence], { left: 50, top: 280, width: 200, height: 340 })?.id, "f");
});

test("fenceFits: a fence compared against ITSELF by id fits — a move must not refuse its own start", () => {
  assert.equal(WBGeometry.fenceFits(EXISTING, { id: "e", rect: FIT }), true);
});

for (const row of TILES) {
  test(`tileIntoRect: ${row.name}`, () => {
    assert.deepEqual(WBGeometry.tileIntoRect(row.rect, members(row.n)), row.want);
  });
}
