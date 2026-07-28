// Unit tests for assets/ui/wb-console.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-console.js"), "utf8");
// The module reads `window.WBDeskSink.daemon()` at load, exactly as it does in
// the browser — so the harness runs the REAL sink source first, mirroring
// index.html's script order. A stub here would hide a broken script tag.
const SINK_SRC = readFileSync(join(UI, "wb-desk-sink.js"), "utf8");
// Same reasoning for the detach link: `wb-console.js` reads
// `window.WBDetachLink.link()` at load, so the harness runs the REAL source in
// the same script order both documents use. It touches neither `sessionStorage`
// nor `BroadcastChannel` at load, so neither needs to exist here.
const LINK_SRC = readFileSync(join(UI, "wb-detach-link.js"), "utf8");

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
  new Function("window", SINK_SRC)(window);
  new Function("window", LINK_SRC)(window);
  new Function("window", "document", "location", SRC)(window, document, location);
  return window.WBConsole;
}

const VIEWPORT = { width: 1000, height: 700 };
const MARGIN = 200;

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

// Measured on a viewport SMALLER than the constant: with a 1000×700 one the
// headroom outranks every margin and the two spellings agree by accident, which
// would make this row unfalsifiable. The third leg is what proves the default is
// the module's 200 and not merely "some margin".
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
    const got = load().anchorIntoView(row.target, VIEW, row.extent || EXT, row.inset);
    assert.deepEqual(got, row.want);
  });
}

test("anchorIntoView mutates neither argument", () => {
  const target = { left: 600, top: 400, width: 200, height: 100 };
  const viewport = { width: 1000, height: 700 };
  const before = structuredClone(target);
  const viewportBefore = structuredClone(viewport);
  load().anchorIntoView(target, viewport, EXT);
  assert.deepEqual(target, before);
  assert.deepEqual(viewport, viewportBefore);
});

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

// The same pair stacked VERTICALLY. Without this the whole table discriminates
// on X alone, and an implementation half-open on X but CLOSED on Y
// (`c.y <= top + height`) passes every row above while two fences drawn one
// under the other both claim a centre on their shared horizontal edge.
const TB = [
  { id: "t", rect: { left: 0, top: 0, width: 100, height: 100 } },
  { id: "u", rect: { left: 0, top: 100, width: 100, height: 100 } },
];

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

// ---- arrange moves into the fence (issue #342) --------------------------------
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

for (const row of TILES) {
  test(`tileIntoRect: ${row.name}`, () => {
    assert.deepEqual(load().tileIntoRect(row.rect, members(row.n)), row.want);
  });
}

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

// The repos a fence's members belong to, for the fence's own chrome. Sorted
// because DOM order is not stable, deduped because two consoles on one repo
// read as one place.
const REPOS = [
  { name: "no members read as no repos", members: [], want: "" },
  {
    // NEGATIVE CONTROL for the dedupe: a plain join answers "alpha · alpha".
    name: "two members on one repo read as that one repo",
    members: [{ repo: "alpha" }, { repo: "alpha" }],
    want: "alpha",
  },
  {
    // NEGATIVE CONTROL for the sort: a DOM-ordered join answers "beta · alpha".
    name: "two repos read alphabetically, whatever order the members arrive in",
    members: [{ repo: "beta" }, { repo: "alpha" }],
    want: "alpha · beta",
  },
  {
    // NEGATIVE CONTROL: `"~"` is the desk's spelling of "no repo" — the storage
    // token must not leak, the same rule `list()` applies.
    name: "a member with no repo reads as home",
    members: [{ repo: "~" }],
    want: "home",
  },
  {
    name: "home sorts among the named repos, deduped like any other",
    members: [{ repo: "zeta" }, { repo: "~" }, { repo: "~" }],
    want: "home · zeta",
  },
];

for (const row of REPOS) {
  test(`fenceRepos: ${row.name}`, () => {
    assert.equal(load().fenceRepos(row.members), row.want);
  });
}

// ---- the fence list is the map (issue #343) ---------------------------------
// One fold feeds the fence's own chrome AND the toolbar list, so the two can
// never disagree. One entry per fence, IN ORDER, whether or not it holds
// anything.
const SUM_A = { left: 0, top: 0, width: 200, height: 200 };
const box = (x, y) => ({ left: x - 10, top: y - 10, width: 20, height: 20 });

