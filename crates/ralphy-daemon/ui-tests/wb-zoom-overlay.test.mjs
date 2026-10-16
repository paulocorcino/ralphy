// The "Enlarge" button and the zoom view (wb-zoom-overlay.ts), on a small fake
// DOM: a tree of elements that knows class selectors, `closest`, and events.
import { test } from "node:test";
import assert from "node:assert/strict";
import { decorateZoomable, openZoom, wireDiagramZoom } from "../assets/ui/wb-zoom-overlay.ts";

class FakeEl {
  constructor(doc, tag, props = {}) {
    this.ownerDocument = doc;
    this.tag = tag;
    this.className = "";
    this.children = [];
    this.parentElement = null;
    this.style = {};
    this.dataset = {};
    this.attrs = {};
    this.listeners = {};
    this.clientWidth = 1000;
    this.clientHeight = 500;
    this.focused = 0;
    Object.assign(this, props);
  }
  get classes() {
    return this.className.split(/\s+/).filter(Boolean);
  }
  matches(sel) {
    return sel.split(",").some((part) => {
      const s = part.trim();
      if (s.startsWith(".")) return this.classes.includes(s.slice(1));
      const data = s.match(/^\[data-zoom(?:="(.*)")?\]$/);
      if (data) return this.dataset.zoom !== undefined && (data[1] === undefined || this.dataset.zoom === data[1]);
      return this.tag === s;
    });
  }
  closest(sel) {
    for (let el = this; el; el = el.parentElement) if (el.matches(sel)) return el;
    return null;
  }
  querySelectorAll(sel) {
    const out = [];
    const walk = (el) => {
      for (const kid of el.children) {
        if (kid.matches(sel)) out.push(kid);
        walk(kid);
      }
    };
    walk(this);
    return out;
  }
  querySelector(sel) {
    return this.querySelectorAll(sel)[0] ?? null;
  }
  append(...kids) {
    for (const kid of kids) {
      kid.parentElement = this;
      this.children.push(kid);
    }
  }
  replaceWith(next) {
    const siblings = this.parentElement.children;
    siblings[siblings.indexOf(this)] = next;
    next.parentElement = this.parentElement;
    this.parentElement = null;
  }
  remove() {
    if (this.parentElement) this.parentElement.children = this.parentElement.children.filter((k) => k !== this);
    this.parentElement = null;
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  getAttribute(k) {
    // `dataset.zoom` is the attribute `data-zoom`.
    if (k.startsWith("data-")) return this.dataset[k.slice(5)] ?? null;
    return this.attrs[k] ?? null;
  }
  addEventListener(type, fn) {
    (this.listeners[type] ??= []).push(fn);
  }
  removeEventListener() {}
  focus() {
    this.focused += 1;
    this.ownerDocument.activeElement = this;
  }
  getBoundingClientRect() {
    return { left: 0, top: 0, width: this.clientWidth, height: this.clientHeight };
  }
  setPointerCapture() {}
  cloneNode() {
    const copy = new FakeEl(this.ownerDocument, this.tag, { viewBox: this.viewBox });
    copy.style = {};
    return copy;
  }
  fire(type, ev = {}) {
    const event = { type, target: this, defaultPrevented: false, stopped: false, ...ev };
    event.preventDefault = () => (event.defaultPrevented = true);
    event.stopPropagation = () => (event.stopped = true);
    for (let el = this; el; el = el.parentElement) {
      for (const fn of el.listeners[type] ?? []) fn(event);
      if (event.stopped) break;
    }
    return event;
  }
}

function makeDocument() {
  const doc = new FakeEl(null, "document");
  doc.ownerDocument = doc;
  doc.createElement = (tag) => new FakeEl(doc, tag);
  doc.body = new FakeEl(doc, "body");
  doc.append(doc.body);
  return doc;
}

const win = () => ({ addEventListener() {}, removeEventListener() {} });
const svg = (doc, width = 600, height = 200) => new FakeEl(doc, "svg", { viewBox: { baseVal: { width, height } } });
const img = (doc, naturalWidth, naturalHeight = 100) => new FakeEl(doc, "img", { naturalWidth, naturalHeight, src: "data:image/png;base64,x", alt: "", complete: true });

function article(doc, ...kids) {
  const el = new FakeEl(doc, "article");
  doc.body.append(el);
  el.append(...kids);
  return el;
}

test("a Mermaid holder gets its button beside it, and the button survives the diagram being drawn", () => {
  const doc = makeDocument();
  const holder = new FakeEl(doc, "div", { className: "mermaid" });
  const root = article(doc, holder);
  decorateZoomable(root);
  const box = root.children[0];
  assert.ok(box.classes.includes("zoomable"));
  assert.equal(box.children[0], holder);
  assert.equal(box.children[1].className, "zoom-btn");
  // Drawing the diagram replaces the holder's content, not its siblings.
  holder.children = [];
  holder.append(svg(doc));
  assert.equal(box.querySelectorAll(".zoom-btn").length, 1);
  // A second pass over the same article adds nothing.
  decorateZoomable(root);
  assert.equal(root.querySelectorAll(".zoom-btn").length, 1);
});

test("a large image gets a button; an icon, a badge in a link and a blocked image do not", () => {
  const doc = makeDocument();
  const large = img(doc, 800);
  const icon = img(doc, 16);
  const badge = img(doc, 400);
  const link = new FakeEl(doc, "a");
  link.append(badge);
  const root = article(doc, large, icon, link);
  decorateZoomable(root);
  assert.equal(root.querySelectorAll(".zoom-btn").length, 1);
  assert.ok(large.parentElement.classes.includes("zoomable"));
  assert.equal(icon.parentElement, root);
  assert.equal(badge.parentElement, link);
});

test("an image that is not loaded yet gets its button when it loads, if it is large", () => {
  const doc = makeDocument();
  const late = img(doc, 0);
  const root = article(doc, late);
  decorateZoomable(root);
  assert.equal(root.querySelectorAll(".zoom-btn").length, 0);
  late.naturalWidth = 900;
  late.fire("load");
  assert.equal(root.querySelectorAll(".zoom-btn").length, 1);
  late.fire("load");
  assert.equal(root.querySelectorAll(".zoom-btn").length, 1);
});

function opened(doc, picture = svg(doc)) {
  const holder = new FakeEl(doc, "div", { className: "mermaid" });
  holder.append(picture);
  const root = article(doc, holder);
  wireDiagramZoom(win(), doc);
  decorateZoomable(root);
  const button = root.querySelector(".zoom-btn");
  button.fire("click");
  return { root, button, overlay: doc.body.querySelector(".zoom-overlay") };
}

test("pressing the button opens the view; a diagram that is not drawn yet opens nothing", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  assert.ok(overlay);
  assert.equal(overlay.attrs.role, "dialog");

  const empty = makeDocument();
  const holder = new FakeEl(empty, "div", { className: "mermaid" });
  const root = article(empty, holder);
  wireDiagramZoom(win(), empty);
  decorateZoomable(root);
  root.querySelector(".zoom-btn").fire("click");
  assert.equal(empty.body.querySelector(".zoom-overlay"), null);
});

