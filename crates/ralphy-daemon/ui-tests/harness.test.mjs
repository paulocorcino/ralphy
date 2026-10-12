// The harness builds the Alpine stores as `main.ts` registers them. A store
// moved out of `shell()` is read as `this.$store.<name>` by shell code and as
// `Alpine.store("<name>")` by code outside Alpine; if the two gave different
// objects, a test would pass on one copy while the page uses the other.
import { test } from "node:test";
import assert from "node:assert/strict";
import { STORES, loadComponent, loadShell } from "./harness.mjs";

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

// A component changes a store only through the store's own methods
// (ADR-0073 D4): `loadComponent` gives it a view of each store that refuses a
// write, so a test of any component fails when its code writes a store field.
test("a component's $store refuses a field write and lets the store's own methods write", () => {
  STORES.probeStore = () => ({
    count: 0,
    rows: [{ n: 1 }],
    nested: { deep: { v: 1 } },
    get double() {
      return this.count * 2;
    },
    bump() {
      this.count += 1;
    },
    mark(row) {
      row.seen = true;
    },
  });
  try {
    const { scope, shell } = loadComponent("wbSpend");
    for (const name of Object.keys(STORES)) {
      const raw = shell.$store[name];
      const field = Object.keys(raw).find((k) => {
        const d = Object.getOwnPropertyDescriptor(raw, k);
        return "value" in d && typeof d.value !== "function";
      });
      assert.ok(field, `$store.${name} has an own field`);
      const before = raw[field];
      assert.throws(
        () => {
          scope.$store[name][field] = "written";
        },
        (e) => e instanceof TypeError && e.message.includes(`$store.${name}.${field}`),
        `a write to $store.${name}.${field} throws`,
      );
      assert.equal(raw[field], before, `$store.${name}.${field} is unchanged`);
    }

    const raw = shell.$store.probeStore;
    const probe = scope.$store.probeStore;
    probe.bump();
    assert.equal(raw.count, 1, "a method writes through this");
    assert.equal(probe.double, 2, "a getter reads the raw store");
    probe.mark(probe.rows[0]);
    assert.equal(raw.rows[0].seen, true, "a method given a view writes the raw object");
    assert.throws(() => probe.rows.push({ n: 2 }), TypeError, "push on a view array throws");
    assert.equal(raw.rows.length, 1);
    assert.throws(
      () => {
        probe.nested.deep.v = 2;
      },
      (e) => e instanceof TypeError && e.message.includes("$store.probeStore.nested.deep.v"),
    );
    assert.equal(raw.nested.deep.v, 1);
  } finally {
    delete STORES.probeStore;
  }
});
