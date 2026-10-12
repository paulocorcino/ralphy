// Unit tests for assets/ui/wb-stage-columns.ts — the column paint.
// `createStageColumns` is driven with fake `deps`: a viewport of a given
// width and one window with the title-bar button, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createStageColumns } from "../assets/ui/wb-stage-columns.ts";

// The title-bar button that opens the list of consoles.
function fakeButton() {
  const attrs = {};
  return {
    hidden: true,
    title: "Slice",
    innerHTML: '<i class="bi bi-arrows-expand-vertical"></i>',
    setAttribute: (k, v) => (attrs[k] = v),
    getAttribute: (k) => attrs[k] ?? null,
  };
}

// A maximized window with that button, on a viewport `width` px wide.
function paint({ width, autoBoot = true }) {
  const btn = fakeButton();
  const win = { _deskId: "a", _colBtn: btn, classList: { contains: (c) => c === "maximized" } };
  const noop = () => {};
  const cols = createStageColumns({
    OPTS: { autoBoot },
    wins: new Set([win]),
    workspace: () => ({ clientWidth: width }),
    stage: () => null,
    popups: { entries: () => [] },
    desk: () => [],
    focusWin: noop,
    setMax: noop,
    paintMaxButton: noop,
    syncMaxLock: noop,
    syncMaxPin: noop,
    applyExtent: noop,
    list: () => [],
    paintFenceColumns: noop,
    fenceList: () => [],
    readFenceRects: () => [],
    readWindowRects: () => [],
    findWindow: () => null,
    tearDownMember: noop,
  });
  cols.applyColumns([]);
  return btn;
}

// A phone paints one console, so the list there switches the maximized
// console: the button shows with its own name and icon (#599).
test("a maximized console shows the list button as Switch on a phone and as Slice wider", () => {
  const phone = paint({ width: 400 });
  assert.equal(phone.hidden, false, "the button shows on a phone");
  assert.deepEqual([phone.title, phone.getAttribute("aria-label")], ["Switch", "Switch"]);
  assert.match(phone.innerHTML, /bi-arrow-down-up/);
  const wide = paint({ width: 1200 });
  assert.equal(wide.hidden, false);
  assert.deepEqual([wide.title, wide.getAttribute("aria-label")], ["Slice", "Slice"]);
  assert.match(wide.innerHTML, /bi-arrows-expand-vertical/);
});

test("the detached-fence popup never shows the list button", () => {
  for (const width of [400, 1200]) assert.equal(paint({ width, autoBoot: false }).hidden, true, `${width}px`);
});
