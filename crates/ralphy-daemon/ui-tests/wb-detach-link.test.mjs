// Unit tests for assets/ui/wb-detach-link.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-detach-link.js"),
  "utf8",
);

// The module's only load-time global is `window`, which it assigns onto — the
// storage and the channel are reached as BARE globals, exactly as the browser
// resolves them and exactly as `wb-desk-sink.js` reaches `fetch`. So the fakes
// below are installed on `globalThis`, like that module's `spyFetch`.
function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBDetachLink;
}

// One tab's session-scoped store. A SEPARATE instance is a SEPARATE tab, which
// is the isolation rule this module exists to enforce.
function fakeStorage() {
  const map = new Map();
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, String(v)),
    removeItem: (k) => map.delete(k),
    _map: map,
  };
}

// Swap a global for the duration of one test, restoring whatever was there —
// including "nothing", which is the absence case two tests below need.
function withGlobals(patch, body) {
  const prev = new Map();
  for (const [k, v] of Object.entries(patch)) {
    prev.set(k, k in globalThis ? globalThis[k] : undefined);
    if (v === undefined) delete globalThis[k];
    else globalThis[k] = v;
  }
  try {
    return body();
  } finally {
    for (const [k, v] of prev) {
      if (v === undefined) delete globalThis[k];
      else globalThis[k] = v;
    }
  }
}

test("the registry round-trips through the tab's session storage", () => {
  withGlobals({ sessionStorage: fakeStorage() }, () => {
    const link = load().link();
    assert.deepEqual(link.readRegistry(), []);
    link.writeRegistry(["f-alpha"]);
    assert.deepEqual(link.readRegistry(), ["f-alpha"]);
  });
});

// The key's PRESENCE is the fact "this tab holds a detach", which a boot-time
// write would forge for every tab that ever loaded the shell.
test("opening a link writes nothing — a tab that detached nothing leaves no record", () => {
  const store = fakeStorage();
  withGlobals({ sessionStorage: store }, () => {
    const link = load().link();
    assert.equal(store.getItem("wb.detach.v1"), null);
    assert.ok(link.tab, "the identity is still minted, in memory");
  });
});

test("the registry is stored under wb.detach.v1 as a versioned record", () => {
  const store = fakeStorage();
  withGlobals({ sessionStorage: store }, () => {
    const link = load().link();
    link.writeRegistry(["f-alpha", "f-beta"]);
    const rec = JSON.parse(store.getItem("wb.detach.v1"));
    assert.equal(rec.v, 1);
    assert.deepEqual(rec.fences, ["f-alpha", "f-beta"]);
    assert.equal(rec.tab, link.tab);
  });
});

// THE ISOLATION RULE, at unit level: a second tab of the same browser gets its
// own store, so it sees no detach of the first and carries a different identity.
test("a second tab sees an empty registry and a different tab id", () => {
  let first;
  withGlobals({ sessionStorage: fakeStorage() }, () => {
    first = load().link();
    first.writeRegistry(["f-alpha"]);
  });
  withGlobals({ sessionStorage: fakeStorage() }, () => {
    const second = load().link();
    assert.deepEqual(second.readRegistry(), [], "a second tab must not see tab one's detaches");
    assert.notEqual(second.tab, first.tab);
  });
});

// The other half of the same rule, and the reload survival the slice is about:
// the SAME store answers with the SAME identity and the SAME registry.
test("re-loading against the same store keeps the tab id and the registry — the F5", () => {
  const store = fakeStorage();
  let before;
  withGlobals({ sessionStorage: store }, () => {
    before = load().link();
    before.writeRegistry(["f-alpha"]);
  });
  withGlobals({ sessionStorage: store }, () => {
    const after = load().link();
    assert.equal(after.tab, before.tab);
    assert.deepEqual(after.readRegistry(), ["f-alpha"]);
  });
});

test("a corrupt or foreign-version record reads as an empty registry", () => {
  const store = fakeStorage();
  store.setItem("wb.detach.v1", "{not json");
  withGlobals({ sessionStorage: store }, () => {
    assert.deepEqual(load().link().readRegistry(), []);
  });
  store.setItem("wb.detach.v1", JSON.stringify({ v: 9, tab: "t-x", fences: ["f-alpha"] }));
  withGlobals({ sessionStorage: store }, () => {
    assert.deepEqual(load().link().readRegistry(), []);
  });
});

test("no sessionStorage at all (private mode) degrades to an empty registry, never a throw", () => {
  withGlobals({ sessionStorage: undefined }, () => {
    const link = load().link();
    assert.deepEqual(link.readRegistry(), []);
    link.writeRegistry(["f-alpha"]);
    assert.deepEqual(link.readRegistry(), []);
    assert.ok(link.tab, "a tab identity is still minted so the channel can be scoped");
  });
});

test("a throwing sessionStorage (quota, blocked cookies) degrades the same way", () => {
  const hostile = {
    getItem() {
      throw new Error("blocked");
    },
    setItem() {
      throw new Error("quota");
    },
  };
  withGlobals({ sessionStorage: hostile }, () => {
    const link = load().link();
    assert.deepEqual(link.readRegistry(), []);
    link.writeRegistry(["f-alpha"]);
  });
});

