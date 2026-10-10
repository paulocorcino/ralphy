// Unit tests for assets/ui/wb-desk.ts — the console's desk: sending
// the desk changes and restoring the desk layout. `createDesk` is driven with
// fake `deps` (a fake sync, sink, store and stage), with no console and no
// browser. The popup registry is the real one over a fake store.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDesk } from "../assets/ui/wb-desk.ts";
import { createPopupRegistry } from "../assets/ui/wb-desk-popups.ts";

const settle = () => new Promise((resolve) => setImmediate(resolve));

// A desk over fakes that record each call. `over` replaces any dep.
function fakeDesk(over = {}) {
  const calls = { put: [], putSync: [], flushed: [], spawned: [], placeholders: [], glyphs: [], events: [], posts: [], landings: 0 };
  let batch = 0;
  let landed;
  const landing = new Promise((resolve) => {
    landed = resolve;
  });
  const link = {
    tab: "tab-1",
    readRegistry: () => [],
    readMembers: () => ({}),
    writeRegistry: () => {},
    post: (m) => calls.posts.push(m),
  };
  const deps = {
    window: {},
    document: { dispatchEvent: (e) => calls.events.push(e.type) },
    OPTS: {},
    deskSink: {
      put: (body) => {
        calls.put.push(JSON.parse(body));
        return Promise.resolve({ kind: "ok" });
      },
      putSync: (body) => calls.putSync.push(JSON.parse(body)),
    },
    sync: {
      phase: () => "ready",
      emit: () => {},
      nextBatch: () => ({ seq: ++batch }),
      closingBatch: () => ({ closing: true }),
    },
    viewStore: { read: () => ({}) },
    link,
    popups: null,
    wins: new Set(),
    fences: () => [],
    peerGroups: () => new Map(),
    deskLoaded: () => true,
    currentDeskFailure: () => "",
    whenDeskLoaded: () => Promise.resolve(),
    reloadDesk: () => Promise.resolve(),
    refreshView: () => {},
    flushed: (out) => calls.flushed.push(out),
    loadDesk: () => [],
    readSessions: () => Promise.resolve({ sessions: [], unheard: [] }),
    spawnWindow: (termOpts, label, repo, record) => calls.spawned.push({ termOpts, record }),
    spawnOrMissing: () => Promise.resolve(),
    spawnPlaceholder: (record) => calls.placeholders.push(record),
    renderFences: () => {},
    renderNotes: () => {},
    showDetachGlyph: (id, on) => calls.glyphs.push([id, on]),
    applyExtent: () => {},
    raiseMaximized: () => {},
    applyLanding: () => {
      calls.landings += 1;
      landed();
    },
    ...over,
  };
  deps.popups = over.popups || createPopupRegistry({ link: deps.link, startBeat: () => {}, stopBeat: () => {} });
  return { desk: createDesk(deps), deps, calls, landing };
}

test("two desk changes close together make one upload", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { desk, calls } = fakeDesk();
  desk.emitDesk({ op: "set", type: "window", id: "w1", fields: {} });
  t.mock.timers.tick(100);
  desk.emitDesk({ op: "set", type: "window", id: "w1", fields: {} });
  t.mock.timers.tick(200);
  assert.equal(calls.put.length, 0, "the second change waits a full 250 ms again");
  t.mock.timers.tick(50);
  assert.deepEqual(calls.put, [{ seq: 1 }]);
});

test("a flush during a flush waits for the batch on the wire", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let answer;
  const { desk, calls } = fakeDesk({
    deskSink: {
      put: (body) => {
        calls.put.push(JSON.parse(body));
        return new Promise((resolve) => {
          answer = resolve;
        });
      },
      putSync: () => {},
    },
  });
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  assert.equal(calls.put.length, 1, "one batch is on the wire at a time");

  answer({ kind: "ok" });
  await settle();
  assert.deepEqual(calls.flushed, [{ kind: "ok" }]);
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  assert.equal(calls.put.length, 2);
});

test("an upload the network lost is answered as a network failure", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { desk, calls } = fakeDesk({
    deskSink: { put: () => Promise.reject(new Error("offline")), putSync: () => {} },
  });
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  await settle();
  assert.deepEqual(calls.flushed, [{ kind: "network" }]);
});

test("the last batch as the tab closes goes by putSync, and the waiting upload does not go too", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { desk, calls } = fakeDesk();
  desk.scheduleDeskFlush();
  desk.flushDeskOnClose();
  assert.deepEqual(calls.putSync, [{ closing: true }]);
  t.mock.timers.tick(1000);
  assert.equal(calls.put.length, 0);
});

test("a page that has not read the desk sends nothing as the tab closes", () => {
  const { desk, calls } = fakeDesk({ deskLoaded: () => false });
  desk.flushDeskOnClose();
  assert.deepEqual(calls.putSync, []);
});

test("the restore reads the detached fences from the registry and skips their members", async () => {
  const layout = [
    { id: "w1", repo: "r", agent: "a", kind: "console", sessionId: 1, rect: {} },
    { id: "w2", repo: "r", agent: "a", kind: "console", sessionId: 2, rect: {} },
  ];
  const sessions = [
    { id: 1, record: "w1", repo: "r", agent: "a" },
    { id: 2, record: "w2", repo: "r", agent: "a" },
  ];
  const { desk, deps, calls, landing } = fakeDesk({
    loadDesk: () => layout.slice(),
    readSessions: () => Promise.resolve({ sessions, unheard: [] }),
  });
  deps.link.readRegistry = () => ["f1"];
  deps.link.readMembers = () => ({ f1: ["w1"] });

  desk.restoreDetached();
  assert.equal(deps.popups.isDetached("f1"), true);
  assert.deepEqual(calls.posts, [{ type: "origin-here", tab: "tab-1" }]);

  desk.restoreDesk();
  await landing;
  assert.deepEqual(
    calls.spawned.map((s) => s.record.id),
    ["w2"],
    "a member of a detached fence lives in its popup: no second window",
  );
  assert.deepEqual(
    deps.popups.entry("f1").members.map((m) => m.id),
    ["w1"],
    "the skipped record is the fallback the popup comes home to",
  );
  assert.deepEqual(calls.glyphs, [["f1", true]]);
  assert.deepEqual(calls.events, ["workbench:desk-restored"]);
});

test("the restore reconciles the layout once, whatever calls it again", async () => {
  const layout = [{ id: "w1", repo: "r", agent: "a", kind: "console", sessionId: 1, rect: {} }];
  const { desk, calls, landing } = fakeDesk({
    loadDesk: () => layout.slice(),
    readSessions: () => Promise.resolve({ sessions: [{ id: 1, record: "w1", repo: "r" }], unheard: [] }),
  });
  assert.equal(desk.isDeskReconciled(), false);
  assert.equal(desk.isDeskSettled(), false);
  desk.restoreDesk();
  desk.restoreDesk();
  await landing;
  await settle();
  assert.equal(calls.spawned.length, 1, "a second reconcile would spawn every window twice");
  assert.equal(desk.isDeskReconciled(), true);
  assert.equal(desk.isDeskSettled(), true);
});

test("a refused restore is settled anyway, and is not a reconciled layout", async () => {
  const { desk, calls, landing } = fakeDesk({
    readSessions: () => Promise.reject(new Error("sessions unavailable")),
  });
  desk.restoreDesk();
  await landing;
  assert.equal(desk.isDeskSettled(), true, "the landing must not wait for a desk that never comes");
  assert.equal(desk.isDeskReconciled(), false);
  assert.deepEqual(calls.spawned, []);
});
