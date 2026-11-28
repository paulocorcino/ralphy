// The pure folds of assets/ui/wb-console.ts: autoPan, peerFold, and that
// no fold mutates its arguments.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBChanges } from "../assets/ui/wb-changes.ts";
import { WBGeometry } from "../assets/ui/wb-geometry.ts";
import { idLookups } from "./fake-document.mjs";
import { load } from "./wb-console-fixture.mjs";

// Fixtures the autoPan tests and the no-mutation test read; the fold's own
// tests are in wb-geometry.test.mjs.
const PAN_VIEW = { left: 100, top: 50, right: 1100, bottom: 850 };
const BAND = 48;
const STEP = 24;
const EXT = { width: 2000, height: 1500 };

// ---- autoPan: the loop around panNudge ---------------------------------------
// One loop serves the window drag and the fence drag. Frames are queued by a
// stub `requestAnimationFrame` and run by hand, so the test sees each tick.
function withFrames(body) {
  const frames = new Map();
  let next = 0;
  const saved = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  globalThis.requestAnimationFrame = (cb) => {
    next += 1;
    frames.set(next, cb);
    return next;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  const runFrame = () => {
    const [id, cb] = frames.entries().next().value || [];
    if (!cb) return false;
    frames.delete(id);
    cb();
    return true;
  };
  try {
    body(frames, runFrame);
  } finally {
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = saved;
  }
}

function panHarness() {
  const ws = { scrollLeft: 0, scrollTop: 0, getBoundingClientRect: () => PAN_VIEW };
  const api = load({}, { ...idLookups((id) => (id === "workspace" ? ws : null)) });
  const node = { isConnected: true };
  const placed = [];
  const pan = api.autoPan(node, (p) => placed.push(p));
  return { ws, node, placed, pan };
}

test("autoPan: a pointer at the edge scrolls the plane and places again each frame", () => {
  withFrames((frames, runFrame) => {
    const { ws, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    assert.ok(runFrame());
    assert.ok(runFrame());
    assert.equal(ws.scrollLeft, 48);
    assert.equal(placed.length, 2);
    assert.equal(frames.size, 1, "the loop keeps running while the pointer is in the band");
  });
});

test("autoPan: stop ends the loop, so a released button pans nothing more", () => {
  withFrames((frames, runFrame) => {
    const { ws, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    pan.stop();
    assert.equal(frames.size, 0);
    assert.equal(runFrame(), false);
    assert.equal(ws.scrollLeft, 0);
  });
});

test("autoPan: a node that left the page ends the loop without placing it", () => {
  withFrames((frames, runFrame) => {
    const { ws, node, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    node.isConnected = false;
    assert.ok(runFrame());
    assert.equal(frames.size, 0);
    assert.equal(ws.scrollLeft, 0);
    assert.equal(placed.length, 0);
  });
});

test("autoPan: a pointer away from the edges starts no loop", () => {
  withFrames((frames) => {
    const { pan } = panHarness();
    pan.follow({ x: 600, y: 400 });
    assert.equal(frames.size, 0);
  });
});

// ---- peerFold: the heartbeat and the peer-lost transition (issue #347) -------
// The window is passed in, so these run in microseconds rather than six real
// seconds — which is the whole reason the rule is a pure fold and not a timer.
const WINDOW_MS = 6000;
const SEEN = 100000;

// Each row is one path of the fold: the state and the event it gets, and the
// effects and the state it gives back. The boundary rows are the negative
// controls: inverting the comparison, or dropping the beat's `seen` update,
// turns a row red while the rows beside it stay green.
const LOST = { seen: SEEN, lost: true };
const PEER_FOLD_ROWS = [
  {
    name: "silence past the window loses the peer",
    state: { seen: SEEN, lost: false },
    event: { type: "tick", at: SEEN + WINDOW_MS + 1 },
    effects: [{ type: "peer-lost" }],
    next: LOST,
  },
  {
    // The effect turns into a `window.close()`, so a SECOND tick must be silent.
    name: "a tick after the loss is silent: the loss is reported once",
    state: LOST,
    event: { type: "tick", at: SEEN + WINDOW_MS + 5000 },
    effects: [],
    next: LOST,
  },
  {
    name: "a beat inside the window moves the clock and loses nothing",
    state: { seen: SEEN, lost: false },
    event: { type: "beat", at: SEEN + 5000 },
    effects: [],
    next: { seen: SEEN + 5000, lost: false },
  },
  {
    name: "1001 ms of silence after a beat is inside the window: an F5 costs no popup",
    state: { seen: SEEN + 5000, lost: false },
    event: { type: "tick", at: SEEN + 6001 },
    effects: [],
    next: { seen: SEEN + 5000, lost: false },
  },
  {
    name: "the boundary is strict: a tick exactly at the window is still alive",
    state: { seen: SEEN, lost: false },
    event: { type: "tick", at: SEEN + WINDOW_MS },
    effects: [],
    next: { seen: SEEN, lost: false },
  },
  {
    name: "a peer never heard from does not expire: the boot-adoption grace",
    state: { seen: null, lost: false },
    event: { type: "tick", at: SEEN + 999999 },
    effects: [],
    next: { seen: null, lost: false },
  },
  {
    name: "loss is terminal: a beat after the loss does not resurrect the peer",
    state: LOST,
    event: { type: "beat", at: SEEN + 1 },
    effects: [],
    next: LOST,
  },
  {
    name: "an announced departure loses the peer without waiting the window out",
    state: { seen: SEEN, lost: false },
    event: { type: "gone" },
    effects: [{ type: "peer-lost" }],
    next: LOST,
  },
  {
    name: "an unknown event is inert, so a stray channel message cannot lose a peer",
    state: { seen: SEEN, lost: false },
    event: { type: "origin-ping" },
    effects: [],
    next: { seen: SEEN, lost: false },
  },
  {
    name: "no state at all starts as a peer never heard from",
    state: undefined,
    event: { type: "tick", at: SEEN },
    effects: [],
    next: { seen: null, lost: false },
  },
];

test("peerFold gives the effects and the state of every path of the heartbeat", () => {
  const WB = load();
  for (const row of PEER_FOLD_ROWS) {
    const out = WB.peerFold(row.state, row.event, WINDOW_MS);
    assert.deepStrictEqual(out.effects, row.effects, `${row.name}: effects`);
    assert.deepStrictEqual(out.state, row.next, `${row.name}: state`);
  }
});

// The folds and the geometry are pure: none mutates what it is given.
test("the pure folds mutate none of their arguments", () => {
  const WB = load();
  const detach = (event) => [
    `detachFold on ${event.type}`,
    [["f-a", "f-b"], event],
    (reg, e) => WB.detachFold(reg, e),
    // A NEW array comes back, never the input.
    (out, [reg]) => assert.notEqual(out.registry, reg, `detachFold on ${event.type} returns a new array`),
  ];
  const peer = (event) => [
    `peerFold on ${event.type}`,
    [{ seen: SEEN, lost: false }, event, WINDOW_MS],
    (...args) => WB.peerFold(...args),
  ];
  // [case, arguments, call, extra check on the result]
  const rows = [
    [
      "anchorIntoView",
      [{ left: 600, top: 400, width: 200, height: 100 }, { width: 1000, height: 700 }, EXT],
      (...args) => WB.anchorIntoView(...args),
    ],
    [
      "panNudge",
      [{ x: 1076, y: 400 }, { left: 100, top: 50, right: 1100, bottom: 850 }, BAND, STEP],
      (...args) => WB.panNudge(...args),
    ],
    [
      "bringIntoView",
      [
        { left: 1200, top: 400, width: 200, height: 100 },
        { width: 1000, height: 700 },
        { width: 2000, height: 1500 },
      ],
      (...args) => WB.bringIntoView(...args),
    ],
    [
      "fenceCycle",
      [
        [
          { id: "a", rect: { left: 0, top: 0, width: 400, height: 300 } },
          { id: "b", rect: { left: 500, top: 0, width: 400, height: 300 } },
        ],
        "a",
        1,
      ],
      (...args) => WB.fenceCycle(...args),
    ],
    detach({ type: "detach", fenceId: "f-new" }),
    detach({ type: "detach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-nope" }),
    detach({ type: "focus", fenceId: "f-a" }),
    detach({ type: "wat", fenceId: "f-a" }),
    peer({ type: "beat", at: SEEN + 10 }),
    peer({ type: "tick", at: SEEN + WINDOW_MS + 1 }),
    peer({ type: "gone" }),
    [
      "stageExtent",
      [
        [
          { left: 900, top: 40, width: 600, height: 380 },
          { left: 40, top: 600, width: 600, height: 380 },
        ],
        { width: 1000, height: 700 },
        200,
      ],
      (...args) => WBGeometry.stageExtent(...args),
    ],
    [
      "fold",
      [
        {
          status: "ok",
          changes: {
            changes: [
              {
                path: "docs/a.txt",
                original_path: null,
                status: "added",
                index_status: "added",
                worktree_status: "modified",
              },
            ],
          },
        },
      ],
      (...args) => WBChanges.fold(...args),
    ],
    [
      "foldSync",
      [
        {
          status: "ok",
          sync: {
            sync: {
              head: { kind: "branch", name: "main" },
              tracking: { upstream: "origin/main", ahead: 1, behind: 2 },
              last_fetch: "2026-07-25T11:00:00Z",
            },
          },
        },
        Date.parse("2026-07-25T12:00:00Z"),
      ],
      (...args) => WBChanges.foldSync(...args),
    ],
    [
      "discardConfirm",
      [{ path: "a.txt", name: "a.txt", status: "untracked" }],
      (...args) => WBChanges.discardConfirm(...args),
      () => assert.equal(typeof WBChanges.discardConfirm().message, "string", "no input is answered"),
    ],
    ["dragBegins", [{ x: 1, y: 2 }, { x: 30, y: 40 }, 10], (...args) => WB.dragBegins(...args)],
    [
      "isDoubleTap",
      [{ t: 0, x: 1, y: 2 }, { t: 100, x: 3, y: 4 }],
      (...args) => WB.isDoubleTap(...args),
    ],
  ];
  for (const [name, args, call, check] of rows) {
    const before = structuredClone(args);
    const out = call(...args);
    assert.deepEqual(args, before, `${name} mutated its arguments`);
    if (check) check(out, args);
  }
});
