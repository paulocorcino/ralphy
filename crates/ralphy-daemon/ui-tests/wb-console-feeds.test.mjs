// Unit tests for assets/ui/wb-console-feeds.ts — the last facts the shell fed.
// `createFeeds` is driven with stubs and no DOM.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { createFeeds } from "../assets/ui/wb-console-feeds.ts";

function feeds(OPTS = {}) {
  return createFeeds({
    OPTS,
    wins: new Set(),
    pageWindows: () => [],
    renderTitle() {},
  });
}

test("the getters return the last value fed, read after the factory was built", () => {
  const f = feeds({ isStale: () => false });
  assert.deepEqual(f.lastSessions(), []);
  assert.equal(f.wakePeerNow(), null);
  assert.equal(f.readFleetNow(), null);
  const probe = () => true;
  f.setStaleProbe(probe);
  assert.equal(f.staleProbeNow(), probe);
  f.setStaleProbe(null);
  assert.equal(f.staleProbeNow(), null);
  const wake = async () => true;
  const read = () => {};
  f.ingestFleet([], { wake, read });
  assert.equal(f.wakePeerNow(), wake);
  assert.equal(f.readFleetNow(), read);
  const rows = [{ id: "s1" }];
  f.ingestSessions(rows);
  assert.equal(f.lastSessions(), rows);
  f.ingestSessions(null);
  assert.deepEqual(f.lastSessions(), []);
});

test("ingestFleet keeps only peer groups", () => {
  const f = feeds();
  const peer = { daemon: "d2", local: false };
  f.ingestFleet([{ daemon: "d1", local: true }, peer, { daemon: "", local: false }, null], null);
  assert.deepEqual([...f.peerGroups.keys()], ["d2"]);
  assert.equal(f.peerGroups.get("d2"), peer);
});

test("a shared sessions read that passed its deadline is not shared again", async () => {
  const prev = globalThis.fetch;
  let fetches = 0;
  // The stall report that the deadline sends is not a sessions read.
  globalThis.fetch = (url) => {
    if (url !== "/api/device/stall") fetches += 1;
    return new Promise(() => {});
  };
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const f = feeds();
    const first = f.readSessions().catch((e) => e);
    const second = f.readSessions().catch((e) => e);
    assert.equal(fetches, 1, "the second read shares the first request");
    mock.timers.tick(20000);
    await new Promise((r) => setImmediate(r));
    assert.equal((await first).name, "ReadDeadline");
    assert.equal((await second).name, "ReadDeadline");
    f.readSessions().catch(() => {});
    assert.equal(fetches, 2, "a read after the deadline makes a new request");
  } finally {
    mock.timers.reset();
    globalThis.fetch = prev;
  }
});
