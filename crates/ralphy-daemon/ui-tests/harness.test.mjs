// The harness builds the Alpine stores as `main.ts` registers them. A store
// moved out of `shell()` is read as `this.$store.<name>` by shell code and as
// `Alpine.store("<name>")` by code outside Alpine; if the two gave different
// objects, a test would pass on one copy while the page uses the other.
import { test } from "node:test";
import assert from "node:assert/strict";
import { STORES, loadShell } from "./harness.mjs";

test("every store in STORES is one object per page, under $store and Alpine.store", () => {
  // A probe entry stands for the next store a stage adds: the harness must
  // reach a store by its entry in STORES, not by a name it knows.
  STORES.probeStore = (deps) => ({ deps });
  try {
    const page = loadShell();
    const other = loadShell();
    for (const name of Object.keys(STORES)) {
      const store = page.state.$store[name];
      assert.equal(typeof store, "object", `$store.${name} is built`);
      assert.equal(page.window.Alpine.store(name), store, `Alpine.store("${name}") is $store.${name}`);
      assert.notEqual(other.state.$store[name], store, `each page builds its own ${name}`);
    }
    assert.equal(typeof page.state.$store.probeStore.deps.messages.flash, "function", "a store gets the message door");
  } finally {
    delete STORES.probeStore;
  }
});
