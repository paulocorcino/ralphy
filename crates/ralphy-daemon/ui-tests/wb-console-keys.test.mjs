// Unit tests for assets/ui/wb-console-keys.ts — the font size, shared by the
// key bar and the terminal. `createConsoleKeys` is driven with stubs and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createConsoleKeys } from "../assets/ui/wb-console-keys.ts";
import { FONT_DEFAULT } from "../assets/ui/wb-console-input.ts";

function keys(store, wins = new Set()) {
  return createConsoleKeys({ window: {}, document: {}, viewStore: store, wins, focusWin() {} });
}

// A window with a terminal that records its font size and its fits.
function termWindow() {
  const t = { term: { options: { fontSize: 0 } }, fits: 0, fit: { fit: () => t.fits++ } };
  return { _term: t };
}

test("setFont stores the size and refits every window that has a terminal", () => {
  const patches = [];
  const a = termWindow();
  const b = termWindow();
  const dormant = {};
  const { setFont } = keys({ read: () => ({}), patch: (p) => patches.push(p) }, new Set([a, dormant, b]));
  assert.equal(setFont(16), 16);
  assert.deepEqual(patches, [{ font: 16 }]);
  // The box does not change, so the ResizeObserver stays quiet: only `fit`
  // tells the daemon the new size.
  for (const w of [a, b]) {
    assert.equal(w._term.term.options.fontSize, 16);
    assert.equal(w._term.fits, 1);
  }
});

test("fontSize reads the stored size and falls back to the default", () => {
  assert.equal(keys({ read: () => ({ font: 18 }), patch() {} }).fontSize(), 18);
  assert.equal(keys({ read: () => ({}), patch() {} }).fontSize(), FONT_DEFAULT);
  assert.equal(keys({ read: () => null, patch() {} }).fontSize(), FONT_DEFAULT);
});