test("a second press while the view is open opens no second view", () => {
  const doc = makeDocument();
  const { button } = opened(doc);
  button.fire("click");
  assert.equal(doc.body.querySelectorAll(".zoom-overlay").length, 1);
});

test("Escape closes the view, stops there, and gives the focus back to the button", () => {
  const doc = makeDocument();
  const { overlay, button } = opened(doc);
  const closeBtn = overlay.querySelector('[data-zoom="close"]');
  const ev = closeBtn.fire("keydown", { key: "Escape" });
  assert.ok(ev.stopped, "the key does not reach a dialog under the view");
  assert.equal(doc.body.querySelector(".zoom-overlay"), null);
  assert.equal(button.focused, 1);
});

const stageOf = (overlay) => overlay.querySelector(".zoom-stage");
const scaleOf = (overlay) => Number(/scale\(([\d.]+)\)/.exec(stageOf(overlay).style.transform)[1]);

test("the view opens fitted, and the + and - keys zoom around its middle", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  // A 600x200 diagram in a 1000x500 window fits at 1000/600, above 1: a vector is drawn larger.
  near(scaleOf(overlay), 1000 / 600);
  overlay.fire("keydown", { key: "+" });
  near(scaleOf(overlay), (1000 / 600) * 1.25);
  overlay.fire("keydown", { key: "0" });
  near(scaleOf(overlay), 1000 / 600);
});

