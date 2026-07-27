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
  // The four globals the module touches at LOAD time: `window.addEventListener`
  // (the pagehide flush), `document.readyState`/`addEventListener` (the boot
  // hooks — "loading" parks them on a no-op listener instead of running them
  // against a DOM that does not exist), `location.protocol`/`host` (WS_ORIGIN),
  // and `ResizeObserver`, still constructed at module scope until clampAll goes.
  const window = { addEventListener() {} };
  const document = { readyState: "loading", addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  class ResizeObserver {
    observe() {}
    disconnect() {}
  }
  new Function(
    "window",
    "document",
    "location",
    "ResizeObserver",
    SRC,
  )(window, document, location, ResizeObserver);
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
