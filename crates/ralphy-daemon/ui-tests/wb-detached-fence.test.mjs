// Unit tests for assets/ui/wb-detached-fence.ts, the page script of the
// torn-off fence window, on stub `window` and `document` objects.
import { test } from "node:test";
import assert from "node:assert/strict";
import { isFenceOpen, wireDetachedFence } from "../assets/ui/wb-detached-fence.ts";

const ORIGIN = "https://daemon.test";

// One log of every listener added and every message posted, in order.
function page({ opener = true } = {}) {
  const log = [];
  const stage = { innerHTML: "" };
  const window = {
    location: { origin: ORIGIN },
    opener: opener ? { postMessage: (data, target) => log.push(["post", data, target]) } : null,
    addEventListener: (type) => log.push(["window", type]),
  };
  const document = {
    title: "",
    addEventListener: (type) => log.push(["document", type]),
    getElementById: (id) => (id === "stage" ? stage : null),
  };
  return { window, document, log, stage };
}

// The opener answers "ready" in a later task, so every listener the wire
// function adds is in place before the answer can arrive. The one that
// takes the answer is added before "ready" is posted.
test("the fence window asks its own origin for the fence, with its listeners in place", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page();
  wireDetachedFence(p.window, p.document);
  const ready = p.log.findIndex(([kind, data]) => kind === "post" && data.type === "wb-fence-ready");
  assert.ok(ready >= 0, JSON.stringify(p.log));
  assert.equal(p.log[ready][2], ORIGIN, "the handshake goes to the page's own origin");
  const message = p.log.findIndex(([kind, type]) => kind === "window" && type === "message");
  assert.ok(message >= 0 && message < ready, `the answer's listener is added first: ${JSON.stringify(p.log)}`);
  for (const [kind, type] of [
    ["window", "beforeunload"],
    ["window", "resize"],
    ["document", "workbench:column-restore"],
    ["document", "workbench:columns-stale"],
    ["document", "workbench:consoles-changed"],
  ]) {
    assert.ok(
      p.log.some(([k, ty]) => k === kind && ty === type),
      `${kind} ${type} is added: ${JSON.stringify(p.log)}`,
    );
  }
});

test("the fence window with no opener says so and asks nobody", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const p = page({ opener: false });
  wireDetachedFence(p.window, p.document);
  assert.match(p.stage.innerHTML, /Nothing to show\. Detach a fence from the workbench first\./);
  assert.deepEqual(p.log, []);
});

// The opener can be an older build than this page: its answer is read only in
// the shape this page knows.
test("the opener's answer mounts a fence only when it names one", () => {
  const fence = { id: "f1", name: "Build", rect: { left: 10, top: 20, width: 300, height: 200 } };
  const members = [{ id: "w1", kind: "agent", agent: "claude", repo: "o/r", session: 4, rect: { left: 12, top: 22 } }];
  const ok = { type: "wb-fence-open", fence, members, tab: "t1", pid: "p1" };
  assert.equal(isFenceOpen(ok), true);
  assert.equal(isFenceOpen({ type: "wb-fence-open", fence: { id: "f1", rect: null } }), true, "members, tab and pid may be absent");
  for (const bad of [
    { ...ok, fence: { ...fence, id: 1 } },
    { ...ok, fence: { ...fence, rect: { left: "10" } } },
    { ...ok, members: [{ ...members[0], id: undefined }] },
    { ...ok, members: "w1" },
    { ...ok, tab: 5 },
    { ...ok, type: "wb-detach-open" },
  ]) {
    assert.equal(isFenceOpen(bad), false, JSON.stringify(bad));
  }
});

// The window after the opener's answer, on a stubbed `BroadcastChannel`. `hear`
// delivers a message of the channel; `closes` counts `window.close()`.
function openedFence(t, { tab = "t1", pid = "p1" } = {}) {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval", "Date"] });
  const saved = {};
  const set = (name, value) => {
    saved[name] = globalThis[name];
    globalThis[name] = value;
  };
  const channels = [];
  set("BroadcastChannel", class { constructor() { channels.push(this); } postMessage() {} close() {} });
  set("MutationObserver", class { observe() {} });
  set("WBConsole", { mountDetached() {}, peerFold: (peer) => ({ state: peer, effects: [] }), list: () => [] });
  t.after(() => {
    for (const [name, value] of Object.entries(saved)) globalThis[name] = value;
  });
  const handlers = {};
  const opener = { postMessage() {}, closed: false };
  const stage = { innerHTML: "", querySelector: () => null, querySelectorAll: () => [] };
  const tool = () => ({ append() {}, addEventListener() {} });
  const closes = { count: 0, focus: 0 };
  const window = {
    location: { origin: ORIGIN },
    opener,
    addEventListener: (type, fn) => (handlers[type] = fn),
    close: () => closes.count++,
    focus: () => closes.focus++,
  };
  const document = {
    title: "",
    body: { append() {} },
    createElement: tool,
    addEventListener() {},
    getElementById: () => stage,
  };
  wireDetachedFence(window, document);
  handlers.message({
    origin: ORIGIN,
    source: opener,
    data: { type: "wb-fence-open", fence: { id: "f1", name: "Build" }, members: [], tab, pid },
  });
  const hear = (data) => channels[0].onmessage({ data });
  return { hear, closes };
}

test("origin-close for another window of the fence keeps this one, and for this one closes it", (t) => {
  const w = openedFence(t);
  w.hear({ type: "origin-close", tab: "t1", fenceId: "f1", pid: "other" });
  assert.equal(w.closes.count, 0, "another window's order");
  w.hear({ type: "origin-close", tab: "t1", fenceId: "f2", pid: "p1" });
  assert.equal(w.closes.count, 0, "another fence's order");
  w.hear({ type: "origin-close", tab: "t2", fenceId: "f1", pid: "p1" });
  assert.equal(w.closes.count, 0, "another tab's order");
  w.hear({ type: "origin-close", tab: "t1", fenceId: "f1", pid: "p1" });
  assert.equal(w.closes.count, 1);
});

test("an order with no pid is for this window, and origin-focus follows the same rule", (t) => {
  const w = openedFence(t);
  w.hear({ type: "origin-focus", tab: "t1", fenceId: "f1", pid: "other" });
  assert.equal(w.closes.focus, 0);
  w.hear({ type: "origin-focus", tab: "t1", fenceId: "f1" });
  assert.equal(w.closes.focus, 1, "an opener that has not adopted this window yet");
  w.hear({ type: "origin-close", tab: "t1", fenceId: "f1" });
  assert.equal(w.closes.count, 1);
});
