// The harness builds the Alpine stores as `main.ts` registers them. A store
// moved out of `shell()` is read as `this.$store.<name>` by shell code and as
// `Alpine.store("<name>")` by code outside Alpine; if the two gave different
// objects, a test would pass on one copy while the page uses the other.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  STORES,
  fakeClassList,
  fakeWindow,
  fixture,
  load,
  loadComponent,
  loadShell,
  swapFetch,
  withoutBroadcast,
} from "./harness.mjs";

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

test("load runs a factory against a stub page at the daemon's origin", () => {
  const got = load((window, document, location, deps) => ({ window, document, location, deps }), {
    document: { readyState: "loading" },
    deps: { messages: "door" },
  });
  got.window.addEventListener("pagehide", () => {});
  got.document.addEventListener("workbench:action", () => {});
  assert.equal(got.document.readyState, "loading", "a given member is on the stub");
  assert.deepEqual(got.location, { protocol: "http:", host: "127.0.0.1:7431" });
  assert.deepEqual(got.deps, { messages: "door" });
});

test("fixture reads the JSON a daemon test wrote under fixtures/", () => {
  const reply = fixture("api-attention");
  assert.ok(Array.isArray(reply.requests), "the file's object, parsed");
  assert.throws(() => fixture("no-such-fixture"), /ENOENT/u);
});

test("swapFetch answers with the fake, records each call, and puts fetch back when the test ends", async (t) => {
  const real = globalThis.fetch;
  await t.test("inside", async (inner) => {
    const calls = swapFetch(inner, (url) => Promise.resolve(`answer ${url}`));
    assert.equal(await fetch("/api/repos", { method: "GET" }), "answer /api/repos");
    assert.deepEqual(calls, [{ url: "/api/repos", init: { method: "GET" } }]);
  });
  assert.equal(globalThis.fetch, real, "the real fetch is back");
});

test("fakeWindow delivers an event to the listeners of its type and keeps it", () => {
  const win = fakeWindow({ innerWidth: 800 });
  const heard = [];
  const fn = (e) => heard.push(e.detail);
  win.addEventListener("a", fn);
  win.addEventListener("b", () => heard.push("b"));
  assert.equal(win.dispatchEvent({ type: "a", detail: 1 }), true);
  win.removeEventListener("a", fn);
  win.dispatchEvent({ type: "a", detail: 2 });
  assert.deepEqual(heard, [1]);
  assert.deepEqual(win.events.map((e) => e.detail), [1, 2]);
  assert.equal(win.innerWidth, 800);
});

test("fakeClassList answers as the DOM's classList does", () => {
  const list = fakeClassList("open");
  assert.equal(list.contains("open"), true);
  list.add("a", "b");
  list.remove("open");
  assert.equal(list.toggle("a"), false, "toggle turns an on name off");
  assert.equal(list.toggle("c"), true, "and an off name on");
  assert.equal(list.toggle("b", true), true, "force keeps a name on");
  assert.equal(list.toggle("x", false), false, "force keeps a name off");
  assert.deepEqual([...list].toSorted(), ["b", "c"]);
});

test("fakeClassList shows its names through has and the live classes set", () => {
  const list = fakeClassList("open");
  list.add("a");
  assert.equal(list.has("a"), true);
  assert.equal(list.has("b"), false);
  list.classes.add("b");
  assert.equal(list.contains("b"), true, "the set is the list's own, not a copy");
  assert.deepEqual([...list.classes].toSorted(), ["a", "b", "open"]);
});

test("withoutBroadcast hides BroadcastChannel for the call only, even when it throws", () => {
  const real = globalThis.BroadcastChannel;
  const channel = { sentinel: "a BroadcastChannel" };
  globalThis.BroadcastChannel = channel;
  try {
    assert.equal(withoutBroadcast(() => typeof globalThis.BroadcastChannel), "undefined");
    assert.equal(globalThis.BroadcastChannel, channel, "back after a return");
    assert.throws(() => withoutBroadcast(() => {
      throw new Error("boom");
    }), /boom/u);
    assert.equal(globalThis.BroadcastChannel, channel, "back after a throw");
  } finally {
    globalThis.BroadcastChannel = real;
  }
});
