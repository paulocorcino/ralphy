// Unit tests for the image pane of assets/ui/wb-file-viewer.ts: the zoom surface and its bar.
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildImage } from "../assets/ui/wb-file-viewer-read-only.ts";

// --- the image pane: the zoom surface and its bar ------------------------
// A small DOM: the markup is not parsed, so `parts` answers each selector the
// pane reads, and every part keeps its listeners.
function part(extra = {}) {
  const listeners = {};
  return {
    style: {},
    children: [],
    listeners,
    textContent: "",
    dataset: {},
    className: "",
    append(...kids) {
      this.children.push(...kids);
    },
    setAttribute() {},
    addEventListener(type, fn) {
      listeners[type] = fn;
    },
    ...extra,
  };
}

function imagePane() {
  const parts = {
    ".img-canvas": part({ naturalWidth: 400, naturalHeight: 200 }),
    ".img-meta": part(),
    ".zoom-surface": part({
      clientWidth: 200,
      clientHeight: 100,
      getBoundingClientRect: () => ({ left: 0, top: 0 }),
    }),
    ".zoom-stage": part(),
    ".viewer-body": part(),
    '[data-act="reload"]': part(),
    '[data-act="detach"]': part(),
  };
  const pane = part({ markup: "", get innerHTML() { return this.markup; }, set innerHTML(v) { this.markup = v; }, querySelector: (sel) => parts[sel] ?? null });
  const host = {
    document: { createElement: (tag) => (tag === "div" && !pane.made ? ((pane.made = true), pane) : part()) },
    viewers: part(),
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  buildImage(host, { id: "i", project: "p", path: "x.png", ftype: "image" });
  return { pane, parts };
}

test("an image opens at the fit scale, and the bar says the percent", () => {
  const { parts } = imagePane();
  parts[".img-canvas"].listeners.load();
  // 400 x 200 in a 200 x 100 surface: half its own size.
  assert.match(parts[".zoom-stage"].style.transform, /scale\(0\.5\)/u);
  assert.equal(parts[".viewer-body"].children[0].children[0].textContent, "50%");
});

test("the old Actual size toggle is gone from the image pane", () => {
  const { pane, parts } = imagePane();
  assert.equal(pane.markup.includes('data-act="zoom"'), false);
  assert.equal(parts[".viewer-body"].children.length, 1);
});

test("the browser menu on the image is prevented", () => {
  const { parts } = imagePane();
  let prevented = false;
  parts[".img-canvas"].listeners.contextmenu({
    preventDefault: () => {
      prevented = true;
    },
  });
  assert.equal(prevented, true);
});
