// The zoom view of a diagram or an image (wb-diagram-zoom.ts): the arithmetic of
// the view, and, further down, the DOM that draws it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { clampPan, clampScale, fitScale, fitView, minScale, MAX_SCALE, panBy, pinchView, wheelFactor, zoomAt } from "../assets/ui/wb-diagram-zoom.ts";

const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-9, `${a} is not ${b}`);

test("the point under the cursor stays under it when the view zooms", () => {
  const view = { scale: 1, x: 30, y: 10 };
  const at = { x: 200, y: 120 };
  // The picture point under `at`, before and after.
  const under = (v) => ({ x: (at.x - v.x) / v.scale, y: (at.y - v.y) / v.scale });
  const zoomed = zoomAt(view, 2, at, 0.1);
  assert.equal(zoomed.scale, 2);
  near(under(zoomed).x, under(view).x);
  near(under(zoomed).y, under(view).y);
});

test("the scale stops at the largest scale and at the minimum", () => {
  assert.equal(clampScale(100, 0.5), MAX_SCALE);
  assert.equal(clampScale(0.01, 0.5), 0.5);
  assert.equal(zoomAt({ scale: 6, x: 0, y: 0 }, 2, { x: 0, y: 0 }, 0.5).scale, MAX_SCALE);
});

test("fit fills the tighter side, and a raster image is not drawn above its own pixels", () => {
  const box = { w: 1000, h: 500 };
  assert.equal(fitScale({ w: 2000, h: 500 }, box, 4), 0.5);
  assert.equal(fitScale({ w: 100, h: 100 }, box, 4), 4);
  assert.equal(fitScale({ w: 100, h: 100 }, box, 1), 1);
  assert.equal(fitScale({ w: 0, h: 0 }, box, 4), 1);
});

test("a small diagram can be shown at 100%; a large one cannot go below its fit", () => {
  assert.equal(minScale(2.3), 1);
  assert.equal(minScale(0.4), 0.4);
});

test("a picture smaller than the window is centered; a larger one keeps its edges in the window", () => {
  const box = { w: 1000, h: 500 };
  const small = clampPan({ scale: 1, x: 900, y: -90 }, { w: 400, h: 100 }, box);
  assert.deepEqual([small.x, small.y], [300, 200]);
  const large = { w: 3000, h: 2000 };
  assert.equal(clampPan({ scale: 1, x: 50, y: 50 }, large, box).x, 0);
  assert.equal(clampPan({ scale: 1, x: -9000, y: -9000 }, large, box).x, -2000);
  assert.equal(clampPan({ scale: 1, x: -9000, y: -9000 }, large, box).y, -1500);
});

test("the fit view is centered", () => {
  const view = fitView({ w: 500, h: 500 }, { w: 1000, h: 500 }, 1);
  assert.deepEqual(view, { scale: 1, x: 250, y: 0 });
});

test("two fingers scale by the change of their distance and follow their middle point", () => {
  const view = { scale: 1, x: 0, y: 0 };
  const prev = [{ x: 100, y: 100 }, { x: 200, y: 100 }];
  const spreadOnly = [{ x: 50, y: 100 }, { x: 250, y: 100 }];
  const doubled = pinchView(view, prev, spreadOnly, 0.1);
  assert.equal(doubled.scale, 2);
  // The middle point did not move, so the picture point under it did not move.
  near((150 - doubled.x) / doubled.scale, 150);
  const moved = pinchView(view, prev, [{ x: 130, y: 120 }, { x: 230, y: 120 }], 0.1);
  assert.equal(moved.scale, 1);
  assert.deepEqual([moved.x, moved.y], [30, 20]);
});

test("two fingers on the same point ask for no scale", () => {
  const same = [{ x: 5, y: 5 }, { x: 5, y: 5 }];
  assert.equal(pinchView({ scale: 3, x: 0, y: 0 }, same, [{ x: 0, y: 0 }, { x: 40, y: 0 }], 0.1).scale, 3);
});

test("a wheel step is one factor, longer for a trackpad pinch, and held to a halving or a doubling", () => {
  const down = wheelFactor({ deltaY: 100, deltaMode: 0, ctrlKey: false });
  assert.ok(down < 1 && down > 0.5);
  assert.ok(wheelFactor({ deltaY: -100, deltaMode: 0, ctrlKey: false }) > 1);
  assert.ok(wheelFactor({ deltaY: 10, deltaMode: 0, ctrlKey: true }) < wheelFactor({ deltaY: 10, deltaMode: 0, ctrlKey: false }));
  assert.equal(wheelFactor({ deltaY: -100000, deltaMode: 0, ctrlKey: false }), 2);
  // A line-mode wheel reports lines, not pixels.
  assert.ok(wheelFactor({ deltaY: 3, deltaMode: 1, ctrlKey: false }) < wheelFactor({ deltaY: 3, deltaMode: 0, ctrlKey: false }));
});

test("panBy moves the offset and keeps the scale", () => {
  assert.deepEqual(panBy({ scale: 2, x: 1, y: 2 }, 10, -5), { scale: 2, x: 11, y: -3 });
});
