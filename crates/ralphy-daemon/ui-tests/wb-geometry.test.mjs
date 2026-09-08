// Unit tests for assets/ui/wb-geometry.js — the workbench's plane geometry.
//
// These tests came over from wb-console.test.mjs with the code they exercise
// (ADR-0022 §3, restated for assets as ADR-0057 D4). Not one assertion was
// rewritten in the move: they say what they said before the extraction, which
// is the whole point of moving them rather than writing new ones. The only
// change is the loader — this module needs no DOM, no socket and no siblings,
// which is what "pure" bought.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-geometry.js"), "utf8");

// ONE global, and it is only the assignment target. Compare with the loader in
// wb-console.test.mjs, which needs a window, a document, a location, two real
// sibling modules and a hidden BroadcastChannel — that gap is the argument for
// the extraction, stated as code.
function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBGeometry;
}

const MARGIN = 200;
const FENCE_VIEW = { width: 1400, height: 900 };
const ORIGIN = { left: 0, top: 0 };

// Two abutting fences. Membership is DERIVED from the centre point, never
// stored: no window record gains a `fenceId`, so a fence and a window can never
// disagree about it. Containment is HALF-OPEN (`left <= cx < left + width`)
// because fences may abut — a closed test would put a centre sitting on the
// shared border inside BOTH of them, breaking "exactly one fence".
const AB = [
  { id: "a", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "b", rect: { left: 100, top: 0, width: 100, height: 100 } },
];

const memberCount = (m) => Object.values(m).reduce((n, ids) => n + ids.length, 0);

const TB = [
  { id: "t", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "u", rect: { left: 0, top: 100, width: 100, height: 100 } },
];

const FIT = { left: 100, top: 100, width: 100, height: 100 };

const EXISTING = [{ id: "e", rect: FIT }];

const R = { left: 100, top: 200, width: 1000, height: 600 };

const members = (n) => Array.from({ length: n }, (_, i) => ({ id: `m${i}` }));

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
  const withMargin = load().stageExtent(rects, tiny, 200);
  const defaulted = load().stageExtent(rects, tiny);
  const other = load().stageExtent(rects, tiny, 900);
  assert.deepEqual(defaulted, withMargin);
  assert.notDeepEqual(defaulted, other);
});

test("stageExtent mutates neither argument", () => {
  const rects = [
    { left: 900, top: 40, width: 600, height: 380 },
    { left: 40, top: 600, width: 600, height: 380 },
  ];
  const viewport = { width: 1000, height: 700 };
  const rectsBefore = structuredClone(rects);
  const viewportBefore = structuredClone(viewport);
  load().stageExtent(rects, viewport, MARGIN);
  assert.deepEqual(rects, rectsBefore);
  assert.deepEqual(viewport, viewportBefore);
});

test("fenceSpawnRect: the first six spawn rects are pairwise disjoint", () => {
  const wb = load();
  const rects = [0, 1, 2, 3, 4, 5].map((i) => wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, i));
  const overlaps = (a, b) =>
    a.left < b.left + b.width &&
    a.left + a.width > b.left &&
    a.top < b.top + b.height &&
    a.top + a.height > b.top;
  for (let i = 0; i < rects.length; i++) {
    for (let j = i + 1; j < rects.length; j++) {
      assert.ok(
        !overlaps(rects[i], rects[j]),
        `fences ${i} and ${j} overlap: ${JSON.stringify(rects[i])} vs ${JSON.stringify(rects[j])}`,
      );
    }
  }
});

test("fenceMembership: a window whose centre is inside a fence is its member", () => {
  const m = load().fenceMembership(AB, [
    { id: "w1", rect: { left: 10, top: 10, width: 40, height: 40 } },
  ]);
  assert.deepEqual(m, { a: ["w1"], b: [] });
});

test("fenceMembership: a window whose centre is outside every fence belongs nowhere", () => {
  const m = load().fenceMembership(AB, [
    { id: "w1", rect: { left: 400, top: 400, width: 40, height: 40 } },
  ]);
  assert.deepEqual(m, { a: [], b: [] });
  assert.equal(memberCount(m), 0);
});

test("fenceMembership: a window straddling the border belongs to the fence holding its centre", () => {
  // Spans 60..140 across the shared edge at 100; centre x = 90, inside A.
  const m = load().fenceMembership(AB, [
    { id: "w1", rect: { left: 60, top: 20, width: 60, height: 40 } },
  ]);
  assert.deepEqual(m, { a: ["w1"], b: [] });
  assert.equal(memberCount(m), 1, "a straddling window belongs to exactly ONE fence");
});

test("fenceMembership: a centre exactly on the shared edge belongs to the RIGHT fence", () => {
  // NEGATIVE CONTROL: a CLOSED containment test (`cx <= left + width`) hands
  // this centre to A — the fence it is leaving — and a `break`-less fold lists
  // it under both. Half-open on the far edge is what makes "exactly one" hold
  // for abutting fences.
  const m = load().fenceMembership(AB, [
    { id: "w1", rect: { left: 80, top: 20, width: 40, height: 40 } },
  ]);
  assert.deepEqual(m, { a: [], b: ["w1"] });
  assert.equal(memberCount(m), 1);
});

test("fenceMembership: a centre exactly on the shared HORIZONTAL edge belongs to the LOWER fence", () => {
  const m = load().fenceMembership(TB, [
    { id: "w1", rect: { left: 20, top: 80, width: 40, height: 40 } },
  ]);
  assert.deepEqual(m, { t: [], u: ["w1"] });
  assert.equal(memberCount(m), 1);
});

test("fenceMembership: a window straddling the horizontal border belongs to the fence holding its centre", () => {
  const m = load().fenceMembership(TB, [
    { id: "w1", rect: { left: 20, top: 60, width: 40, height: 60 } },
  ]);
  assert.deepEqual(m, { t: ["w1"], u: [] });
  assert.equal(memberCount(m), 1);
});

test("fenceMembership: no fences at all is an empty map, whatever the windows", () => {
  assert.deepEqual(
    load().fenceMembership([], [{ id: "w1", rect: { left: 0, top: 0, width: 10, height: 10 } }]),
    {},
  );
});

test("fenceFits: a fence compared against ITSELF by id fits — a move must not refuse its own start", () => {
  assert.equal(load().fenceFits(EXISTING, { id: "e", rect: FIT }), true);
});

test("tileIntoRect: every tile of every row lies inside the target rect", () => {
  const wb = load();
  for (const row of TILES) {
    const tiles = wb.tileIntoRect(row.rect, members(row.n));
    assert.equal(tiles.length, row.n, `${row.name}: one rect per member`);
    for (const t of tiles) {
      // Asserted as a RELATION, not against the expected numbers above: an
      // implementation returning the right COUNT of wrong rects must still red.
      const detail = `${row.name}: ${JSON.stringify(t)} escapes ${JSON.stringify(row.rect)}`;
      assert.ok(t.left >= row.rect.left, detail);
      assert.ok(t.top >= row.rect.top, detail);
      assert.ok(t.left + t.width <= row.rect.left + row.rect.width, detail);
      assert.ok(t.top + t.height <= row.rect.top + row.rect.height, detail);
      assert.ok(t.width > 0, detail);
      assert.ok(t.height > 0, detail);
    }
  }
});
