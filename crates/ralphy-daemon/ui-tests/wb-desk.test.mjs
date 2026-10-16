// Unit tests for assets/ui/wb-desk.ts — the console's desk: the desk read,
// sending the desk changes and restoring the desk layout. `createDesk` is
// driven with fake `deps` (a fake sync, sink, store and stage) and a fake
// `fetch` for the desk read, with no console and no browser. The popup
// registry is the real one over a fake store.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDesk } from "../assets/ui/wb-desk.ts";
import { createPopupRegistry } from "../assets/ui/wb-desk-popups.ts";

const settle = () => new Promise((resolve) => setImmediate(resolve));

// The desk read the daemon answers: a desk this page takes.
const readOk = () => Promise.resolve({ ok: true, status: 200, json: async () => ({}) });

// A desk over fakes that record each call. `over` replaces any dep; `read`
// answers the desk read, which `createDesk` starts while it is built.
function fakeDesk(over = {}, read = readOk) {
  const calls = { put: [], putSync: [], acked: [], listeners: [], spawned: [], placeholders: [], glyphs: [], events: [], posts: [], landings: 0 };
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
    window: { addEventListener: (type) => calls.listeners.push(type) },
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
      take: () => "taken",
      view: () => ({ windows: [], fences: [], notes: [], checkouts: {} }),
      hasPending: () => false,
      acked: (reply) => calls.acked.push(reply),
    },
    viewStore: { read: () => ({}) },
    link,
    popups: null,
    wins: new Set(),
    stage: () => null,
    peerGroups: () => new Map(),
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
    inGesture: () => false,
    nameUnnamed: () => {},
    createRecord: () => {},
    applyLock: () => {},
    renderTitle: () => {},
    dropClosedElsewhere: () => {},
    flushPendingOffset: () => {},
    ...over,
  };
  deps.popups = over.popups || createPopupRegistry({ link: deps.link, startBeat: () => {}, stopBeat: () => {} });
  // The read calls `fetch` while the desk is built, and only then.
  const realFetch = globalThis.fetch;
  globalThis.fetch = read;
  let desk;
  try {
    desk = createDesk(deps);
  } finally {
    globalThis.fetch = realFetch;
  }
  return { desk, deps, calls, landing };
}

// `restoreDesk` and `whenDeskLoaded` wait for this read, and the `pagehide`
// flush sends what the page changed after it.
test("the desk read starts while the desk is built, and the tab close is heard from then on", () => {
  const reads = [];
  const { calls } = fakeDesk({}, (url, init) => {
    reads.push([String(url), init?.method]);
    return new Promise(() => {});
  });
  assert.deepEqual(reads, [["/api/desk", "GET"]], "the read is on the wire before createDesk returns");
  assert.deepEqual(calls.listeners, ["pagehide"]);
});

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
  assert.equal(calls.acked.length, 1, "the answer reached the sync");
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  assert.equal(calls.put.length, 2);
});

test("an upload the network lost is sent again a second later", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { desk, calls } = fakeDesk({
    deskSink: {
      put: (body) => {
        calls.put.push(JSON.parse(body));
        return Promise.reject(new Error("offline"));
      },
      putSync: () => {},
    },
  });
  desk.scheduleDeskFlush();
  t.mock.timers.tick(250);
  await settle();
  assert.equal(calls.put.length, 1);
  assert.equal(calls.acked.length, 0, "a lost upload is not an answer");
  t.mock.timers.tick(1000);
  assert.equal(calls.put.length, 2);
});

test("the last batch as the tab closes goes by putSync, and the waiting upload does not go too", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { desk, calls } = fakeDesk();
  await desk.whenDeskLoaded();
  desk.scheduleDeskFlush();
  desk.flushDeskOnClose();
  assert.deepEqual(calls.putSync, [{ closing: true }]);
  t.mock.timers.tick(1000);
  assert.equal(calls.put.length, 0);
});

