// Unit tests for assets/ui/wb-console-detach.ts — the opener's side of a
// detached fence. `createDetach` is driven with fake `deps`, a fake
// lifecycle channel and the real popup registry, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDetach } from "../assets/ui/wb-console-detach.ts";
import { createPopupRegistry } from "../assets/ui/wb-console-popups.ts";
import { WBDetachLink } from "../assets/ui/wb-detach-link.ts";

const ORIGIN = "http://127.0.0.1:7800";
const { HEARTBEAT_MS, PEER_WINDOW_MS } = WBDetachLink;

// The page's `WB`: the re-attach announces itself through it.
function withWB(t) {
  const real = globalThis.WB;
  const emitted = [];
  globalThis.WB = { emit: (type, detail) => emitted.push({ type, detail }) };
  t.after(() => {
    globalThis.WB = real;
  });
  return emitted;
}

// One console's detach with the real popup registry. The channel keeps
// what this tab posts in `posts`; the window keeps its listeners in
// `listeners`; the calls into the console are recorded in `calls`.
function harness({ wins = [], desk = [] } = {}) {
  const calls = [];
  const record = (name) => (...args) => calls.push([name, ...args]);
  const posts = [];
  const link = {
    tab: "t1",
    post: (m) => posts.push(m),
    onMessage() {},
    writeRegistry() {},
  };
  const listeners = {};
  const window = {
    addEventListener: (type, fn) => {
      listeners[type] = fn;
    },
  };
  let detach = null;
  const popups = createPopupRegistry({
    link,
    startBeat: () => detach.startBeat(),
    stopBeat: () => detach.stopBeat(),
  });
  detach = createDetach({
    window,
    location: { origin: ORIGIN },
    link,
    popups,
    wins: new Set(wins),
    budget: { untrackDormancy() {} },
    fences: () => [],
    notes: () => [],
    stage: () => null,
    changed() {},
    applyExtent() {},
    forgetRecord: record("forgetRecord"),
    loadDesk: () => desk,
    saveNotes: record("saveNotes"),
    spawnWindow: record("spawnWindow"),
    spawnPlaceholder: (member) => {
      calls.push(["spawnPlaceholder", member.id]);
      return { _revive: record("revive") };
    },
    deskOf: (win) => ({ id: win._deskId }),
    whenDeskLoaded: () => Promise.resolve(),
    fenceFloor: {
      readFenceRects: () => [],
      readWindowRects: () => [],
      refreshFenceChrome() {},
      renderNotes() {},
      showDetachGlyph: record("showDetachGlyph"),
    },
  });
  // Detaches fence `id` with `fields` on its popup entry.
  const detachFence = (id, fields = {}) => {
    const entry = { ...popups.newPopupEntry(), ...fields };
    popups.put(id, entry);
    popups.commitDetached([...popups.detachedIds(), id]);
    return entry;
  };
  return { detach, popups, posts, listeners, calls, detachFence };
}

test("a quiet popup is asked once before its consoles come home", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "Date"] });
  withWB(t);
  const { popups, posts, detachFence } = harness();
  // No handle: this tab reloaded, so only the probe can answer.
  detachFence("f", { pid: "p1" });
  t.mock.timers.tick(PEER_WINDOW_MS + HEARTBEAT_MS);
  assert.deepEqual(
    posts.filter((m) => m.type === "origin-ping"),
    [{ type: "origin-ping", tab: "t1", fenceId: "f" }],
  );
  assert.equal(popups.isDetached("f"), true, "one quiet window is not death");
  assert.equal(posts.some((m) => m.type === "origin-close"), false);
  // The probe goes unanswered for a whole window: now the popup is gone.
  t.mock.timers.tick(PEER_WINDOW_MS + HEARTBEAT_MS);
  assert.equal(popups.isDetached("f"), false);
  assert.deepEqual(posts.filter((m) => m.type === "origin-close"), [
    { type: "origin-close", tab: "t1", fenceId: "f", pid: "p1" },
  ]);
});

test("a re-attach brings the members home, and not one that is already on the stage", (t) => {
  const emitted = withWB(t);
  const { detach, popups, posts, calls, detachFence } = harness({
    wins: [{ _deskId: "w3" }],
    desk: [{ id: "w1", consoleName: "build" }],
  });
  detachFence("f", {
    pid: "p1",
    members: [
      { id: "w1", session: "s1", repo: "r", agent: "claude" },
      { id: "w2", repo: "r" },
      { id: "w3", session: "s3", repo: "r" },
    ],
  });
  detach.reattachFence("f");
  assert.equal(popups.isDetached("f"), false);
  assert.equal(popups.has("f"), false);
  assert.deepEqual(
    calls.filter(([name]) => name !== "showDetachGlyph"),
    [
      [
        "spawnWindow",
        { id: "s1", repo: "r" },
        "claude",
        "r",
        { id: "w1", session: "s1", repo: "r", agent: "claude", consoleName: "build" },
      ],
      ["spawnPlaceholder", "w2"],
      ["revive"],
    ],
  );
  assert.deepEqual(posts.at(-1), { type: "origin-close", tab: "t1", fenceId: "f", pid: "p1" });
  assert.deepEqual(emitted, [{ type: "fence-reattach", detail: { fence: "f" } }]);
});

test("a handshake message from another origin is ignored", (t) => {
  withWB(t);
  const { popups, listeners, detachFence } = harness();
  const handle = { closed: false, close() {}, postMessage() {} };
  detachFence("f", { handle });
  const message = (origin) =>
    listeners.message({ origin, source: handle, data: { type: "wb-fence-reattach" } });
  message("https://elsewhere.example");
  assert.equal(popups.isDetached("f"), true);
  message(ORIGIN);
  assert.equal(popups.isDetached("f"), false);
});

test("a popup's wb-emit reaches WB only when it names a gesture", (t) => {
  const emitted = withWB(t);
  const { listeners, detachFence } = harness();
  const handle = { closed: false, close() {}, postMessage() {} };
  detachFence("f", { handle });
  const post = (data) => listeners.message({ origin: ORIGIN, source: handle, data });
  try {
    post({ type: "wb-emit", action: "file.delete", detail: { repo: "o/r", path: "a" } });
    post({ type: "wb-emit", action: "toString", detail: {} });
    // CONTROL: a gesture name is forwarded with its detail.
    post({ type: "wb-emit", action: "console-open", detail: { repo: "o/r", agent: null, plain: true } });
    assert.deepEqual(emitted, [{ type: "console-open", detail: { repo: "o/r", agent: null, plain: true } }]);
  } finally {
    // Bring the fence home, so the heartbeat of the detach stops.
    post({ type: "wb-fence-reattach" });
  }
});
