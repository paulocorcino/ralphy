// Unit tests for the key handler of assets/ui/wb-console-press.ts: the chords
// the page takes from a terminal. They are here, not in wb-console-tabs.test.mjs,
// so a change of the tabs and a change of this handler never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { wirePress } from "../assets/ui/wb-console-press.ts";

// The handler `wirePress` gives xterm, for a page with (or without) a shell's
// key listener.
function handlerOf({ getShell }) {
  let handler = null;
  wirePress({
    term: { attachCustomKeyEventHandler: (h) => (handler = h) },
    body: { closest: () => null, addEventListener() {} },
    window: getShell ? { getShell } : {},
    writeClipboard() {},
  });
  return handler;
}

const chord = (code, more = {}) => ({ type: "keydown", altKey: true, shiftKey: true, ctrlKey: false, metaKey: false, code, ...more });

test("Alt+Shift+R and Alt+Shift+digit go to the page only where the shell listens", () => {
  const withShell = handlerOf({ getShell: () => ({}) });
  const popup = handlerOf({});
  for (const code of ["KeyR", "Digit1", "Digit9"]) {
    assert.equal(withShell(chord(code)), false, `${code} goes to the page`);
    assert.equal(popup(chord(code)), true, `${code} stays in a terminal with no shell`);
  }
});

test("Alt+Shift+R keeps its terminal when another modifier is down or another key is hit", () => {
  const withShell = handlerOf({ getShell: () => ({}) });
  assert.equal(withShell(chord("KeyR", { ctrlKey: true })), true);
  assert.equal(withShell(chord("KeyR", { metaKey: true })), true);
  assert.equal(withShell(chord("KeyR", { shiftKey: false })), true);
  assert.equal(withShell(chord("KeyR", { altKey: false })), true);
  assert.equal(withShell(chord("KeyQ")), true);
});
