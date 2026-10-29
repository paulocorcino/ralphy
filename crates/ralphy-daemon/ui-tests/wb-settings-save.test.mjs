// Unit tests for how the Settings dialog saves a client-scoped key. They are
// here, not in wb-settings-dialog.test.mjs, so a change of the dialog's markup
// and a change of this save never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

test("the key bar setting stores on or off, and anything else as null", async () => {
  const real = globalThis.localStorage;
  const map = new Map();
  globalThis.localStorage = {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, String(v)),
  };
  try {
    const { scope } = loadComponent("wbSettingsDialog");
    const stored = () => JSON.parse(map.get("wb.view.v1")).keys;
    for (const [value, keys] of [["on", "on"], ["off", "off"], ["unset", null], ["", null], [true, null]]) {
      await scope.saveSetting("consoles.key_bar", value);
      assert.equal(stored(), keys, JSON.stringify(value));
      assert.equal(scope.settings["consoles.key_bar"], value, "the dialog shows what was picked");
    }
  } finally {
    if (real === undefined) delete globalThis.localStorage;
    else globalThis.localStorage = real;
  }
});
