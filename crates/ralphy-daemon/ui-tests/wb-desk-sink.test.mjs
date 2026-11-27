// Unit tests for assets/ui/wb-desk-sink.ts — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";
import { swapFetch } from "./harness.mjs";

// A counting `fetch` spy on the global, which is what the module's bare
// `fetch(...)` call resolves to under Node. `swapFetch` returns the calls.
const answerOk = () => Promise.resolve({ ok: true });

const BODY = JSON.stringify({ windows: [], fences: [] });

test("the null sink writes nowhere — neither put nor putSync reaches fetch", async (t) => {
  const calls = swapFetch(t, answerOk);
  const sink = WBDeskSink.none();
  await sink.put(BODY);
  sink.putSync(BODY);
  assert.equal(calls.length, 0);
});

// The NEGATIVE CONTROL for the test above: a sink test that never saw a write
// would pass against a `fetch` that was simply never wired up.
test("the daemon sink PUTs the body to /api/desk — the control the null sink is measured against", async (t) => {
  const calls = swapFetch(t, answerOk);
  const sink = WBDeskSink.daemon();
  await sink.put(BODY);
  assert.equal(calls.length, 1);
  assert.match(calls[0].url, /^\/api\/desk\?tab=/u);
  assert.equal(calls[0].init.method, "PUT");
  assert.equal(calls[0].init.body, BODY);
  assert.equal(calls[0].init.headers["Content-Type"], "application/json");
});

test("the daemon sink's putSync rides keepalive, so a closing tab's last write survives", (t) => {
  const calls = swapFetch(t, answerOk);
  WBDeskSink.daemon().putSync(BODY);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].init.keepalive, true);
  assert.equal(calls[0].init.method, "PUT");
});

test("the daemon sink chains its writes, so two mutations cannot land out of order", async (t) => {
  const order = [];
  let release;
  let sawFirst;
  const gate = new Promise((r) => {
    release = r;
  });
  const firstCalled = new Promise((r) => {
    sawFirst = r;
  });
  swapFetch(t, (_url, init) => {
    order.push(JSON.parse(init.body).windows[0]);
    // The FIRST write hangs until released; an unchained sink would issue the
    // second one while it is still open.
    if (order.length === 1) {
      sawFirst();
      return gate.then(() => ({ ok: true }));
    }
    return Promise.resolve({ ok: true });
  });
  try {
    const sink = WBDeskSink.daemon();
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
  }
});

// A rejected PUT must not poison the chain: the next mutation still writes.
test("a refused PUT does not stop the next one — the chain swallows the rejection", async (t) => {
  const calls = [];
  swapFetch(t, (_url, init) => {
    calls.push(init.body);
    return calls.length === 1
      ? Promise.reject(new Error("offline"))
      : Promise.resolve({ ok: true });
  });
  const sink = WBDeskSink.daemon();
  await sink.put(BODY);
  await sink.put(BODY);
  assert.equal(calls.length, 2);
});

test("a held sink writes nothing, and writes again when released", async (t) => {
  const calls = swapFetch(t, answerOk);
  const mod = WBDeskSink;
  const sink = mod.daemon();
  mod.setHold(true);
  await sink.put(BODY);
  sink.putSync(BODY);
  assert.equal(calls.length, 0);
  mod.setHold(false);
  await sink.put(BODY);
  assert.equal(calls.length, 1);
});
