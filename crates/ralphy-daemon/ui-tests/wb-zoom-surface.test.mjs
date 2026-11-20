// The zoom surface (wb-zoom-surface.ts) on a bare element: only pointer
// events, a size and a stage.
import { test } from "node:test";
import assert from "node:assert/strict";
import { attachZoomSurface } from "../assets/ui/wb-zoom-surface.ts";

function bare() {
  const listeners = {};
  const el = {
    clientWidth: 1000,
    clientHeight: 500,
    addEventListener: (type, fn) => {
      listeners[type] = fn;
    },
    getBoundingClientRect: () => ({ left: 0, top: 0 }),
  };
  return { el, listeners, stage: { style: {} } };
}

test("a wheel on a bare element changes the scale at the pointer", () => {
  const { el, listeners, stage } = bare();
  const ctl = attachZoomSurface(el, { stage, size: { w: 400, h: 200 }, cap: 1 }, () => {});
  ctl.fit();
  const before = stage.style.transform;
  let prevented = false;
  listeners.wheel({
    deltaY: -100,
    deltaMode: 0,
    ctrlKey: false,
    clientX: 300,
    clientY: 100,
    preventDefault: () => {
      prevented = true;
    },
  });
  assert.ok(prevented);
  assert.notEqual(stage.style.transform, before);
  const scale = Number(/scale\(([\d.]+)\)/u.exec(stage.style.transform)[1]);
  assert.ok(scale > 1, `scale ${scale}`);
});
