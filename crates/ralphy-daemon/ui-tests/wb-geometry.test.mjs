// Unit tests for assets/ui/wb-geometry.ts — the workbench's plane geometry.
//
// These tests came over from wb-console.test.mjs with the code they exercise
// (ADR-0022 §3, restated for assets as ADR-0057 D4). Not one assertion was
// rewritten in the move: they say what they said before the extraction, which
// is the whole point of moving them rather than writing new ones. The only
// change is the loader — this module needs no DOM, no socket and no siblings,
// which is what "pure" bought.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";

// No global at all: the module exports its namespace. Compare with the loader
// in wb-console.test.mjs, which needs a window, a document, a location and a
// hidden BroadcastChannel — that gap is the argument for the extraction,
// stated as code.
function load() {
  return WBGeometry;
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

// `tileIntoRect(rect, members)` is the old global Arrange generalised: target
// rect plus member list in, one rect per member out, in order. The grid is
// today's — `cols = ceil(sqrt(n))` — and aspect-independent on purpose: making
// it follow the rect's aspect is a second change hiding inside a move.
//
// The base rect has a NON-ZERO origin as a built-in negative control: an
// implementation that tiles from 0,0 and forgets `rect.left`/`rect.top` reds
// every row below.
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

// --- fenceOf: the one fence a rect belongs to ---------------------------------
test("fenceOf answers the fence holding the rect's centre, and null outside every fence", () => {
  const { fenceOf } = load();
  assert.equal(fenceOf(AB, { left: 10, top: 10, width: 20, height: 20 })?.id, "a");
  assert.equal(fenceOf(AB, { left: 150, top: 10, width: 20, height: 20 })?.id, "b");
  assert.equal(fenceOf(AB, { left: 300, top: 300, width: 20, height: 20 }), null);
  assert.equal(fenceOf([], { left: 10, top: 10, width: 20, height: 20 }), null);
  assert.equal(fenceOf(undefined, { left: 10, top: 10, width: 20, height: 20 }), null);
});

test("fenceOf is half-open on the far edge, like fenceMembership: the shared edge is the RIGHT fence's", () => {
  const { fenceOf } = load();
  assert.equal(fenceOf(AB, { left: 80, top: 20, width: 40, height: 40 })?.id, "b");
  assert.equal(fenceOf(TB, { left: 20, top: 80, width: 40, height: 40 })?.id, "u");
});

test("fenceOf never disagrees with fenceMembership", () => {
  const { fenceOf, fenceMembership } = load();
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

for (const row of TILES) {
  test(`tileIntoRect: ${row.name}`, () => {
    assert.deepEqual(load().tileIntoRect(row.rect, members(row.n)), row.want);
  });
}

// ---- came over from wb-console.test.mjs: the WBGeometry tables ------------
// wb-console.ts re-exports these functions; they are tested here, against
// the module that owns them.
const VIEWPORT = { width: 1000, height: 700 };

// The stage extent: the bbox of the window rects plus breathing room past it,
// unioned per axis with the viewport. Origin pinned at 0,0.
//
// The breathing room is `max(margin, viewport)` per axis, not the bare margin:
// the plane carries a FULL VIEWPORT past its furthest content so that any item
// can be scrolled flush to the top-left corner (`anchorIntoView` below computes
// that offset; without the headroom `clampOffset` would swallow it and the
// fence would stop mid-screen). With a 1000×700 viewport the room is therefore
// 1000 and 700 — the 200 constant only ever bites on a viewport smaller than it.
const TABLE = [
  {
    // The viewport leg still wins with nothing on the plane, so an empty stage
    // does not invent a scrollbar over emptiness (ADR-0051 §2).
    name: "an empty stage is exactly the viewport — the scrollbar measures nothing",
    rects: [],
    want: { width: 1000, height: 700 },
  },
  {
    // Was `{1000,700}` before the headroom: a window well inside the viewport
    // used to leave the plane unscrollable, which is exactly what pinned it to
    // the middle of the screen with no way to reach the corner.
    name: "a window well inside the viewport still buys a viewport of headroom",
    rects: [{ left: 40, top: 40, width: 600, height: 380 }],
    want: { width: 1640, height: 1120 },
  },
  {
    name: "a window past the viewport on X reaches further on X than on Y",
    rects: [{ left: 900, top: 40, width: 600, height: 380 }],
    want: { width: 2500, height: 1120 },
  },
  {
    name: "a window past the viewport on Y reaches further on Y than on X",
    rects: [{ left: 40, top: 600, width: 600, height: 380 }],
    want: { width: 1640, height: 1680 },
  },
  {
    name: "two windows: each axis takes its extent from whichever window reaches furthest",
    rects: [
      { left: 900, top: 40, width: 600, height: 380 },
      { left: 40, top: 600, width: 600, height: 380 },
    ],
    want: { width: 2500, height: 1680 },
  },
  {
    // The headroom is the CURRENT viewport's, so a bigger browser buys a bigger
    // plane rather than the same one: 1500 + 2000 across, 420 + 1500 down.
    name: "the headroom scales with the viewport, on both axes",
    rects: [{ left: 900, top: 40, width: 600, height: 380 }],
    viewport: { width: 2000, height: 1500 },
    want: { width: 3500, height: 1920 },
  },
  {
    // NEGATIVE CONTROL for the headroom itself: with the bare 200 margin this
    // answers {1200, 700}, and with no margin at all {1000, 700}. Only the
    // `max(margin, viewport)` spelling lands here.
    name: "a window exactly filling the viewport is followed by a whole viewport of room",
    rects: [{ left: 0, top: 0, width: 1000, height: 100 }],
    want: { width: 2000, height: 800 },
  },
  {
    // The FLOOR leg, isolated: on a viewport narrower than the constant the 200
    // is what applies, so the margin argument is not dead code.
    name: "a viewport smaller than the margin falls back to the margin",
    rects: [{ left: 0, top: 0, width: 300, height: 300 }],
    viewport: { width: 120, height: 90 },
    want: { width: 500, height: 500 },
  },
];

for (const row of TABLE) {
  test(`stageExtent: ${row.name}`, () => {
    const got = load().stageExtent(row.rects, row.viewport || VIEWPORT, MARGIN);
    assert.deepEqual(got, row.want);
  });
}

// ---- where a new fence lands (issue #340) -----------------------------------
// A deterministic 2-column grid anchored at the viewport's CURRENT offset, sized
// to the viewport and clamped to a floor.
const FENCES = [
  {
    name: "the first fence lands one inset into the current view",
    index: 0,
    want: { left: 40, top: 40, width: 720, height: 460 },
  },
  {
    name: "the second sits beside it, one gap across",
    index: 1,
    want: { left: 784, top: 40, width: 720, height: 460 },
  },
  {
    name: "the third wraps to the next row",
    index: 2,
    want: { left: 40, top: 524, width: 720, height: 460 },
  },
  {
    name: "the fourth completes the 2x2 block",
    index: 3,
    want: { left: 784, top: 524, width: 720, height: 460 },
  },
  {
    // NEGATIVE CONTROL: a fence born at the pinned origin instead of in the
    // current view reds this row — the operator would draw a fence they cannot
    // see, several screens back up the plane.
    name: "the anchor is the viewport's own offset, not the stage origin",
    offset: { left: 1000, top: 600 },
    index: 0,
    want: { left: 1040, top: 640, width: 720, height: 460 },
  },
  {
    name: "a viewport smaller than the default size shrinks the fence to fit",
    viewport: { width: 600, height: 400 },
    index: 1,
    want: { left: 584, top: 40, width: 520, height: 320 },
  },
  {
    // NEGATIVE CONTROL: without the `Math.max` floor this answers a 120-wide,
    // 40-tall fence — smaller than the box its own name field needs.
    name: "a tiny viewport still yields a usable fence, not a sliver",
    viewport: { width: 200, height: 120 },
    index: 0,
    want: { left: 40, top: 40, width: 240, height: 150 },
  },
];

for (const row of FENCES) {
  test(`fenceSpawnRect: ${row.name}`, () => {
    const got = load().fenceSpawnRect(
      row.offset || ORIGIN,
      row.viewport || FENCE_VIEW,
      row.index,
    );
    assert.deepEqual(got, row.want);
  });
}

// The RELATION, which survives a size or gap change the literals above do not.
// ADR-0051 §6's non-overlap enforcement is the next slice's, so this slice must
// not ship an overlap on the very first gesture.

// ---- a fence is a group (issue #341): `AB` above --------------------------
// The containment predicate itself (issue #343), extracted so membership and the
// floor's focus hit test share one spelling. Both axes are pinned: for a 2-D
// predicate, one axis is half the specification (#341's plan friction).
const HOLDS = [
  {
    name: "a point at the near corner is IN (the near edge is closed)",
    point: { x: 100, y: 200 },
    want: true,
  },
  { name: "a point in the middle is IN", point: { x: 150, y: 250 }, want: true },
  {
    // NEGATIVE CONTROL for X: a closed `x <= left + width` reds here.
    name: "a point on the far X edge is OUT",
    point: { x: 200, y: 250 },
    want: false,
  },
  {
    // NEGATIVE CONTROL for Y — the twin that a copy-pasted X-only test misses.
    name: "a point on the far Y edge is OUT",
    point: { x: 150, y: 300 },
    want: false,
  },
  { name: "a point left of the rect is OUT", point: { x: 99, y: 250 }, want: false },
  { name: "a point above the rect is OUT", point: { x: 150, y: 199 }, want: false },
];

const HOLDS_RECT = { left: 100, top: 200, width: 100, height: 100 };

for (const row of HOLDS) {
  test(`rectHolds: ${row.name}`, () => {
    assert.equal(load().rectHolds(HOLDS_RECT, row.point), row.want);
  });
}

test("fenceMembership: a fence with no members maps to an empty list", () => {
  assert.deepEqual(load().fenceMembership(AB, []), { a: [], b: [] });
});


// Whether a fence's candidate rect may take the plane: it must overlap no OTHER
// fence. Abutting is allowed — one predicate for spawn and for enforcement.
const FITS = [
  { name: "an overlap from the north is refused", rect: { left: 100, top: 50, width: 100, height: 100 }, want: false },
  { name: "an overlap from the south is refused", rect: { left: 100, top: 150, width: 100, height: 100 }, want: false },
  { name: "an overlap from the east is refused", rect: { left: 150, top: 100, width: 100, height: 100 }, want: false },
  { name: "an overlap from the west is refused", rect: { left: 50, top: 100, width: 100, height: 100 }, want: false },
  {
    name: "a candidate wholly CONTAINING an existing fence is refused",
    rect: { left: 0, top: 0, width: 400, height: 400 },
    want: false,
  },
  {
    name: "a candidate wholly CONTAINED by an existing fence is refused",
    rect: { left: 120, top: 120, width: 40, height: 40 },
    want: false,
  },
  {
    // NEGATIVE CONTROL: a non-strict overlap test (`<=`) reds this row, and the
    // natural layout — fences drawn edge to edge — becomes unbuildable.
    name: "a candidate abutting exactly on the west edge fits",
    rect: { left: 0, top: 100, width: 100, height: 100 },
    want: true,
  },
  {
    // The Y twin of the control above: making `rectsOverlap` non-strict on the
    // Y comparisons ALONE leaves the west row green, so without this the table
    // never punishes vertically abutting fences becoming unbuildable.
    name: "a candidate abutting exactly on the north edge fits",
    rect: { left: 100, top: 0, width: 100, height: 100 },
    want: true,
  },
  {
    name: "a candidate abutting exactly on the south edge fits",
    rect: { left: 100, top: 200, width: 100, height: 100 },
    want: true,
  },
  {
    name: "a candidate far away fits",
    rect: { left: 900, top: 900, width: 100, height: 100 },
    want: true,
  },
];

for (const row of FITS) {
  test(`fenceFits: ${row.name}`, () => {
    assert.equal(load().fenceFits(EXISTING, { id: "c", rect: row.rect }), row.want);
  });
}


test("fenceFits: an empty fence list fits anything", () => {
  assert.equal(load().fenceFits([], { id: "c", rect: FIT }), true);
});

// The move delta, clamped so the plane's pinned origin holds: neither the fence
// NOR any member it carries may land at a negative coordinate (issue #336 — the
// stage grows right and down only).
const MOVES = [
  {
    name: "a delta that keeps everything positive passes through unchanged",
    delta: { dx: 120, dy: 80 },
    fence: { left: 200, top: 200, width: 100, height: 100 },
    members: [{ left: 220, top: 220, width: 40, height: 40 }],
    want: { dx: 120, dy: 80 },
  },
  {
    name: "a delta pushing the fence past the origin clamps to the fence's own left/top",
    delta: { dx: -500, dy: -400 },
    fence: { left: 200, top: 150, width: 100, height: 100 },
    members: [],
    want: { dx: -200, dy: -150 },
  },
  {
    // NEGATIVE CONTROL: clamping on the FENCE alone answers -200/-150 here and
    // parks the member at left = -20, off the plane's pinned origin.
    name: "a member further left than the fence is what the clamp answers to",
    delta: { dx: -500, dy: -400 },
    fence: { left: 200, top: 150, width: 400, height: 400 },
    members: [{ left: 180, top: 130, width: 40, height: 40 }],
    want: { dx: -180, dy: -130 },
  },
  {
    name: "no members at all clamps on the fence",
    delta: { dx: -50, dy: -50 },
    fence: { left: 20, top: 30, width: 100, height: 100 },
    members: [],
    want: { dx: -20, dy: -30 },
  },
  {
    name: "a positive delta is never clamped, however far it travels",
    delta: { dx: 9000, dy: 9000 },
    fence: { left: 0, top: 0, width: 100, height: 100 },
    members: [{ left: 0, top: 0, width: 10, height: 10 }],
    want: { dx: 9000, dy: 9000 },
  },
];

for (const row of MOVES) {
  test(`fenceMoveDelta: ${row.name}`, () => {
    assert.deepEqual(load().fenceMoveDelta(row.delta, row.fence, row.members), row.want);
  });
}
