// The command palette (wb-palette.ts) and the rows it reads from the list of
// commands (wb-commands.ts). The component is built with `loadComponent`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";
import { filterRows, stepActive } from "../assets/ui/wb-palette.ts";
import { bindCommand, dispatchKey, keysText, paletteRows } from "../assets/ui/wb-commands.ts";

const row = (label) => ({ id: label, which: "", label, keys: null });

test("typing keeps the rows that hold every word, in any case", () => {
  const rows = [row("Open Settings"), row("Go to the fence one"), row("Open the files search")];
  assert.deepEqual(
    filterRows(rows, "OPEN se").map((r) => r.label),
    ["Open Settings", "Open the files search"],
  );
  assert.equal(filterRows(rows, "  ").length, 3, "no word keeps every row");
});

test("Up and Down move the active row and wrap at the ends", () => {
  assert.equal(stepActive(0, 3, "ArrowDown"), 1);
  assert.equal(stepActive(2, 3, "ArrowDown"), 0);
  assert.equal(stepActive(0, 3, "ArrowUp"), 2);
  assert.equal(stepActive(1, 3, "Enter"), 1);
  assert.equal(stepActive(0, 0, "ArrowDown"), 0);
});

test("the palette lists the commands that can run now, one row per key, and not itself", () => {
  bindCommand("fence.go", {
    can: (code) => code === "F1" || code === "F2",
    run: () => true,
    label: (code) => `Go to fence ${code.slice(1)}`,
  });
  bindCommand("palette.open", { can: () => true, run: () => {} });
  bindCommand("settings.open", { can: () => true, run: () => {} });
  const rows = paletteRows();
  const fences = rows.filter((r) => r.id === "fence.go").map((r) => r.label);
  assert.deepEqual(fences, ["Go to fence 1", "Go to fence 2"]);
  assert.equal(rows.some((r) => r.id === "palette.open"), false);
  const settings = rows.find((r) => r.id === "settings.open");
  assert.deepEqual([settings?.label, settings?.keys], ["Open settings", null], "a command with no key has a row");
});

test("the keys read as on the person's system", () => {
  assert.equal(keysText({ kind: "chord", code: "Digit3" }, false), "Alt+Shift+3");
  assert.equal(keysText({ kind: "chord", code: "Digit3" }, true), "⌥⇧3");
  assert.equal(keysText({ kind: "chord", code: "ArrowLeft" }, false), "Alt+Shift+←");
  assert.equal(keysText({ kind: "chord", code: "F12" }, true), "⌥⇧F12");
  assert.equal(keysText({ kind: "modShift", key: "f" }, false), "Ctrl+Shift+F");
  assert.equal(keysText({ kind: "modShift", key: "f" }, true), "⌘⇧F");
  assert.equal(keysText({ kind: "typed", key: "/" }, true), "/");
});

const altShiftP = () => ({
  key: "P",
  code: "KeyP",
  altKey: true,
  shiftKey: true,
  ctrlKey: false,
  metaKey: false,
  preventDefault() {},
});

test("Alt+Shift+P opens the palette, and Enter runs the active row after it closes", () => {
  const order = [];
  const { scope } = loadComponent("wbPalette", { magics: { $nextTick: (fn) => fn(), $refs: {} } });
  scope.init();
  bindCommand("settings.open", { can: () => true, run: () => void order.push("run") });
  assert.equal(dispatchKey(altShiftP(), "terminal", () => false), true, "the palette opens from a terminal");
  assert.equal(scope.paletteOpen, true);
  assert.equal(dispatchKey(altShiftP(), "page", () => false), false, "an open palette does not open again");
  scope.query = "settings";
  const close = scope.closePalette.bind(scope);
  scope.closePalette = () => (order.push("close"), close());
  scope.paletteKey({ key: "Enter", preventDefault() {} });
  assert.equal(scope.paletteOpen, false);
  assert.deepEqual(order, ["close", "run"]);
});
