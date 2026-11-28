// Unit tests for assets/ui/wb-geometry.ts: views, landings, pans and spawn rects.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";

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
    const got = WBGeometry.bringIntoView(row.target, VIEW, row.extent || EXT);
    assert.deepEqual(got, row.want);
  });
}

// ---- anchorIntoView ----------------------------------------------------------
// The fence jump's own anchoring: the target's TOP-LEFT corner one inset in from
// the viewport's, through the same clamp. A fence is a region worked inside, not
// a point of interest looked at, so centring it wastes the screen above and left
// of it. `bringIntoView` above is untouched and still serves the Go-to picker.
const ANCHOR = [
  {
    name: "the target's corner lands one inset in from the viewport's",
    target: { left: 600, top: 400, width: 200, height: 100 },
    want: { left: 576, top: 376 },
  },
  {
    // NEGATIVE CONTROL against the centring fold: `bringIntoView` answers
    // {200,100} for this same target, so a call routed to the wrong one is red.
    name: "the SIZE of the target changes nothing — only its corner is read",
    target: { left: 600, top: 400, width: 1600, height: 1200 },
    want: { left: 576, top: 376 },
  },
  {
    name: "a target near the origin clamps at the pinned origin rather than going negative",
    target: { left: 10, top: 4, width: 200, height: 100 },
    want: { left: 0, top: 0 },
  },
  {
    name: "a target at the far corner never scrolls past `extent - viewport`",
    target: { left: 1900, top: 1400, width: 200, height: 150 },
    want: { left: 1000, top: 800 },
  },
  {
    // The whole point of the plane's new headroom: with one viewport of room
    // past the content, the corner offset is INSIDE the ceiling and survives the
    // clamp. `stageExtent`'s table is what guarantees this extent is real.
    name: "with a viewport of headroom past it, a far fence really reaches the corner",
    target: { left: 2200, top: 1500, width: 720, height: 460 },
    extent: { width: 3920, height: 2660 },
    want: { left: 2176, top: 1476 },
  },
  {
    name: "the inset is overridable, and zero means flush against the corner",
    target: { left: 600, top: 400, width: 200, height: 100 },
    inset: 0,
    want: { left: 600, top: 400 },
  },
];

for (const row of ANCHOR) {
  test(`anchorIntoView: ${row.name}`, () => {
    const got = WBGeometry.anchorIntoView(row.target, VIEW, row.extent || EXT, row.inset);
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
    const got = WBGeometry.viewLanding(row.stored, row.rects, LAND_VIEW, LAND_EXT);
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
    const got = WBGeometry.panNudge(row.pointer, row.viewport || PAN_VIEW, BAND, STEP);
    assert.deepEqual(got, row.want);
  });
}

// Where a console born into a focused fence lands. The fence has a NON-ZERO
// origin as a built-in negative control: an implementation that cascades from
// 0,0 and forgets `fence.left`/`fence.top` reds every row.
const SPAWN_F = { left: 700, top: 300, width: 600, height: 460 };
const HEAD = 28;

const SPAWNS = [
  {
    name: "the first console takes the fence's full inner box",
    fence: SPAWN_F,
    index: 0,
    want: { left: 712, top: 340, width: 560, height: 340 },
  },
  {
    // NEGATIVE CONTROL for the room cap: a bare `k * step` answers
    // `left: 772, top: 412` here — 60 and 72 px of cascade in a fence with only
    // 16 and 68 px of slack, walking the window out of its own fence.
    name: "a later slot cascades only as far as the fence has room",
    fence: SPAWN_F,
    index: 3,
    want: { left: 728, top: 408, width: 560, height: 340 },
  },
  {
    name: "the cascade wraps at eight, so slot 8 is slot 0 again",
    fence: SPAWN_F,
    index: 8,
    want: { left: 712, top: 340, width: 560, height: 340 },
  },
  {
    // A fence smaller than `.session-window`'s CSS floor (240x150): the box is
    // BELOW it on both axes, which is exactly what the caller relaxes inline.
    name: "a small fence yields a box below the CSS floor rather than one that escapes",
    fence: { left: 0, top: 0, width: 200, height: 120 },
    index: 7,
    want: { left: 12, top: 40, width: 176, height: 68 },
  },
];

