// The zoom bar and its keys (wb-zoom-bar.ts, wb-commands.ts) on a fake
// controller and a small fake DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { ZOOM_STEP } from "../assets/ui/wb-diagram-zoom.ts";
import { COMMANDS, dispatchKey, listProblems } from "../assets/ui/wb-commands.ts";
import { bindZoomKeys, buildZoomBar } from "../assets/ui/wb-zoom-bar.ts";

class FakeEl {
  constructor(tag) {
    this.tag = tag;
    this.className = "";
    this.children = [];
    this.dataset = {};
    this.listeners = {};
    this.textContent = "";
  }
  append(...kids) {
    this.children.push(...kids);
  }
  setAttribute() {}
  addEventListener(type, fn) {
    this.listeners[type] = fn;
  }
}
const doc = { createElement: (tag) => new FakeEl(tag) };

// A controller that keeps a scale; Fit is 0.5 here.
function fake(start = 2) {
  const ctl = {
    scale: start,
    at: null,
    fit: () => (ctl.scale = 0.5),
    zoom: (factor, at) => ((ctl.scale *= factor), (ctl.at = at)),
    actual: (at) => ((ctl.scale = 1), (ctl.at = at)),
  };
  return { state: ctl };
}

function build(start) {
  const f = fake(start);
  const drive = {
    fit: f.state.fit,
    zoom: f.state.zoom,
    actual: f.state.actual,
    scale: () => f.state.scale,
  };
  const bar = buildZoomBar(doc, drive, { clientWidth: 1000, clientHeight: 400 });
  const button = (act) => bar.el.children.find((c) => c.dataset.zoom === act);
  const percent = () => bar.el.children[0].textContent;
  return { bar, button, percent, state: f.state };
}

test("each of the buttons sets the expected scale", () => {
  const { button, state } = build(2);
  button("in").listeners.click();
  assert.equal(state.scale, 2 * ZOOM_STEP);
  assert.deepEqual(state.at, { x: 500, y: 200 });
  button("out").listeners.click();
  assert.equal(state.scale, 2);
  button("fit").listeners.click();
  assert.equal(state.scale, 0.5);
  button("actual").listeners.click();
  assert.equal(state.scale, 1);
});

test("the percent shows the scale of the controller, rounded", () => {
  const { bar, percent, state } = build(0.456);
  assert.equal(percent(), "46%");
  state.scale = 1.5;
  bar.refresh();
  assert.equal(percent(), "150%");
  bar.run("actual");
  assert.equal(percent(), "100%");
});

function key(k) {
  const e = { key: k, code: "", altKey: false, shiftKey: false, ctrlKey: false, metaKey: false };
  e.preventDefault = () => (e.taken = true);
  return e;
}

test("the keys + - 0 are commands of the list and drive the focused pane", () => {
  assert.deepEqual(listProblems(COMMANDS), []);
  const { bar, state } = build(2);
  let focused = null;
  bindZoomKeys(() => focused);
  assert.equal(
    dispatchKey(key("+"), "page", () => false),
    false,
    "no pane has the focus",
  );
  focused = bar;
  assert.equal(
    dispatchKey(key("+"), "page", () => false),
    true,
  );
  assert.equal(state.scale, 2 * ZOOM_STEP);
  assert.equal(
    dispatchKey(key("="), "page", () => false),
    true,
  );
  assert.equal(
    dispatchKey(key("-"), "page", () => false),
    true,
  );
  assert.equal(
    dispatchKey(key("-"), "page", () => false),
    true,
  );
  assert.equal(state.scale, 2);
  assert.equal(
    dispatchKey(key("0"), "page", () => false),
    true,
  );
  assert.equal(state.scale, 0.5);
});
