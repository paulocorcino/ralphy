// Unit tests for assets/ui/wb-console-touch.ts — the finger drag on a
// console: an armed selection selects lines, two fingers pan the workspace.
// `wireTouch` is driven with stubs and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { wireTouch } from "../assets/ui/wb-console-touch.ts";

// A console body that keeps its listeners, a terminal of 10 rows of 20 px
// that records the lines it selects, and a workspace scrolled to (100, 50).
function touchConsole() {
  const on = {};
  const body = { addEventListener: (type, fn) => (on[type] = fn) };
  const selected = [];
  const term = {
    rows: 10,
    element: { clientHeight: 200, querySelector: () => ({ getBoundingClientRect: () => ({ top: 0 }) }) },
    buffer: { active: { viewportY: 0 } },
    selectLines: (from, to) => selected.push([from, to]),
  };
  const ws = { scrollLeft: 100, scrollTop: 50, classList: { contains: () => false } };
  const classes = { add() {}, remove() {} };
  const touch = wireTouch({
    term,
    body,
    stage: () => ({ classList: classes }),
    workspace: () => ws,
    cancelSlide() {},
    opts: {},
  });
  return { on, selected, ws, touch };
}

// A touch event with one point per `[x, y]`, that counts its prevents.
function touchEvent(...points) {
  const e = { touches: points.map(([clientX, clientY]) => ({ clientX, clientY })), timeStamp: 0, prevented: 0 };
  e.preventDefault = () => e.prevented++;
  return e;
}

test("a drag on an armed selection selects the lines from the first row to the finger's row", () => {
  const { on, selected, touch } = touchConsole();
  touch.setSelecting(true);
  on.touchstart(touchEvent([0, 45]));
  const move = touchEvent([0, 125]);
  on.touchmove(move);
  assert.deepEqual(selected, [
    [2, 2],
    [2, 6],
  ]);
  assert.equal(move.prevented, 1);
});

test("a drag upward on an armed selection selects from the finger's row down to the first row", () => {
  const { on, selected, touch } = touchConsole();
  touch.setSelecting(true);
  on.touchstart(touchEvent([0, 125]));
  on.touchmove(touchEvent([0, 45]));
  assert.deepEqual(selected.at(-1), [2, 6]);
});

test("two fingers pan the workspace by the move of the point between them", () => {
  const { on, ws } = touchConsole();
  on.touchstart(touchEvent([10, 10], [30, 30]));
  const move = touchEvent([40, 30], [60, 50]);
  on.touchmove(move);
  assert.equal(ws.scrollLeft, 70);
  assert.equal(ws.scrollTop, 30);
  assert.equal(move.prevented, 1);
});

test("a pan that drops to one finger moves nothing", () => {
  const { on, ws } = touchConsole();
  on.touchstart(touchEvent([10, 10], [30, 30]));
  const move = touchEvent([40, 30]);
  on.touchmove(move);
  assert.equal(ws.scrollLeft, 100);
  assert.equal(ws.scrollTop, 50);
  assert.equal(move.prevented, 0);
});