const SUMMARIES = [
  {
    name: "two members read their count and their repos, sorted and home-renamed",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "~", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, repos: "a · home" }],
  },
  {
    // Dedup is a REPOS rule, not a count rule: two consoles on one repo read as
    // one place but still as two consoles.
    name: "two members on one repo keep a count of two",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "a", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, repos: "a" }],
  },
  {
    name: "an empty fence is still listed, at zero",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [],
    want: [{ id: "a", name: "alpha", count: 0, repos: "" }],
  },
  {
    // NEGATIVE CONTROL: the centre sits exactly on `left + width`. A CLOSED
    // containment counts it as a member and reds this row.
    name: "a member whose centre is on the far edge belongs to no fence",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [{ id: "w1", repo: "a", rect: box(200, 100) }],
    want: [{ id: "a", name: "alpha", count: 0, repos: "" }],
  },
  {
    // NEGATIVE CONTROL for "exactly one fence": hand-overlapped rects (which a
    // hand-edited desk.toml can carry) must not double-count the shared member.
    name: "two overlapping fences split a shared member into the FIRST one only",
    fences: [
      { id: "a", name: "alpha", rect: SUM_A },
      { id: "b", name: "beta", rect: { left: 100, top: 100, width: 200, height: 200 } },
    ],
    windows: [{ id: "w1", repo: "a", rect: box(150, 150) }],
    want: [
      { id: "a", name: "alpha", count: 1, repos: "a" },
      { id: "b", name: "beta", count: 0, repos: "" },
    ],
  },
];

for (const row of SUMMARIES) {
  test(`fenceSummaries: ${row.name}`, () => {
    const got = load().fenceSummaries(row.fences, row.windows);
    assert.deepEqual(got, row.want);
    // Asserted as a RELATION on every row, not only the overlap one: no window
    // may be counted twice, whatever the rects.
    assert.ok(
      got.reduce((n, s) => n + s.count, 0) <= row.windows.length,
      "a window is a member of at most ONE fence",
    );
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
    assert.deepEqual(load().spawnRectIn(row.fence, row.index, HEAD), row.want);
  });
}

