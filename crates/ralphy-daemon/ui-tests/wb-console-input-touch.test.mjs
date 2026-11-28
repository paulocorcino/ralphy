// Unit tests for assets/ui/wb-console-input.ts: touch, pointer, font and
// engine. Pure functions, called directly, with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  DOUBLE_TAP_MS,
  FONT_DEFAULT,
  FONT_MAX,
  FONT_MIN,
  PHONE_MAX_WIDTH,
  dragBegins,
  dragThreshold,
  flingStep,
  fullscreenOffered,
  isDoubleTap,
  isWebKit,
  phoneBleed,
  prefersDomRenderer,
  selectionRow,
  stepFont,
  touchCentroid,
  touchGesture,
  touchScrollLines,
  touchScrollTarget,
} from "../assets/ui/wb-console-input.ts";

const UI = join(import.meta.dirname, "../assets/ui");

// --- phoneBleed: when a maximized console folds the chrome away ------------

// pin: the phone width of the script is the one of the CSS —
// node applies no media query, so only the CSS text shows its width.
test("phoneBleed is maximize AND a phone-width viewport", () => {
  // The breakpoint is the workbench's phone breakpoint: 01-base.css gates the
  // phone layout on the same number, so a drift between the two fails here.
  const css = readFileSync(join(UI, "styles/01-base.css"), "utf8");
  const phone = css.match(/@media \(max-width: (\d+)px\) \{/u);
  assert.ok(phone, "01-base.css has a phone-width @media block");
  assert.equal(PHONE_MAX_WIDTH, Number(phone[1]));
  assert.equal(phoneBleed(true, 390), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH + 1), false);
  assert.equal(phoneBleed(true, 1280), false);
  // Not maximized: nothing folds, whatever the width.
  assert.equal(phoneBleed(false, 390), false);
  // A viewport that has never laid out reports NaN — no bleed, no throw.
  assert.equal(phoneBleed(true, NaN), false);
  assert.equal(phoneBleed(true), false);
});

// --- selectionRow: the buffer line under a finger --------------------------

