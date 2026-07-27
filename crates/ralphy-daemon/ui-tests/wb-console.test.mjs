// Unit tests for assets/ui/wb-console.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-console.js"),
  "utf8",
);

function load() {
  // The three globals the module touches at LOAD time: `window.addEventListener`
  // (the pagehide flush), `document.readyState`/`addEventListener` (the boot
  // hooks — "loading" parks them on a no-op listener instead of running them
  // against a DOM that does not exist) and `location.protocol`/`host`
  // (WS_ORIGIN). No `ResizeObserver` is injected ON PURPOSE: the surviving one
  // lives inside `attachTerminal`, which this harness never reaches, so a
  // module-scope observer re-added alongside a clamp fails LOUDLY here.
  const window = { addEventListener() {} };
  const document = { readyState: "loading", addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  new Function("window", "document", "location", SRC)(window, document, location);
  return window.WBConsole;
}

const VIEWPORT = { width: 1000, height: 700 };
const MARGIN = 200;

// The stage extent: the bbox of the window rects plus a margin of breathing
// room, unioned per axis with the viewport. Origin pinned at 0,0.
const TABLE = [
  {
    name: "an empty stage is exactly the viewport — the scrollbar measures nothing",
    rects: [],
    want: { width: 1000, height: 700 },
  },
  {
    name: "a window well inside the viewport does not grow the stage",
    rects: [{ left: 40, top: 40, width: 600, height: 380 }],
    want: { width: 1000, height: 700 },
  },
  {
    name: "a window past the viewport on X grows the stage on X only",
    rects: [{ left: 900, top: 40, width: 600, height: 380 }],
    want: { width: 1700, height: 700 },
  },
  {
    name: "a window past the viewport on Y grows the stage on Y only",
    rects: [{ left: 40, top: 600, width: 600, height: 380 }],
    want: { width: 1000, height: 1180 },
  },
  {
    name: "two windows: each axis takes its extent from whichever window reaches furthest",
    rects: [
      { left: 900, top: 40, width: 600, height: 380 },
      { left: 40, top: 600, width: 600, height: 380 },
    ],
    want: { width: 1700, height: 1180 },
  },
  {
    name: "a viewport larger than the bbox wins on both axes",
    rects: [{ left: 900, top: 40, width: 600, height: 380 }],
    viewport: { width: 2000, height: 1500 },
    want: { width: 2000, height: 1500 },
  },
  {
    // NEGATIVE CONTROL: red if the margin is dropped, or if the union is
    // written `Math.max(viewport, right)` instead of `Math.max(viewport,
    // right + margin)` — both would answer 1000 here.
    name: "a window exactly filling the viewport width still buys a margin of drag room",
    rects: [{ left: 0, top: 0, width: 1000, height: 100 }],
    want: { width: 1200, height: 700 },
  },
];

for (const row of TABLE) {
  test(`stageExtent: ${row.name}`, () => {
    const got = load().stageExtent(row.rects, row.viewport || VIEWPORT, MARGIN);
    assert.deepEqual(got, row.want);
  });
}

test("stageExtent falls back to the module's own margin when none is passed", () => {
  const withMargin = load().stageExtent(
    [{ left: 0, top: 0, width: 1000, height: 100 }],
    VIEWPORT,
    200,
  );
  const defaulted = load().stageExtent(
    [{ left: 0, top: 0, width: 1000, height: 100 }],
    VIEWPORT,
  );
  assert.deepEqual(defaulted, withMargin);
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

// ---- bringIntoView (issue #337) ---------------------------------------------
// The scroll offsets that CENTRE a target rect in the viewport, clamped to the
// extent. Always centres — it is not a scroll-into-view-if-needed (ADR-0051 §7's
// fence jump wants the same slide for a partially visible target).
const VIEW = { width: 1000, height: 700 };
const EXT = { width: 2000, height: 1500 };

const BRING = [
  {
    name: "a target already in view is still CENTRED",
    target: { left: 600, top: 400, width: 200, height: 100 },
    want: { left: 200, top: 100 },
  },
  {
    name: "off the right edge",
    target: { left: 1200, top: 400, width: 200, height: 100 },
    want: { left: 800, top: 100 },
  },
  {
    name: "off the left edge clamps at the pinned origin",
    target: { left: 0, top: 400, width: 200, height: 100 },
    want: { left: 0, top: 100 },
  },
  {
    name: "off the bottom edge",
    target: { left: 600, top: 900, width: 200, height: 100 },
    want: { left: 200, top: 600 },
  },
  {
    name: "off the top edge clamps at the pinned origin",
    target: { left: 600, top: 0, width: 200, height: 100 },
    want: { left: 200, top: 0 },
  },
  {
    name: "a target LARGER than the viewport centres on its own middle",
    target: { left: 200, top: 100, width: 1600, height: 1200 },
    want: { left: 500, top: 350 },
  },
  {
    name: "a target at the far corner never scrolls past `extent - viewport`",
    target: { left: 1800, top: 1350, width: 200, height: 150 },
    want: { left: 1000, top: 800 },
  },
  {
    // The ceiling itself, un-clamped: distinguishes "clamped at the boundary"
    // from "the arithmetic happened to land there".
    name: "a target whose centring lands EXACTLY on the ceiling is not clamped away",
    target: { left: 1400, top: 1000, width: 200, height: 100 },
    want: { left: 1000, top: 700 },
  },
  {
    // NEGATIVE CONTROL: drop the final `Math.max(0, …)` and this answers
    // {-200,-100}, which the DOM would silently swallow as 0,0 — the bug would
    // be invisible in the browser and only ever show up here. It discriminates
    // only because that floor is the axis's ONLY floor: flooring the ceiling
    // too would make both spellings answer 0 and this row unfalsifiable.
    name: "a viewport LARGER than the extent yields the origin, never a negative offset",
    target: { left: 600, top: 400, width: 200, height: 100 },
    extent: { width: 800, height: 600 },
    want: { left: 0, top: 0 },
  },
];

for (const row of BRING) {
  test(`bringIntoView: ${row.name}`, () => {
    const got = load().bringIntoView(row.target, VIEW, row.extent || EXT);
    assert.deepEqual(got, row.want);
  });
}

// ---- viewLanding (issue #339) ------------------------------------------------
// Where the viewport lands on load: the stored per-client offset when it still
// SHOWS work, otherwise the bounding box of the restored windows. Both legs end
// in the same clamp, so a stored offset from a bigger screen is pulled into the
// current extent rather than silently swallowed by the DOM.
const LAND_VIEW = { width: 1052, height: 854 };
const LAND_EXT = { width: 3200, height: 2080 };
const LAND_RECTS = [
  { left: 1600, top: 900, width: 600, height: 380 },
  { left: 2400, top: 1500, width: 600, height: 380 },
];
// The bbox landing these rects imply: bbox {1600,900,1400x980} centred in a
// 1052x854 viewport → 1600 + 700 - 526 = 1774, 900 + 490 - 427 = 963.
const BBOX_LANDING = { left: 1774, top: 963 };

const LAND = [
  {
    name: "an empty stage with nothing stored lands on the origin",
    stored: null,
    rects: [],
    want: { left: 0, top: 0 },
  },
  {
    name: "nothing stored lands on the bounding box of the restored windows",
    stored: null,
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
  {
    // DELIBERATELY not the bbox landing: a `viewLanding` that ignored `stored`
    // entirely would pass this row if the two coincided, which is the one
    // mutant the honour path exists to catch.
    name: "a stored offset that still shows a window is honoured verbatim",
    stored: { left: 1500, top: 850 },
    rects: LAND_RECTS,
    want: { left: 1500, top: 850 },
  },
  {
    // NEGATIVE CONTROL: 0,0 is a perfectly well-formed stored offset that shows
    // NO window here. Invert the intersection test (or drop it and trust any
    // stored pair) and this row answers {0,0} instead of the bbox landing.
    name: "a stored offset showing no window at all falls back to the bounding box",
    stored: { left: 0, top: 0 },
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
  {
    name: "a stored offset past the extent is clamped, not discarded",
    stored: { left: 9999, top: 9999 },
    rects: LAND_RECTS,
    want: { left: 2148, top: 1226 },
  },
  {
    name: "a corrupt stored offset is treated as absent",
    stored: { left: "x", top: null },
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
];

for (const row of LAND) {
  test(`viewLanding: ${row.name}`, () => {
    const got = load().viewLanding(row.stored, row.rects, LAND_VIEW, LAND_EXT);
    assert.deepEqual(got, row.want);
  });
}

// ---- panNudge (issue #337) ---------------------------------------------------
// How far the plane scrolls per animation frame while a window is dragged
// against the viewport edge. `viewport` is a CLIENT rect with a NON-ZERO origin
// on purpose: a handler that mistakes client coordinates for viewport-relative
// ones reds every EDGE row here — the centre row answers {0,0} under that
// mutant too, so it is not the one doing the discriminating.
const PAN_VIEW = { left: 100, top: 50, right: 1100, bottom: 850 };
const BAND = 48;
const STEP = 24;

const NUDGE = [
  { name: "the middle of the viewport does not pan", pointer: { x: 600, y: 400 }, want: { dx: 0, dy: 0 } },
  {
    // NEGATIVE CONTROL: exactly at the band's outer lip. An off-by-one `<=`
    // starts the loop here, one pixel before the operator asked for it.
    name: "one pixel outside the band is still not panning",
    pointer: { x: 1052, y: 400 },
    want: { dx: 0, dy: 0 },
  },
  { name: "just inside the right band creeps", pointer: { x: 1054, y: 400 }, want: { dx: 1, dy: 0 } },
  { name: "half-way into the right band is half speed", pointer: { x: 1076, y: 400 }, want: { dx: 12, dy: 0 } },
  { name: "at the right edge is full speed", pointer: { x: 1100, y: 400 }, want: { dx: 24, dy: 0 } },
  {
    name: "PAST the right edge is capped, never faster",
    pointer: { x: 1400, y: 400 },
    want: { dx: 24, dy: 0 },
  },
  { name: "at the left edge pans back", pointer: { x: 100, y: 400 }, want: { dx: -24, dy: 0 } },
  { name: "at the top edge pans up", pointer: { x: 600, y: 50 }, want: { dx: 0, dy: -24 } },
  { name: "at the bottom edge pans down", pointer: { x: 600, y: 850 }, want: { dx: 0, dy: 24 } },
  {
    name: "a corner pans on both axes at once",
    pointer: { x: 1100, y: 850 },
    want: { dx: 24, dy: 24 },
  },
  {
    name: "the opposite corner pans back on both axes",
    pointer: { x: 100, y: 50 },
    want: { dx: -24, dy: -24 },
  },
  {
    // NEGATIVE CONTROL: a viewport narrower than two bands. The rule is written
    // as a DIFFERENCE of the two edge pressures so they cancel here; a naive
    // "in the right band → +step, else in the left band → -step" answers
    // {dx: 24} and the plane oscillates under the operator.
    name: "a viewport narrower than two bands cancels instead of oscillating",
    viewport: { left: 0, top: 0, right: 60, bottom: 850 },
    pointer: { x: 30, y: 400 },
    want: { dx: 0, dy: 0 },
  },
];

for (const row of NUDGE) {
  test(`panNudge: ${row.name}`, () => {
    const got = load().panNudge(row.pointer, row.viewport || PAN_VIEW, BAND, STEP);
    assert.deepEqual(got, row.want);
  });
}

test("panNudge mutates neither argument", () => {
  const pointer = { x: 1076, y: 400 };
  const viewport = { left: 100, top: 50, right: 1100, bottom: 850 };
  const before = structuredClone([pointer, viewport]);
  load().panNudge(pointer, viewport, BAND, STEP);
  assert.deepEqual([pointer, viewport], before);
});

test("bringIntoView mutates neither argument", () => {
  const target = { left: 1200, top: 400, width: 200, height: 100 };
  const viewport = { width: 1000, height: 700 };
  const extent = { width: 2000, height: 1500 };
  const before = structuredClone([target, viewport, extent]);
  load().bringIntoView(target, viewport, extent);
  assert.deepEqual([target, viewport, extent], before);
});

// ---- where a new fence lands (issue #340) -----------------------------------
// A deterministic 2-column grid anchored at the viewport's CURRENT offset, sized
// to the viewport and clamped to a floor.
const FENCE_VIEW = { width: 1400, height: 900 };
const ORIGIN = { left: 0, top: 0 };

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

// Which slot a NEW fence takes. Indexing by `fences.length` reuses a slot after
// a removal, which is how an overlap ships before ADR-0051 §6 exists to enforce
// it away — every row below is that bug's oracle.
const SLOTS = [
  { name: "the first fence on an empty plane takes slot 0", rects: [], want: 0 },
  {
    name: "a second fence takes the next free slot",
    rects: [{ left: 40, top: 40, width: 720, height: 460 }],
    want: 1,
  },
  {
    // NEGATIVE CONTROL: `fences.length` answers 2 here and lands the new fence
    // exactly on the surviving slot-2 rect.
    name: "after the middle of three is removed, the FREED slot is reused, not the survivor's",
    rects: [
      { left: 40, top: 40, width: 720, height: 460 },
      { left: 40, top: 524, width: 720, height: 460 },
    ],
    want: 1,
  },
  {
    name: "a plane whose first four slots are full spills into the fifth",
    rects: [0, 1, 2, 3].map((i) => ({
      left: 40 + (i % 2) * 744,
      top: 40 + Math.floor(i / 2) * 484,
      width: 720,
      height: 460,
    })),
    want: 4,
  },
];

for (const row of SLOTS) {
  test(`nextFenceSlot: ${row.name}`, () => {
    assert.equal(load().nextFenceSlot(row.rects, ORIGIN, FENCE_VIEW), row.want);
  });
}

test("nextFenceSlot: the slot it picks never overlaps an existing fence", () => {
  const wb = load();
  // Three fences with a HOLE at slot 1 — the shape a removal leaves behind.
  const rects = [0, 2, 3].map((i) => wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, i));
  const slot = wb.nextFenceSlot(rects, ORIGIN, FENCE_VIEW);
  const born = wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, slot);
  const overlaps = (a, b) =>
    a.left < b.left + b.width &&
    a.left + a.width > b.left &&
    a.top < b.top + b.height &&
    a.top + a.height > b.top;
  for (const r of rects) {
    assert.ok(!overlaps(born, r), `slot ${slot} lands on ${JSON.stringify(r)}`);
  }
});

// ---- a fence is a group (issue #341) ----------------------------------------
// Membership is DERIVED from the centre point, never stored: no window record
// gains a `fenceId`, so a fence and a window can never disagree about it.
// Containment is HALF-OPEN (`left <= cx < left + width`) because fences may
// abut: a closed test would put a centre on a shared border in two fences.
const AB = [
  { id: "a", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "b", rect: { left: 100, top: 0, width: 100, height: 100 } },
];
// Total member ids across every fence — the "exactly one fence" oracle.
const memberCount = (m) => Object.values(m).reduce((n, ids) => n + ids.length, 0);

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

test("fenceMembership: a fence with no members maps to an empty list", () => {
  assert.deepEqual(load().fenceMembership(AB, []), { a: [], b: [] });
});

test("fenceMembership: no fences at all is an empty map, whatever the windows", () => {
  assert.deepEqual(
    load().fenceMembership([], [{ id: "w1", rect: { left: 0, top: 0, width: 10, height: 10 } }]),
    {},
  );
});

// Whether a fence's candidate rect may take the plane: it must overlap no OTHER
// fence. Abutting is allowed — one predicate for spawn and for enforcement.
const FIT = { left: 100, top: 100, width: 100, height: 100 };
const EXISTING = [{ id: "e", rect: FIT }];

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

test("fenceFits: a fence compared against ITSELF by id fits — a move must not refuse its own start", () => {
  assert.equal(load().fenceFits(EXISTING, { id: "e", rect: FIT }), true);
});

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