test("spawnRectIn: the box lies inside the fence at every cascade slot", () => {
  const wb = load();
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

// ---- walking the fences from the keyboard -----------------------------------
// Alt+Shift+←/→ steps through the fences in the plane's own READING ORDER — top
// band first, left to right inside it — not in the order the desk array happens
// to carry them. Creation order on a plane means the walk teleports across the
// stage; reading order makes the shortcut a sweep.
//
// A 120 px band is what keeps a row a row: two fences placed side by side are
// never pixel-aligned on `top`, and a raw `top` sort would zig-zag between them.
const at = (id, left, top) => ({ id, rect: { left, top, width: 400, height: 300 } });

// Deliberately shuffled against reading order: `c` is first in the array and
// last on the plane, so every row below is red under a fold that walks the
// array.
const GRID = [
  at("c", 900, 700), //  bottom row, right
  at("b", 900, 40), //   top row, right
  at("d", 60, 700), //   bottom row, left
  at("a", 60, 40), //    top row, left
];

const CYCLE = [
  { name: "no fences at all: nothing to walk", fences: [], from: null, step: 1, want: null },
  {
    name: "forward with nothing in hand enters at the top-left fence",
    from: null,
    step: 1,
    want: "a",
  },
  {
    // The other end, so "enters at an end" cannot pass by answering `order[0]`
    // for both directions.
    name: "backward with nothing in hand enters at the bottom-right fence",
    from: null,
    step: -1,
    want: "c",
  },
  { name: "forward walks left to right inside the top band", from: "a", step: 1, want: "b" },
  {
    // The band's own boundary: the walk leaves the top row only after both of
    // its fences, which is what a raw `top` sort would get wrong.
    name: "forward crosses to the next band after the last fence of this one",
    from: "b",
    step: 1,
    want: "d",
  },
  { name: "forward walks left to right inside the bottom band too", from: "d", step: 1, want: "c" },
  { name: "forward wraps from the last fence to the first", from: "c", step: 1, want: "a" },
  { name: "backward wraps from the first fence to the last", from: "a", step: -1, want: "c" },
  { name: "backward retraces the same order", from: "d", step: -1, want: "b" },
  {
    // A focus can outlive the fence that carried it (another client removed it
    // between the jump and the key): entering from an unknown id is the
    // no-fence-in-hand case, not a crash and not a stall.
    name: "an id no fence carries re-enters at the end the step names",
    from: "gone",
    step: -1,
    want: "c",
  },
  {
    name: "one fence: the walk is a no-op that still answers that fence",
    fences: [at("solo", 40, 40)],
    from: "solo",
    step: 1,
    want: "solo",
  },
];

for (const row of CYCLE) {
  test(`fenceCycle: ${row.name}`, () => {
    const got = load().fenceCycle(row.fences || GRID, row.from, row.step);
    assert.equal(got, row.want);
  });
}

test("fenceCycle breaks a tie on id, so every client walks the same order", () => {
  const same = [at("z", 100, 100), at("a", 100, 100)];
  assert.equal(load().fenceCycle(same, null, 1), "a");
  assert.equal(load().fenceCycle(same, "a", 1), "z");
});

test("fenceCycle mutates neither the list nor its rects", () => {
  const fences = structuredClone(GRID);
  const before = structuredClone(GRID);
  load().fenceCycle(fences, "a", 1);
  assert.deepEqual(fences, before);
});

// ---- detachFold: the detach registry's transitions ---------------------------
// The fold is the whole decision surface for detaching a fence into its own
// window — every case below is a RELATION (what the registry becomes, which
// effect comes back), never a count of popups, because the caller is what turns
// an effect into a `window.open`.

test("detachFold: the cap is four — the fifth detach is refused, not opened", () => {
  const WB = load();
  assert.equal(WB.DETACH_MAX, 4);
  let reg = [];
  const opened = [];
  for (const id of ["f-a", "f-b", "f-c", "f-d"]) {
    const out = WB.detachFold(reg, { type: "detach", fenceId: id });
    opened.push(...out.effects.filter((e) => e.type === "open").map((e) => e.fenceId));
    reg = out.registry;
  }
  assert.deepStrictEqual(opened, ["f-a", "f-b", "f-c", "f-d"]);
  assert.equal(reg.length, 4);

  // THE NEGATIVE CONTROL. With the cap check deleted this line goes green with
  // an `open` and a 5-long registry, so both halves are asserted: the effect is
  // the literal `refuse`, AND the registry came back untouched.
  const fifth = WB.detachFold(reg, { type: "detach", fenceId: "f-e" });
  assert.deepStrictEqual(fifth.effects, [{ type: "refuse", fenceId: "f-e", reason: "cap" }]);
  assert.equal(fifth.registry.length, 4);
  assert.deepStrictEqual(fifth.registry, reg);
});

test("detachFold: one popup per fence — detaching a detached fence focuses it", () => {
  const WB = load();
  const first = WB.detachFold([], { type: "detach", fenceId: "f-a" });
  assert.deepStrictEqual(first.effects, [{ type: "open", fenceId: "f-a" }]);
  assert.equal(first.registry.length, 1);

  const again = WB.detachFold(first.registry, { type: "detach", fenceId: "f-a" });
  assert.equal(again.registry.length, 1, "no second entry for the same fence");
  assert.equal(again.effects.length, 1);
  assert.equal(again.effects[0].type, "focus");
  assert.equal(again.effects[0].fenceId, "f-a");
});

test("detachFold: re-attach empties the registry, and a SECOND one is a no-op", () => {
  const WB = load();
  const held = WB.detachFold([], { type: "detach", fenceId: "f-a" }).registry;

  const home = WB.detachFold(held, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(home.effects, [{ type: "close", fenceId: "f-a" }]);
  assert.deepStrictEqual(home.registry, []);

  // Both the popup's `beforeunload` AND the opener's `closed` poll report the
  // re-attach: the doubled signal must not spawn the consoles twice.
  const twice = WB.detachFold(home.registry, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(twice.effects, []);
  assert.deepStrictEqual(twice.registry, []);
});

test("detachFold: re-attaching a fence that was never detached changes nothing", () => {
  const WB = load();
  const out = WB.detachFold(["f-a"], { type: "reattach", fenceId: "f-zzz" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

test("detachFold: focus reaches a detached fence only", () => {
  const WB = load();
  const member = WB.detachFold(["f-a"], { type: "focus", fenceId: "f-a" });
  assert.deepStrictEqual(member.effects, [{ type: "focus", fenceId: "f-a" }]);
  assert.deepStrictEqual(member.registry, ["f-a"]);

  const stranger = WB.detachFold(["f-a"], { type: "focus", fenceId: "f-b" });
  assert.deepStrictEqual(stranger.effects, [], "nothing to focus for an attached fence");
  assert.deepStrictEqual(stranger.registry, ["f-a"]);
});

test("detachFold: the input registry is never mutated, whatever the event", () => {
  const WB = load();
  for (const event of [
    { type: "detach", fenceId: "f-new" },
    { type: "detach", fenceId: "f-a" },
    { type: "reattach", fenceId: "f-a" },
    { type: "reattach", fenceId: "f-nope" },
    { type: "focus", fenceId: "f-a" },
    { type: "wat", fenceId: "f-a" },
  ]) {
    const reg = ["f-a", "f-b"];
    const before = [...reg];
    const out = WB.detachFold(reg, event);
    assert.deepStrictEqual(reg, before, `mutated on ${event.type}`);
    assert.notEqual(out.registry, reg, "a NEW array comes back, never the input");
  }
});

test("detachFold: an unknown event is inert, so a stray message cannot detach", () => {
  const WB = load();
  const out = WB.detachFold(["f-a"], { type: "heartbeat", fenceId: "f-a" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

// ---- peerFold: the heartbeat and the peer-lost transition (issue #347) -------
// The window is passed in, so these run in microseconds rather than six real
// seconds — which is the whole reason the rule is a pure fold and not a timer.
const WINDOW_MS = 6000;
const SEEN = 100000;

test("peerFold: silence past the window loses the peer, exactly once", () => {
  const WB = load();
  const lost = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS + 1 }, WINDOW_MS);
  assert.deepStrictEqual(lost.effects, [{ type: "peer-lost" }]);
  assert.equal(lost.state.lost, true);
  // The effect turns into a `window.close()`, so a SECOND tick must be silent.
  const again = WB.peerFold(lost.state, { type: "tick", at: SEEN + WINDOW_MS + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(again.effects, []);
  assert.equal(again.state.lost, true);
});

// THE NEGATIVE CONTROL for the whole rule: inverting the comparison, or dropping
// the beat's `seen` update, makes this row red while the one above stays green.
test("peerFold: a beat inside the window does not expire — an F5 costs no popup", () => {
  const WB = load();
  const beaten = WB.peerFold({ seen: SEEN, lost: false }, { type: "beat", at: SEEN + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(beaten.effects, []);
  assert.equal(beaten.state.seen, SEEN + 5000);
  const tick = WB.peerFold(beaten.state, { type: "tick", at: SEEN + 6001 }, WINDOW_MS);
  assert.deepStrictEqual(tick.effects, [], "1001 ms of silence is well inside a 6000 ms window");
  assert.equal(tick.state.lost, false);
});

test("peerFold: the boundary is strict — a tick exactly at the window is still alive", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: a peer never heard from does not expire — the boot-adoption grace", () => {
  const WB = load();
  const out = WB.peerFold({ seen: null, lost: false }, { type: "tick", at: SEEN + 999999 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: loss is terminal — a beat after the loss does not resurrect the peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: true }, { type: "beat", at: SEEN + 1 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, true);
  assert.equal(out.state.seen, SEEN, "a lost peer's clock stops with it");
});

test("peerFold: an announced departure loses the peer without waiting the window out", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "gone" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, [{ type: "peer-lost" }]);
  assert.equal(out.state.lost, true);
});

test("peerFold: an unknown event is inert, so a stray channel message cannot lose a peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "origin-ping" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.state, { seen: SEEN, lost: false });
  const undef = WB.peerFold(undefined, { type: "tick", at: SEEN }, WINDOW_MS);
  assert.deepStrictEqual(undef.effects, []);
  assert.deepStrictEqual(undef.state, { seen: null, lost: false });
});

test("peerFold: the input state is never mutated", () => {
  const WB = load();
  const state = { seen: SEEN, lost: false };
  for (const event of [
    { type: "beat", at: SEEN + 10 },
    { type: "tick", at: SEEN + WINDOW_MS + 1 },
    { type: "gone" },
  ]) {
    WB.peerFold(state, event, WINDOW_MS);
    assert.deepStrictEqual(state, { seen: SEEN, lost: false });
  }
});
