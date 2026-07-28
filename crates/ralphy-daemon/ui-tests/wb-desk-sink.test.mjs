// Unit tests for assets/ui/wb-desk-sink.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-desk-sink.js"),
  "utf8",
);

// The module's only load-time global is `window`, which it assigns onto.
function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBDeskSink;
}

// A counting `fetch` spy installed on the global, which is what the module's
// bare `fetch(...)` call resolves to under Node.
function spyFetch() {
  const calls = [];
  const prev = globalThis.fetch;
  globalThis.fetch = (url, init) => {
    calls.push({ url, init });
    return Promise.resolve({ ok: true });
  };
  return {
    calls,
    restore() {
      globalThis.fetch = prev;
    },
  };
}

const BODY = JSON.stringify({ windows: [], fences: [] });

test("the null sink writes nowhere — neither put nor putSync reaches fetch", async () => {
  const spy = spyFetch();
  try {
    const sink = load().none();
    await sink.put(BODY);
    sink.putSync(BODY);
    assert.equal(spy.calls.length, 0);
  } finally {
    spy.restore();
  }
});

// The NEGATIVE CONTROL for the test above: a sink test that never saw a write
// would pass against a `fetch` that was simply never wired up.
test("the daemon sink PUTs the body to /api/desk — the control the null sink is measured against", async () => {
  const spy = spyFetch();
  try {
    const sink = load().daemon();
    await sink.put(BODY);
    assert.equal(spy.calls.length, 1);
    assert.equal(spy.calls[0].url, "/api/desk");
    assert.equal(spy.calls[0].init.method, "PUT");
    assert.equal(spy.calls[0].init.body, BODY);
    assert.equal(spy.calls[0].init.headers["Content-Type"], "application/json");
  } finally {
    spy.restore();
  }
});

test("the daemon sink's putSync rides keepalive, so a closing tab's last write survives", async () => {
  const spy = spyFetch();
  try {
    load().daemon().putSync(BODY);
    assert.equal(spy.calls.length, 1);
    assert.equal(spy.calls[0].init.keepalive, true);
    assert.equal(spy.calls[0].init.method, "PUT");
  } finally {
    spy.restore();
  }
});

test("the daemon sink chains its writes, so two mutations cannot land out of order", async () => {
  const order = [];
  const prev = globalThis.fetch;
  let release;
  let sawFirst;
  const gate = new Promise((r) => (release = r));
  const firstCalled = new Promise((r) => (sawFirst = r));
  globalThis.fetch = (url, init) => {
    order.push(JSON.parse(init.body).windows[0]);
    // The FIRST write hangs until released; an unchained sink would issue the
    // second one while it is still open.
    if (order.length === 1) {
      sawFirst();
      return gate.then(() => ({ ok: true }));
    }
    return Promise.resolve({ ok: true });
  };
  try {
    const sink = load().daemon();
    const first = sink.put(JSON.stringify({ windows: ["first"], fences: [] }));
    const second = sink.put(JSON.stringify({ windows: ["second"], fences: [] }));
    // Wait on the FIRST call landing rather than on a guessed number of
    // microtask turns: the sink defers even its first fetch onto the chain.
    await firstCalled;
    assert.deepStrictEqual(order, ["first"], "the second write must wait on the first");
    release();
    // Both chains are drained before this test returns, so nothing it queued
    // can fire against the NEXT test's spy.
    await Promise.all([first, second]);
    assert.deepStrictEqual(order, ["first", "second"]);
  } finally {
    release();
    globalThis.fetch = prev;
  }
});

// A rejected PUT must not poison the chain: the next mutation still writes.
test("a refused PUT does not stop the next one — the chain swallows the rejection", async () => {
  const calls = [];
  const prev = globalThis.fetch;
  globalThis.fetch = (url, init) => {
    calls.push(init.body);
    return calls.length === 1 ? Promise.reject(new Error("offline")) : Promise.resolve({ ok: true });
  };
  try {
    const sink = load().daemon();
    await sink.put(BODY);
    await sink.put(BODY);
    assert.equal(calls.length, 2);
  } finally {
    globalThis.fetch = prev;
  }
});