// A browser-wide channel: two links on the same "browser" hear each other. This
// is the POSITIVE CONTROL for the absence test below — without it, a module that
// never posted anything would pass that test vacuously.
test("post reaches every other link on the channel", () => {
  const buses = new Map();
  class FakeBroadcastChannel {
    constructor(name) {
      this.name = name;
      this.onmessage = null;
      if (!buses.has(name)) buses.set(name, new Set());
      buses.get(name).add(this);
    }
    postMessage(data) {
      for (const peer of buses.get(this.name)) {
        if (peer !== this && peer.onmessage) peer.onmessage({ data });
      }
    }
    close() {
      buses.get(this.name).delete(this);
    }
  }
  withGlobals({ sessionStorage: fakeStorage(), BroadcastChannel: FakeBroadcastChannel }, () => {
    const mod = load();
    const a = mod.link();
    const b = mod.link();
    const heard = [];
    b.onMessage((m) => heard.push(m));
    a.post({ type: "origin-beat", tab: a.tab });
    assert.deepEqual(heard, [{ type: "origin-beat", tab: a.tab }]);
    assert.equal(buses.get("wb.detach.v1").size, 2, "the channel name is wb.detach.v1");
    b.close();
    a.post({ type: "origin-beat", tab: a.tab });
    assert.equal(heard.length, 1, "a closed link hears nothing more");
  });
});

test("no BroadcastChannel at all degrades to silence, never a throw", () => {
  withGlobals({ sessionStorage: fakeStorage(), BroadcastChannel: undefined }, () => {
    const link = load().link();
    link.onMessage(() => {});
    link.post({ type: "origin-beat" });
    link.close();
  });
});

// The popup's link. `none()` is what makes that document INCAPABLE of acting on
// a registry — `window.open` hands it a COPY of the opener's sessionStorage, so
// a real link there would read a plausible ghost.
test("none() is inert: no tab identity, an empty registry, and writes that go nowhere", () => {
  const store = fakeStorage();
  store.setItem("wb.detach.v1", JSON.stringify({ v: 1, tab: "t-opener", fences: ["f-alpha"] }));
  withGlobals({ sessionStorage: store }, () => {
    const link = load().none();
    assert.equal(link.tab, null);
    assert.deepEqual(link.readRegistry(), [], "the inherited copy must stay invisible");
    link.writeRegistry(["f-beta"]);
    link.post({ type: "origin-beat" });
    link.onMessage(() => {});
    link.close();
    const rec = JSON.parse(store.getItem("wb.detach.v1"));
    assert.deepEqual(rec.fences, ["f-alpha"], "none() never writes");
  });
});

// The member ids ride the registry so the boot skip never depends on geometry:
// a detached fence may still be MOVED on the plane, after which a membership
// fold comparing its new rect with the members' original records answers "no
// members" and puts every one of them back under a live popup.
test("the registry carries each detached fence's member ids, and they survive a reload", () => {
  const store = fakeStorage();
  withGlobals({ sessionStorage: store }, () => {
    const link = load().link();
    assert.deepEqual(link.readMembers(), {});
    link.writeRegistry(["f-alpha"], { "f-alpha": ["w-m1", "w-m2"] });
    assert.deepEqual(link.readMembers(), { "f-alpha": ["w-m1", "w-m2"] });
  });
  withGlobals({ sessionStorage: store }, () => {
    assert.deepEqual(load().link().readMembers(), { "f-alpha": ["w-m1", "w-m2"] });
  });
});

test("a registry written with no members reads back an empty map, never undefined", () => {
  withGlobals({ sessionStorage: fakeStorage() }, () => {
    const link = load().link();
    link.writeRegistry(["f-alpha"]);
    assert.deepEqual(link.readMembers(), {});
    assert.deepEqual(link.readRegistry(), ["f-alpha"]);
  });
});

// The popup's factory: the channel and NOTHING else. `window.open` hands that
// document a COPY of its opener's store, so it must not even READ one.
test("channel() reaches the channel and no store at all", () => {
  const store = fakeStorage();
  store.setItem(
    "wb.detach.v1",
    JSON.stringify({ v: 1, tab: "t-opener", fences: ["f-alpha"], members: {} }),
  );
  let got = null;
  const buses = new Map();
  class FakeBroadcastChannel {
    constructor(name) {
      this.name = name;
      this.onmessage = null;
      if (!buses.has(name)) buses.set(name, new Set());
      buses.get(name).add(this);
    }
    postMessage(data) {
      for (const peer of buses.get(this.name)) if (peer !== this && peer.onmessage) peer.onmessage({ data });
    }
    close() {
      buses.get(this.name).delete(this);
    }
  }
  withGlobals({ sessionStorage: store, BroadcastChannel: FakeBroadcastChannel }, () => {
    const mod = load();
    const popup = mod.channel();
    assert.equal(popup.tab, null, "the popup is TOLD its opener's id, never derived one");
    assert.equal(popup.readRegistry, undefined, "channel() exposes no registry surface at all");
    assert.equal(popup.writeRegistry, undefined);
    assert.equal(popup.readMembers, undefined);
    // …but the channel itself works: the origin hears it.
    const origin = mod.link();
    origin.onMessage((m) => (got = m));
    popup.post({ type: "popup-here", tab: "t-opener", fenceId: "f-alpha" });
    assert.deepEqual(got, { type: "popup-here", tab: "t-opener", fenceId: "f-alpha" });
  });
});

test("the heartbeat constants are the ones the callers read off the module", () => {
  const mod = load();
  assert.equal(mod.HEARTBEAT_MS, 1000);
  assert.equal(mod.PEER_WINDOW_MS, 6000);
  assert.equal(mod.KEY, "wb.detach.v1");
  assert.equal(mod.CHANNEL, "wb.detach.v1");
});
