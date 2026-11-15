// Unit tests for the key handler of assets/ui/wb-console-press.ts: the chords
// the page takes from a terminal. They are here, not in wb-console-tabs.test.mjs,
// so a change of the tabs and a change of this handler never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { wirePress } from "../assets/ui/wb-console-press.ts";
import { bindCommand } from "../assets/ui/wb-commands.ts";

// The handler `wirePress` gives xterm, on a page whose shortcuts are
// blocked or not.
function handlerOf({ blocked = false } = {}) {
  let handler = null;
  const modals = { consoleShortcutsBlocked: () => blocked };
  wirePress({
    term: { attachCustomKeyEventHandler: (h) => (handler = h) },
    body: { closest: () => null, addEventListener() {} },
    window: { Alpine: { store: () => modals } },
    writeClipboard() {},
  });
  return handler;
}

const chord = (code, more = {}) => ({
  type: "keydown",
  key: "",
  altKey: true,
  shiftKey: true,
  ctrlKey: false,
  metaKey: false,
  code,
  preventDefault() {},
  ...more,
});

// `false` is "the page took it": xterm sends nothing to the child.
test("a terminal gives a chord to the page only when its command can run now", () => {
  const runs = [];
  bindCommand("consoles.row", {
    can: (code) => code === "Digit1",
    run: (code) => void runs.push(code),
  });
  bindCommand("consoles.run", { can: () => true, run: () => void runs.push("run") });
  const handler = handlerOf();
  assert.equal(handler(chord("Digit1")), false, "a row with that digit: the page");
  assert.equal(handler(chord("Digit9")), true, "no row with that digit: the child");
  assert.equal(handler(chord("KeyR")), false);
  assert.equal(handlerOf({ blocked: true })(chord("KeyR")), true, "a blocked page leaves the key");
  assert.deepEqual(runs, ["Digit1", "run"]);
});

test("the release of a chord the page took stays out of xterm, and the release of another goes in", () => {
  bindCommand("consoles.row", { can: (code) => code === "Digit1", run: () => {} });
  const handler = handlerOf();
  handler(chord("Digit1"));
  assert.equal(handler(chord("Digit1", { type: "keyup" })), false);
  handler(chord("Digit9"));
  assert.equal(handler(chord("Digit9", { type: "keyup" })), true);
});

test("Alt+Shift+F<n> goes to the fence from inside a terminal", () => {
  const runs = [];
  bindCommand("fence.go", { can: () => true, run: (code) => (runs.push(code), code === "F1") });
  const handler = handlerOf();
  assert.equal(handler(chord("F1")), false, "a fence at 1: the page");
  assert.equal(handler(chord("F5")), true, "no fence at 5: the child");
  assert.deepEqual(runs, ["F1", "F5"]);
});

test("Alt+Shift+R keeps its terminal when another modifier is down or another key is hit", () => {
  bindCommand("consoles.run", { can: () => true, run: () => {} });
  const withShell = handlerOf();
  assert.equal(withShell(chord("KeyR", { ctrlKey: true })), true);
  assert.equal(withShell(chord("KeyR", { metaKey: true })), true);
  assert.equal(withShell(chord("KeyR", { shiftKey: false })), true);
  assert.equal(withShell(chord("KeyR", { altKey: false })), true);
  assert.equal(withShell(chord("KeyQ")), true);
});
