// Unit tests for assets/ui/wb-geometry.ts: the stage extent and the fence fits,
// holds and moves.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";
import { EXISTING, FENCE_VIEW, FIT, ORIGIN } from "./wb-geometry-fixture.mjs";

const MARGIN = 200;

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
    const got = WBGeometry.stageExtent(row.rects, row.viewport || VIEWPORT, MARGIN);
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
    const got = WBGeometry.fenceSpawnRect(
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
    assert.equal(WBGeometry.rectHolds(HOLDS_RECT, row.point), row.want);
  });
}


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
    assert.equal(WBGeometry.fenceFits(EXISTING, { id: "c", rect: row.rect }), row.want);
  });
}


test("fenceFits: an empty fence list fits anything", () => {
  assert.equal(WBGeometry.fenceFits([], { id: "c", rect: FIT }), true);
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
    assert.deepEqual(WBGeometry.fenceMoveDelta(row.delta, row.fence, row.members), row.want);
  });
}