test("a page that has not read the desk sends nothing as the tab closes", async () => {
  const { desk, calls } = fakeDesk({}, () => Promise.reject(new Error("offline")));
  await desk.whenDeskLoaded();
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

// A peer project: its ref starts with the peer daemon's id.
const PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";

test("a relaunch into a worktree is waited for before the stage is sized", async () => {
  const record = { id: "w1", repo: "o/r", agent: "claude", kind: "agent", checkout: "wt-a", rect: {} };
  let land;
  const launched = [];
  const order = [];
  const { desk, landing } = fakeDesk({
    viewStore: { read: () => ({ relaunch: true }) },
    loadDesk: () => [record],
    spawnOrMissing: (...args) => {
      launched.push(args);
      return new Promise((resolve) => (land = resolve));
    },
    applyExtent: () => order.push("extent"),
    document: { dispatchEvent: (e) => order.push(e.type) },
  });
  desk.restoreDesk();
  await settle();
  assert.deepEqual(launched, [[{ repo: "o/r", agent: "claude", checkout: "wt-a" }, "claude", "o/r", record]]);
  assert.deepEqual(order, [], "the stage waits for the window that is still asking for its tree");
  land();
  await landing;
  assert.deepEqual(order, ["extent", "workbench:desk-restored"]);
});

test("a record the restore cannot launch gets a placeholder that says why", async () => {
  const records = [
    { id: "agent", repo: "o/r", agent: "claude", kind: "agent", rect: {} },
    { id: "agent-peer", repo: `${PEER}/o/r`, agent: "claude", kind: "agent", rect: {} },
    { id: "shell-peer", repo: `${PEER}/o/s`, agent: "console", kind: "console", rect: {} },
    { id: "shell", repo: "o/u", agent: "console", kind: "console", rect: {} },
  ];
  // What the restore did with each record, in its order.
  const done = [];
  const run = async (unheard) => {
    done.length = 0;
    const { desk, landing } = fakeDesk({
      loadDesk: () => records.slice(),
      readSessions: () => Promise.resolve({ sessions: [], unheard }),
      peerGroups: () => new Map([[PEER, { daemon: PEER, state: "asleep" }]]),
      spawnOrMissing: (request, agent, repo, rec) => {
        done.push(["launch", rec.id]);
        return Promise.resolve();
      },
      spawnPlaceholder: (rec, ...rest) => done.push([rec.id, ...rest]),
    });
    desk.restoreDesk();
    await landing;
  };
  // The fleet list did not hear from the peer: its consoles may still run there.
  await run(new Set([PEER]));
  assert.deepEqual(done, [
    ["agent", null, null, { unheard: false }],
    ["agent-peer", null, null, { unheard: true }],
    ["shell-peer", null, null, { unheard: true }],
    ["launch", "shell"],
  ]);
  // Heard from, the peer is asleep: its shell waits, and launches nothing.
  await run(new Set());
  assert.deepEqual(done, [
    ["agent", null, null, { unheard: false }],
    ["agent-peer", null, null, { unheard: false }],
    ["shell-peer"],
    ["launch", "shell"],
  ]);
});

test("a live session no record holds is adopted, under the record it names", async () => {
  const session = { id: 7, record: "w9", repo: "o/r", agent: "claude", kind: "agent", daemon_id: "D1", environment: "WSL: Ubuntu" };
  const { desk, calls, landing } = fakeDesk({
    readSessions: () => Promise.resolve({ sessions: [session, { id: 8, repo: "o/r", kind: "console" }], unheard: [] }),
  });
  desk.restoreDesk();
  await landing;
  assert.deepEqual(calls.spawned, [
    {
      termOpts: { id: 7, repo: "o/r" },
      record: { id: "w9", kind: "agent", daemonId: "D1", environment: "WSL: Ubuntu", checkout: null, unrecorded: true },
    },
    {
      termOpts: { id: 8, repo: "o/r" },
      record: { id: undefined, kind: "console", daemonId: undefined, environment: undefined, checkout: null, unrecorded: true },
    },
  ]);
});

test("a window already on the stage is not restored twice", async () => {
  const layout = [
    { id: "w1", repo: "r", agent: "a", kind: "console", sessionId: 1, rect: {} },
    { id: "w2", repo: "r", agent: "a", kind: "console", sessionId: 2, rect: {} },
  ];
  const sessions = [
    { id: 1, record: "w1", repo: "r", agent: "a" },
    { id: 2, record: "w2", repo: "r", agent: "a" },
    { id: 3, record: "w3", repo: "r", agent: "a" },
  ];
  const { desk, calls, landing } = fakeDesk({
    wins: new Set([{ _deskId: "w1" }, { _deskId: "w3" }]),
    loadDesk: () => layout.slice(),
    readSessions: () => Promise.resolve({ sessions, unheard: [] }),
  });
  desk.restoreDesk();
  await landing;
  assert.deepEqual(
    calls.spawned.map((s) => s.record.id),
    ["w2"],
    "a re-attach during the restore already put w1 and w3 back",
  );
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
