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
