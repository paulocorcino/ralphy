// The help screen (wb-help.ts): its lines, read from the list of commands,
// and the keys that open it. The component is built with `loadComponent`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { componentKeys, helpSections } from "../assets/ui/wb-help.ts";
import { bindCommand, dispatchKey } from "../assets/ui/wb-commands.ts";

const keysOf = (sections, label) =>
  sections.flatMap((s) => s.lines).find((l) => l.label.startsWith(label))?.keys;

test("a command's keys read as one line: a range, the arrows, or two ways", () => {
  const pc = helpSections(false);
  assert.equal(keysOf(pc, "Open the row with that digit"), "Alt+Shift+0 to 9");
  assert.equal(keysOf(pc, "Go to the fence"), "Alt+Shift+F1 to F12");
  assert.equal(keysOf(pc, "Walk the columns"), "Alt+Shift+← → ↑ ↓");
  assert.equal(keysOf(pc, "Show the keyboard shortcuts"), "Alt+Shift+/ or ?");
  assert.equal(keysOf(helpSections(true), "Go to the fence"), "⌥⇧F1 to F12");
  assert.equal(keysOf(pc, "Open settings"), undefined, "a command with no key has no line");
  assert.equal(pc.at(-1).title, "Inside the components");
});

const save = (mac) => componentKeys(mac).find((l) => l.label.startsWith("Save a note")).keys;
const copy = (mac) => componentKeys(mac).find((l) => l.label.startsWith("Copy")).keys;

test("the keys inside the components read as on the person's system", () => {
  assert.deepEqual([save(false), save(true)], ["Ctrl+S", "⌘S"]);
  assert.deepEqual([copy(false), copy(true)], ["Ctrl+Insert", "⌘C"]);
});

const free = () => false;
const key = (more) => ({
  key: "",
  code: "",
  altKey: false,
  shiftKey: false,
  ctrlKey: false,
  metaKey: false,
  preventDefault() {},
  ...more,
});

const question = () => key({ key: "?", shiftKey: true });
const altShiftSlash = () => key({ key: "?", code: "Slash", altKey: true, shiftKey: true });

test("? opens the help only outside a terminal; Alt+Shift+/ opens it from a terminal too", () => {
  let opened = 0;
  bindCommand("help.open", { can: () => true, run: () => void opened++ });
  assert.equal(dispatchKey(question(), "terminal", free), false, "? goes to the program");
  assert.equal(dispatchKey(question(), "page", free), true);
  assert.equal(dispatchKey(altShiftSlash(), "terminal", free), true);
  assert.equal(opened, 2);
});

test("the help opens on its key and on the account menu's row, and only once", () => {
  const listeners = {};
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const { scope } = loadComponent("wbHelp", { from: loaded });
  scope.init();
  loaded.window.dispatchEvent({ type: "workbench:help-open", detail: null });
  assert.equal(scope.helpOpen, true, "the account menu's row opens it");
  assert.equal(dispatchKey(key({ key: "?" }), "page", free), false, "an open help does not open again");
  scope.closeHelp();
  assert.equal(dispatchKey(key({ key: "?" }), "page", free), true);
  assert.equal(scope.helpOpen, true);
});
