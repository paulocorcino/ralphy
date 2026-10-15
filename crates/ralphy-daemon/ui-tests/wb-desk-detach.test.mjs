// Unit tests for assets/ui/wb-desk-detach.ts — the opener's side of a
// detached fence. `createDetach` is driven with fake `deps`, a fake
// lifecycle channel and the real popup registry, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDetach, isFencePopupMessage } from "../assets/ui/wb-desk-detach.ts";
import { createPopupRegistry } from "../assets/ui/wb-desk-popups.ts";
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
// what this tab posts in `posts` and hands `hear` the message a popup sends;
// the window keeps its listeners in `listeners`; the calls into the console
// are recorded in `calls`.
function harness({ wins = [], desk = [], notes = [] } = {}) {
  const calls = [];
  const record = (name) => (...args) => calls.push([name, ...args]);
  const posts = [];
  const heard = [];
  const link = {
    tab: "t1",
    post: (m) => posts.push(m),
    onMessage: (fn) => heard.push(fn),
    writeRegistry() {},
  };
  const hear = (m) => heard.forEach((fn) => fn(m));
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
    notes: () => notes,
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
  return { detach, popups, posts, listeners, calls, detachFence, hear };
}

// ---- the lifecycle channel: what a popup of this tab says ----------------------

test("a channel message from another tab changes nothing here", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  withWB(t);
  const { popups, hear, detachFence } = harness();
  detachFence("f", { pid: "p1" });
  hear({ type: "popup-gone", tab: "t2", fenceId: "f", pid: "p1" });
  hear({ type: "popup-gone", fenceId: "f", pid: "p1" });
  assert.equal(popups.isDetached("f"), true, "the channel is browser-wide: another tab's popup is not ours");
  // CONTROL: the same message from this tab brings the fence home.
  hear({ type: "popup-gone", tab: "t1", fenceId: "f", pid: "p1" });
  assert.equal(popups.isDetached("f"), false);
});

test("an earlier popup of the fence is not heard, but its ping is answered", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  withWB(t);
  const { popups, posts, calls, hear, detachFence } = harness();
  const entry = detachFence("f", { pid: "p2", members: [{ id: "w1", session: "s1" }], memberIds: ["w1"] });
  const stale = { tab: "t1", fenceId: "f", pid: "p1" };
  hear({ ...stale, type: "popup-members", members: [] });
  hear({ ...stale, type: "popup-here", members: [] });
  hear({ ...stale, type: "popup-gone" });
  assert.equal(popups.isDetached("f"), true, "the stale popup-gone would re-attach the popup that replaced it");
  assert.equal(popups.entry("f"), entry);
  assert.deepEqual(entry.members, [{ id: "w1", session: "s1" }]);
  assert.deepEqual(calls.filter(([name]) => name === "forgetRecord"), [], "a stale member list drops nothing");
  hear({ ...stale, type: "popup-ping" });
  assert.deepEqual(posts.at(-1), { type: "origin-here", tab: "t1", fenceId: "f" });
  // CONTROL: the popup this entry holds is heard.
  hear({ ...stale, pid: "p2", type: "popup-gone" });
  assert.equal(popups.isDetached("f"), false);
});

test("popup-here adopts the members of a detached fence into the same entry", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  withWB(t);
  const notes = [{ id: "n1", rect: {} }];
  const { popups, calls, hear, detachFence } = harness({ notes });
  // A restored entry: no handle and no pid, only the member ids it stored.
  const entry = detachFence("f", { memberIds: ["w1", "w2"] });
  const members = [
    { id: "w1", session: "s1" },
    { id: "n1", kind: "note", path: "notes/a.note" },
  ];
  hear({ type: "popup-here", tab: "t1", fenceId: "f", pid: "p9", members });
  assert.equal(popups.entry("f"), entry, "the entry is changed in place, never replaced");
  assert.equal(entry.pid, "p9");
  assert.deepEqual(entry.members, members);
  assert.deepEqual(entry.memberIds, ["w1", "n1"]);
  assert.equal(entry.greeted, true);
  assert.deepEqual(entry.fence, { id: "f", name: "", rect: null });
  assert.deepEqual(calls.filter(([name]) => name === "forgetRecord"), [["forgetRecord", "w2"]]);
  assert.deepEqual(calls.filter(([name]) => name === "showDetachGlyph"), [["showDetachGlyph", "f", true]]);
  // The name the popup gave its note is recorded once the desk has loaded.
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(calls.filter(([name]) => name === "saveNotes"), [
    ["saveNotes", [{ id: "n1", rect: {}, path: "notes/a.note" }]],
  ]);
  // A fence the registry does not hold is never detached by a popup-here.
  hear({ type: "popup-here", tab: "t1", fenceId: "g", pid: "p3", members: [] });
  assert.equal(popups.has("g"), false);
  assert.equal(popups.isDetached("g"), false);
});

test("a beat from the popup makes it heard now, and clears the probe", (t) => {
  t.mock.timers.enable({ apis: ["setInterval", "Date"] });
  withWB(t);
  const { hear, detachFence } = harness();
  const entry = detachFence("f", { pid: "p1", probed: true, peer: { seen: 0, lost: false } });
  t.mock.timers.setTime(5000);
  hear({ type: "popup-beat", tab: "t1", fenceId: "f", pid: "p1" });
  assert.deepEqual(entry.peer, { seen: 5000, lost: false });
  assert.equal(entry.probed, false, "a beat answers the probe");
});

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

// The daemon serves the popup's page when it opens, so a popup can be a newer
// build than this tab: a message is read only in the shape this tab knows.
test("a fence popup's message is read only in its own shape", () => {
  for (const ok of [
    { type: "wb-fence-ready" },
    { type: "wb-emit", action: "save", detail: {} },
    { type: "wb-note-named", noteId: "n1", path: "notes/a.md" },
    { type: "wb-note-claimed", noteId: "n1", claim: "notes/b.md" },
    { type: "wb-fence-reattach" },
  ]) {
    assert.equal(isFencePopupMessage(ok), true, JSON.stringify(ok));
  }
  for (const bad of [
    { type: "wb-note-named", noteId: 1, path: "notes/a.md" },
    { type: "wb-note-claimed", noteId: "n1", claim: ["notes/b.md"] },
    { type: "wb-detach-ready" },
    "wb-fence-ready",
  ]) {
    assert.equal(isFencePopupMessage(bad), false, JSON.stringify(bad));
  }
});