test("selectionRow maps a touch to a buffer-absolute row", () => {
  // Screen at y=100, 20px rows, 24 rows on screen, scrolled 50 lines in.
  assert.equal(selectionRow(100, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(119, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(120, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 51);
  assert.equal(selectionRow(345, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 62);
  // Unscrolled: the row IS the buffer line.
  assert.equal(selectionRow(140, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 0 }), 2);
});

test("selectionRow clamps to the screen", () => {
  // Above the screen: the top row. Below it: the last row, never a line that
  // is not on screen.
  assert.equal(selectionRow(0, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(9999, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 73);
  assert.equal(selectionRow(9999, { screenTop: 100, cellHeight: 20, rows: 1, viewportY: 0 }), 0);
});

test("selectionRow never answers NaN", () => {
  // A terminal that has not laid out has a zero cell height; a missing
  // viewportY is 0. Either way the answer is a row `selectLines` accepts.
  assert.equal(selectionRow(140, { screenTop: 100, cellHeight: 0, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(140, { screenTop: 100, cellHeight: NaN, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(NaN, { screenTop: 100, cellHeight: 20, rows: 24, viewportY: 50 }), 50);
  assert.equal(selectionRow(140, { screenTop: 100, cellHeight: 20, rows: 24 }), 2);
  assert.equal(selectionRow(140, { screenTop: undefined, cellHeight: 20, rows: 24, viewportY: 0 }), 7);
});

// --- stepFont: the A− / A+ range ------------------------------------------

test("stepFont walks one px at a time and stops at both ends", () => {
  assert.equal(stepFont(15, 1), 16);
  assert.equal(stepFont(15, -1), 14);
  assert.equal(stepFont(FONT_MAX, 1), FONT_MAX);
  assert.equal(stepFont(FONT_MIN, -1), FONT_MIN);
  // Past the ends from outside the range — a store hand-edited before the
  // normalisation in wb-client-view.ts was added.
  assert.equal(stepFont(400, 1), FONT_MAX);
  assert.equal(stepFont(1, -1), FONT_MIN);
});

test("stepFont starts from xterm's own default when nothing is stored", () => {
  // `fontSize()` answers null-ish when the profile has no preference; stepping
  // from there must land next to the size the operator is actually looking at.
  assert.equal(stepFont(null, 1), FONT_DEFAULT + 1);
  assert.equal(stepFont(undefined, -1), FONT_DEFAULT - 1);
  assert.equal(stepFont(NaN, 1), FONT_DEFAULT + 1);
  // Always an integer: a half-px size is a blurred glyph grid.
  assert.equal(Number.isInteger(stepFont(15.4, 1)), true);
});

// --- touchGesture / touchCentroid: how many fingers, whose gesture ---------
test("touchGesture gives one finger to the terminal, two to the canvas, the rest to the system", () => {
  // [case, finger count, maxlock, expected owner]
  const rows = [
    ["one finger", 1, false, "terminal"],
    ["two fingers", 2, false, "canvas"],
    // Under maxlock one finger stays the terminal's, and two go to nobody.
    ["one finger under maxlock", 1, true, "terminal"],
    ["two fingers under maxlock", 2, true, "none"],
    ["three fingers", 3, false, "none"],
    ["no fingers", 0, false, "none"],
    ["an unknown count", undefined, false, "none"],
  ];
  for (const [name, fingers, maxlock, want] of rows) {
    assert.equal(touchGesture(fingers, maxlock), want, name);
  }
});

test("touchCentroid is the point between the fingers", () => {
  assert.deepEqual(touchCentroid([{ clientX: 10, clientY: 20 }, { clientX: 30, clientY: 60 }]), { x: 20, y: 40 });
  assert.deepEqual(touchCentroid([{ clientX: 5, clientY: 5 }]), { x: 5, y: 5 });
  assert.deepEqual(touchCentroid([]), { x: 0, y: 0 });
  assert.deepEqual(touchCentroid(), { x: 0, y: 0 });
});

// --- dragThreshold / dragBegins: a tap is not a drag ------------------------
test("dragThreshold is 4px for a mouse and 10px for a finger, a pen or an unknown pointer", () => {
  assert.equal(dragThreshold("mouse"), 4);
  assert.equal(dragThreshold("touch"), 10);
  assert.equal(dragThreshold("pen"), 10);
  assert.equal(dragThreshold(), 10);
  assert.equal(dragThreshold(""), 10);
});

const BEGINS = [
  // [start, pointer, threshold, expected, why]
  [{ x: 0, y: 0 }, { x: 0, y: 0 }, 4, false, "no travel is a tap"],
  [{ x: 100, y: 100 }, { x: 103.99, y: 100 }, 4, false, "just under the mouse threshold"],
  [{ x: 100, y: 100 }, { x: 104, y: 100 }, 4, true, "exactly the mouse threshold arms"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 4, true, "the diagonal counts as 5, not 3 or 4"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 10, false, "5px is a finger's slip"],
  [{ x: 0, y: 0 }, { x: -6, y: 8 }, 10, true, "direction does not matter"],
  [{ x: 50, y: 50 }, { x: 50, y: 41 }, 10, false, "9px straight up is still a tap for a finger"],
];

for (const [start, pointer, threshold, want, why] of BEGINS) {
  test(`dragBegins: ${why}`, () => {
    assert.equal(dragBegins(start, pointer, threshold), want);
  });
}

// --- isDoubleTap: two taps on a titlebar ----------------------------------
const TAPS = [
  // [prev, tap, expected, why]
  [null, { t: 100, x: 0, y: 0 }, false, "a first tap is not a double tap"],
  [{ t: 0, x: 50, y: 50 }, { t: 200, x: 52, y: 49 }, true, "a second tap close in time and place"],
  [{ t: 0, x: 50, y: 50 }, { t: 300, x: 50, y: 50 }, true, "exactly the time limit still counts"],
  [{ t: 0, x: 50, y: 50 }, { t: 301, x: 50, y: 50 }, false, "one ms past the time limit is a new first tap"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 6, y: 8 }, false, "10px away is another place"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 5, y: 8 }, true, "just under 10px is the same place"],
  [{ t: 500, x: 0, y: 0 }, { t: 400, x: 0, y: 0 }, false, "a tap from the future is not a pair"],
];

for (const [prev, tap, want, why] of TAPS) {
  test(`isDoubleTap: ${why}`, () => {
    assert.equal(DOUBLE_TAP_MS, 300);
    assert.equal(isDoubleTap(prev, tap), want);
  });
}

// --- touchScrollTarget: whose gesture a finger's drag is -------------------
// The finger must be the trackpad, and xterm gives the trackpad's wheel to
// three different owners. Under a TUI that tracks the mouse the viewport's
// history is a heap of the app's stale frames — the "ghosts" an iPad showed.
test("touchScrollTarget moves the viewport only in the plain case", () => {
  // [case, mouse-tracking mode, buffer, expected owner]
  const rows = [
    // An app that is tracking the mouse gets the gesture, in either buffer.
    ...["x10", "vt200", "drag", "any"].flatMap((mode) => [
      [`${mode} tracking, normal buffer`, mode, "normal", "app"],
      [`${mode} tracking, alternate buffer`, mode, "alternate", "app"],
    ]),
    // The alternate buffer has no history, so the app gets it there too.
    ["no tracking, alternate buffer", "none", "alternate", "app"],
    ["no tracking, normal buffer", "none", "normal", "viewport"],
    ["an unknown mode, normal buffer", undefined, "normal", "viewport"],
    ["nothing known", null, undefined, "viewport"],
  ];
  for (const [name, mode, buffer, want] of rows) {
    assert.equal(touchScrollTarget(mode, buffer), want, name);
  }
});

// --- touchScrollLines: the gesture the console had to take back -----------
// A drag over a console used to pan the whole canvas: the touch lands on
// `.xterm-screen`, and the element that scrolls is its sibling, not its
// ancestor, so the browser walked up to `#workspace` (xterm.js #3613/#594).

test("touchScrollLines converts a drag into lines at the terminal's cell height", () => {
  // Dragging the content DOWN moves the VIEW up, hence the sign flip.
  assert.equal(touchScrollLines(-34, 17), 2);
  assert.equal(touchScrollLines(34, 17), -2);
  // Fractional on purpose: a slow drag moves less than a row per event, and
  // truncating each one on its own rounds the whole gesture away to nothing.
  assert.equal(touchScrollLines(-8.5, 17), 0.5);
});

test("touchScrollLines refuses to divide by a cell height it does not have", () => {
  // A terminal mid-teardown, or one that has never laid out, reports 0 —
  // and `-dy / 0` is Infinity, which `scrollLines` would take literally.
  assert.equal(touchScrollLines(-100, 0), 0);
  assert.equal(touchScrollLines(-100, -1), 0);
  assert.equal(touchScrollLines(-100, NaN), 0);
  assert.equal(touchScrollLines(NaN, 17), 0);
  assert.equal(touchScrollLines(undefined, 17), 0);
});

// --- flingStep: the glide that makes scrollback reachable by hand ---------

test("flingStep decays toward a stop and reports the distance for the frame", () => {
  const first = flingStep(1, 16);
  assert.equal(first.dy, 16);
  // One frame of decay, not a fixed subtraction: a longer frame decays more.
  assert.ok(first.velocity < 1 && first.velocity > 0.9);
  assert.ok(flingStep(1, 32).velocity < first.velocity);
});

test("flingStep cuts the glide once it stops being motion", () => {
  // Below the floor it is drift, not a fling — and a velocity that never
  // reaches zero is a requestAnimationFrame loop that never ends.
  assert.equal(flingStep(0.001, 16).velocity, 0);
  assert.equal(flingStep(0, 16).velocity, 0);
  // A long enough frame gap must also land on a stop rather than overshooting.
  assert.equal(flingStep(1, 100000).velocity, 0);
  // Garbage in never produces a moving glide.
  assert.deepEqual(flingStep(NaN, 16), { dy: 0, velocity: 0 });
  assert.deepEqual(flingStep(1, 0), { dy: 0, velocity: 0 });
});

test("a fling always terminates", () => {
  let v = 5;
  let frames = 0;
  while (v !== 0 && frames < 10000) {
    v = flingStep(v, 16).velocity;
    frames += 1;
  }
  assert.equal(v, 0, "the glide must reach a stop");
  assert.ok(frames < 200, `and get there quickly, not in ${frames} frames`);
});

// --- fullscreenOffered: where the fullscreen button is worth building -----
// Two independent reasons to withhold it, and the table keeps them separable:
// no API at all (sandboxed frame, standalone PWA), and an API that WebKit hands
// back the moment a text field takes focus.
test("fullscreenOffered builds the button only where the engine can hold it", () => {
  // [case, fullscreen API present, vendor, expected]
  const rows = [
    ["no API", false, "Google Inc.", false],
    ["an unknown API", undefined, "Google Inc.", false],
    ["no API and no vendor", null, "", false],
    // WebKit hands fullscreen back the moment the keyboard takes focus.
    ["WebKit", true, "Apple Computer, Inc.", false],
    ["Chromium", true, "Google Inc.", true],
    ["an empty vendor", true, "", true],
  ];
  for (const [name, api, vendor, want] of rows) {
    assert.equal(fullscreenOffered(api, vendor), want, name);
  }
});

test("isWebKit is the one engine question both decisions ask", () => {
  assert.equal(isWebKit("Apple Computer, Inc."), true);
  assert.equal(isWebKit("Google Inc."), false);
  assert.equal(isWebKit(""), false);
  assert.equal(isWebKit(), false);
});

// --- prefersDomRenderer: which engines must not get the GPU renderer ------
// The WebGL addon draws scrolled rows twice on WebKit — reported from an iPad
// as the text "distorting", and reproducible by dragging the scrollbar, a path
// this module does not touch. Upstream has carried it for years (xterm.js
// #3357, #5816) and the standing answer is to not use the addon there.

test("prefersDomRenderer asks about the ENGINE, not the brand", () => {
  // [case, vendor, expected]
  const rows = [
    // Safari, and every other browser on iPadOS — all WebKit underneath, all
    // reporting the same vendor. That is exactly why the vendor is the question.
    ["WebKit", "Apple Computer, Inc.", true],
    ["Chromium", "Google Inc.", false],
    ["Firefox, which reports an empty vendor", "", false],
    // A browser that reports nothing at all keeps the faster renderer: the DOM
    // fallback is the safe answer for a KNOWN-bad engine, not a default.
    ["no vendor", undefined, false],
    ["a null vendor", null, false],
    ["a vendor that is not a string", 42, false],
  ];
  for (const [name, vendor, want] of rows) {
    assert.equal(prefersDomRenderer(vendor), want, name);
  }
});