for (const row of SPAWNS) {
  test(`spawnRectIn: ${row.name}`, () => {
    assert.deepEqual(WBGeometry.spawnRectIn(row.fence, row.index, HEAD), row.want);
  });
}

test("spawnRectIn: the box lies inside the fence at every cascade slot", () => {
  const wb = WBGeometry;
  for (const row of SPAWNS) {
    for (let i = 0; i < 8; i++) {
      const b = wb.spawnRectIn(row.fence, i, HEAD);
      // Asserted as a RELATION: the right COUNT of wrong boxes must still red.
      const detail = `${row.name} slot ${i}: ${JSON.stringify(b)} escapes ${JSON.stringify(row.fence)}`;
      assert.ok(b.left >= row.fence.left, detail);
      assert.ok(b.top >= row.fence.top + HEAD, detail);
      assert.ok(b.left + b.width <= row.fence.left + row.fence.width, detail);
      assert.ok(b.top + b.height <= row.fence.top + row.fence.height, detail);
      assert.ok(b.width > 0 && b.height > 0, detail);
    }
  }
});

// ---- a console is never born held by a locked fence -------------------------
// The free cascade from a viewport at 0,0: slot k sits at 30+24k, 20+24k.
const FREE_VIEW = { left: 0, top: 0, width: 1000, height: 600 };
const FREE_BOX = { width: 560, height: 340 };

test("freeSpawnRect: with no fences, slot k is the plain viewport cascade", () => {
  const got = WBGeometry.freeSpawnRect(FREE_VIEW, 2, []);
  assert.deepEqual(got, { rect: { left: 78, top: 68, ...FREE_BOX }, moved: false });
});

test("freeSpawnRect: an UNLOCKED fence under the slot does not move the console", () => {
  const fence = { id: "f", rect: { left: 0, top: 0, width: 800, height: 500 }, locked: false };
  const got = WBGeometry.freeSpawnRect(FREE_VIEW, 0, [fence]);
  assert.deepEqual(got, { rect: { left: 30, top: 20, ...FREE_BOX }, moved: false });
});

test("freeSpawnRect: a slot a locked fence holds is skipped for the next free one", () => {
  // Slot 0's title bar point is (310, 36); slot 1's is (334, 60). The fence
  // holds only the first one.
  const fence = { id: "f", rect: { left: 300, top: 30, width: 20, height: 20 }, locked: true };
  const wb = WBGeometry;
  const got = wb.freeSpawnRect(FREE_VIEW, 0, [fence]);
  assert.deepEqual(got, { rect: { left: 54, top: 44, ...FREE_BOX }, moved: false });
  assert.equal(wb.fenceHolds([fence], got.rect, false), false);
});

test("freeSpawnRect: a locked fence over every slot pushes the console past its right edge", () => {
  const big = { id: "big", rect: { left: 0, top: 0, width: 1200, height: 800 }, locked: true };
  // The box past `big` lands in a second locked fence: the walk passes that too.
  const next = { id: "next", rect: { left: 1200, top: 0, width: 700, height: 800 }, locked: true };
  const wb = WBGeometry;
  const got = wb.freeSpawnRect(FREE_VIEW, 0, [big, next]);
  assert.equal(got.moved, true);
  assert.deepEqual(got.rect, { left: 1912, top: 20, ...FREE_BOX });
  assert.equal(wb.fenceHolds([big, next], got.rect, false), false);
});

test("freeSpawnRect: the cascade is anchored at the viewport offset", () => {
  const got = WBGeometry.freeSpawnRect({ left: 3000, top: 900, width: 0, height: 0 }, 0, []);
  // An unmeasurable viewport takes the plain caps.
  assert.deepEqual(got.rect, { left: 3030, top: 920, ...FREE_BOX });
});

// The popup re-origins a detached fence's members to its own top-left, while
// the fence records keep the stage coordinates. A member of a locked fence at
// the stage origin lands inside that same fence rect, so the stage's answer is
// "held" and the popup's answer must be "free".
test("fenceHolds: a locked fence holds its console on the stage, never in the popup", () => {
  const WB = WBGeometry;
  const fences = [{ id: "f-a", rect: { left: 0, top: 0, width: 800, height: 600 }, locked: true }];
  const rect = { left: 12, top: 12, width: 300, height: 200 };
  assert.equal(WB.fenceHolds(fences, rect, false), true, "the stage honours the fence lock");
  assert.equal(WB.fenceHolds(fences, rect, true), false, "the popup moves the console freely");
  const open = [{ ...fences[0], locked: false }];
  assert.equal(WB.fenceHolds(open, rect, false), false, "an unlocked fence holds nothing");
});