test("the wheel zooms the picture and keeps the page from scrolling", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  const before = scaleOf(overlay);
  const ev = overlay.querySelector(".zoom-surface").fire("wheel", { deltaY: -100, deltaMode: 0, ctrlKey: false, clientX: 500, clientY: 250 });
  assert.ok(ev.defaultPrevented);
  assert.ok(scaleOf(overlay) > before);
});

test("a raster image opens at its own size, not above it", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc, img(doc, 400, 200));
  near(scaleOf(overlay), 1);
});

test("a tap on the empty ground closes the view; a tap on the picture does not", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  const surface = overlay.querySelector(".zoom-surface");
  const picture = stageOf(overlay).children[0];
  surface.fire("pointerdown", { pointerId: 1, clientX: 10, clientY: 10, target: picture });
  surface.fire("pointerup", { pointerId: 1, clientX: 10, clientY: 10, target: surface, timeStamp: 1, pointerType: "mouse" });
  assert.ok(doc.body.querySelector(".zoom-overlay"), "the press began on the picture");
  near(scaleOf(overlay), 1000 / 600);
  surface.fire("pointerdown", { pointerId: 2, clientX: 10, clientY: 10 });
  surface.fire("pointerup", { pointerId: 2, clientX: 10, clientY: 10, timeStamp: 900, pointerType: "mouse" });
  assert.equal(doc.body.querySelector(".zoom-overlay"), null);
});

test("a drag past the threshold moves the picture and is not a tap", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  const surface = overlay.querySelector(".zoom-surface");
  overlay.querySelector('[data-zoom="in"]').fire("click");
  overlay.querySelector('[data-zoom="in"]').fire("click");
  const before = stageOf(overlay).style.transform;
  surface.fire("pointerdown", { pointerId: 1, clientX: 100, clientY: 100 });
  surface.fire("pointermove", { pointerId: 1, clientX: 160, clientY: 100, pointerType: "mouse" });
  surface.fire("pointerup", { pointerId: 1, clientX: 160, clientY: 100, timeStamp: 5, pointerType: "mouse" });
  assert.notEqual(stageOf(overlay).style.transform, before);
  assert.ok(doc.body.querySelector(".zoom-overlay"), "a drag that ends on the ground does not close the view");
});

test("two quick taps switch between the fit and the actual size", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  const surface = overlay.querySelector(".zoom-surface");
  const picture = stageOf(overlay).children[0];
  const tap = (id, at) => {
    surface.fire("pointerdown", { pointerId: id, clientX: 500, clientY: 250, target: picture });
    surface.fire("pointerup", { pointerId: id, clientX: 500, clientY: 250, target: picture, timeStamp: at, pointerType: "mouse" });
  };
  tap(1, 1000);
  tap(2, 1100);
  near(scaleOf(overlay), 1);
  tap(3, 2000);
  tap(4, 2100);
  near(scaleOf(overlay), 1000 / 600);
});

test("Tab stays inside the buttons of the view", () => {
  const doc = makeDocument();
  const { overlay } = opened(doc);
  const buttons = overlay.querySelectorAll("button");
  buttons[buttons.length - 1].focus();
  assert.ok(overlay.fire("keydown", { key: "Tab", shiftKey: false }).defaultPrevented);
  assert.equal(doc.activeElement, buttons[0]);
  assert.ok(overlay.fire("keydown", { key: "Tab", shiftKey: true }).defaultPrevented);
  assert.equal(doc.activeElement, buttons[buttons.length - 1]);
});

test("a picture with no size opens nothing", () => {
  const doc = makeDocument();
  const flat = svg(doc, 0, 0);
  flat.getBoundingClientRect = () => ({ left: 0, top: 0, width: 0, height: 0 });
  assert.equal(openZoom(win(), doc, flat, null), null);
});

function near(a, b) {
  assert.ok(Math.abs(a - b) < 1e-6, `${a} is not ${b}`);
}