// ---- clampOffset, slideEase, bboxOf ------------------------------------------
test("clampOffset keeps an offset in [0, extent - viewport] on each axis", () => {
  const view = { width: 1000, height: 700 };
  const ext = { width: 2000, height: 1500 };
  assert.deepEqual(WBGeometry.clampOffset({ left: 500, top: 300 }, view, ext), {
    left: 500,
    top: 300,
  });
  assert.deepEqual(WBGeometry.clampOffset({ left: -40, top: -1 }, view, ext), { left: 0, top: 0 });
  assert.deepEqual(WBGeometry.clampOffset({ left: 5000, top: 5000 }, view, ext), {
    left: 1000,
    top: 800,
  });
  // A viewport bigger than the extent asks for the origin, never a negative offset.
  assert.deepEqual(
    WBGeometry.clampOffset({ left: 50, top: 50 }, view, { width: 800, height: 600 }),
    { left: 0, top: 0 },
  );
});

test("slideEase runs 0 to 1, fast first, and clamps its clock", () => {
  assert.equal(WBGeometry.slideEase(0), 0);
  assert.equal(WBGeometry.slideEase(1), 1);
  assert.equal(WBGeometry.slideEase(0.5), 0.875);
  assert.equal(WBGeometry.slideEase(-3), 0);
  assert.equal(WBGeometry.slideEase(7), 1);
});

test("bboxOf wraps every rect, and an empty list is the zero box at the origin", () => {
  assert.deepEqual(WBGeometry.bboxOf([]), { left: 0, top: 0, width: 0, height: 0 });
  assert.deepEqual(WBGeometry.bboxOf(), { left: 0, top: 0, width: 0, height: 0 });
  assert.deepEqual(
    WBGeometry.bboxOf([
      { left: 100, top: 50, width: 200, height: 100 },
      { left: 400, top: 20, width: 50, height: 400 },
    ]),
    { left: 100, top: 20, width: 350, height: 400 },
  );
});

// A rect with no edges reads as a zero box at the origin, on either side: it
// overlaps a box that covers the origin, and nothing else.
test("rectsOverlap: an undefined or empty rect is a zero box at the origin", () => {
  const AROUND = { left: -10, top: -10, width: 20, height: 20 };
  const AWAY = { left: 10, top: 10, width: 20, height: 20 };
  for (const empty of [undefined, {}]) {
    assert.equal(WBGeometry.rectsOverlap(empty, AROUND), true, `${JSON.stringify(empty)} inside`);
    assert.equal(WBGeometry.rectsOverlap(AROUND, empty), true, `${JSON.stringify(empty)} as the second rect`);
    assert.equal(WBGeometry.rectsOverlap(empty, AWAY), false, `${JSON.stringify(empty)} away`);
  }
});

// ---- what the folds read when a rect, a point or a viewport is missing -------

test("stageExtent of no rects and no viewport is the margin on both sides", () => {
  assert.deepEqual(WBGeometry.stageExtent(null, null), { width: 200, height: 200 });
});

test("rectHolds of a missing rect holds nothing, not even the origin", () => {
  assert.equal(WBGeometry.rectHolds(null, { x: 0, y: 0 }), false);
  assert.equal(WBGeometry.rectHolds(), false);
});

test("panNudge with no band and no step is the call with 48 and 24", () => {
  for (const pointer of [{ x: 1100, y: 850 }, { x: 1076, y: 400 }, { x: 100, y: 50 }]) {
    assert.deepEqual(WBGeometry.panNudge(pointer, PAN_VIEW), WBGeometry.panNudge(pointer, PAN_VIEW, 48, 24));
  }
});

test("panNudge of a missing pointer and a missing viewport does not pan", () => {
  assert.deepEqual(WBGeometry.panNudge(null, null, BAND, STEP), { dx: 0, dy: 0 });
});
