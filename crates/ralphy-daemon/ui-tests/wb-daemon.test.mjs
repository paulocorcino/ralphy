// Unit tests for assets/ui/wb-daemon.ts — runs the real module with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createDaemon } from "../assets/ui/wb-daemon.ts";
import { ReadDeadline } from "../assets/ui/wb-api.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";
import { CONNECT_TIMEOUT_MS, RESUME_DEBOUNCE_MS, resumeDecision } from "../assets/ui/wb-resume.ts";

const HERE = dirname(fileURLToPath(import.meta.url));

// A push the daemon produced, as its tests wrote it (`tests/support/golden.rs`).
function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

function load() {
  // The two globals the module touches at LOAD time: `document.addEventListener`
  // (it subscribes to the `workbench:action` seam) and `location.protocol`/`host`
  // (WS_ORIGIN). No `WebSocket` is injected: nothing opens a socket at load, so
  // a constructor call moved to module scope fails LOUDLY here.
  const window = { addEventListener() {} };
  const document = { addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  return createDaemon(window, document, location, { messages: { flash() {} } });
}

test("a handshake that never opens is closed at the deadline, and nothing else is", async () => {
  const realSetTimeout = globalThis.setTimeout;
  const timers = [];
  globalThis.setTimeout = (fn, ms) => timers.push({ fn, ms });
  const sockets = [];
  globalThis.WebSocket = class {
    constructor() {
      this.readyState = 0;
      this.closes = 0;
      sockets.push(this);
    }
    close() {
      this.closes += 1;
    }
  };
  // Two pages: the socket of the second opens in time.
  try {
    for (const d of [load(), load()]) {
      d.subscribePresence(() => {});
      d.subscribeRuns("r", () => {});
      d.subscribeChanges("r", () => {});
    }
  } finally {
    globalThis.setTimeout = realSetTimeout;
    delete globalThis.WebSocket;
  }
  assert.equal(sockets.length, 2, "one socket per page");
  const deadlines = timers.filter((t) => t.ms === CONNECT_TIMEOUT_MS);
  assert.equal(deadlines.length, 2, "one handshake deadline per page socket");
  sockets[1].readyState = 1;
  for (const t of deadlines) t.fn();
  assert.deepEqual(
    sockets.map((s) => s.closes),
    [1, 0],
  );
});

// The shell's door and the console run one resume rule, because they resume on
// the same event; the rule lives in wb-console-session.ts and the door imports it.
test("the daemon door uses the session folds' resume rule and deadlines", () => {
  const door = load();
  assert.equal(door.resumeDecision, resumeDecision);
  assert.equal(door.CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_MS);
  assert.equal(door.RESUME_DEBOUNCE_MS, RESUME_DEBOUNCE_MS);
});

// --- detachSocket: retiring a socket so its queued events cannot reach us ---

test("detachSocket nulls EVERY handler, onmessage included, then closes", () => {
  const { detachSocket } = load();
  const ws = {
    readyState: 1,
    closed: false,
    onopen: () => {},
    onmessage: () => {},
    onerror: () => {},
    onclose: () => {},
    close() {
      this.closed = true;
    },
  };
  detachSocket(ws);
  // `onmessage` matters as much as `onclose`: a `session-end` still queued on
  // the outgoing socket would land after the replacement is wired and be read
  // as the NEW connection's news.
  for (const h of ["onopen", "onmessage", "onerror", "onclose"]) {
    assert.equal(ws[h], null, `${h} must be detached before the close`);
  }
  assert.equal(ws.closed, true);
});

test("detachSocket does not call close on an already-closed socket", () => {
  const { detachSocket } = load();
  for (const readyState of [2, 3]) {
    let calls = 0;
    detachSocket({ readyState, close: () => (calls += 1) });
    assert.equal(calls, 0, `readyState ${readyState} is already past closing`);
  }
});

test("detachSocket tolerates no socket and a throwing close", () => {
  const { detachSocket } = load();
  // Both run on a resume path that must never throw into the operator's face.
  assert.doesNotThrow(() => detachSocket(null));
  assert.doesNotThrow(() => detachSocket(undefined));
  assert.doesNotThrow(() =>
    detachSocket({
      readyState: 1,
      close() {
        throw new Error("InvalidStateError");
      },
    }),
  );
});

// --- withCheckout: the optional `checkout` argument on a verb payload --------

test("withCheckout adds the key only for a real name", () => {
  const { withCheckout } = load();
  assert.deepEqual(withCheckout({ repo: "r", path: "" }, "wt-a"), {
    repo: "r",
    path: "",
    checkout: "wt-a",
  });
  // NEGATIVE CONTROL: with no selection the payload is byte-identical to the
  // pre-#406 one — no `checkout: null` key, so an older daemon never sees it.
  for (const none of [null, "", undefined]) {
    const out = withCheckout({ repo: "r", path: "" }, none);
    assert.deepEqual(out, { repo: "r", path: "" });
    assert.ok(!("checkout" in out), `no key for ${String(none)}`);
  }
  // The input is copied, never mutated.
  const input = { repo: "r", path: "x" };
  const out = withCheckout(input, "wt-a");
  assert.ok(!("checkout" in input));
  assert.notEqual(out, input);
  assert.deepEqual(withCheckout(null, "wt-a"), { checkout: "wt-a" });
});

// --- observe: a command with one reply goes over `POST /api/command` ----------

// Let every pending promise callback run; setImmediate is not under mock timers.
const flush = () => new Promise((r) => setImmediate(r));

function newDaemon() {
  const window = { addEventListener() {} };
  const document = { addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  return createDaemon(window, document, location, { messages: { flash() {} } });
}

// A fake `fetch` for the command route. `answer(cmd)` gives the reply payload,
// or a `Response` to send as it is. `calls` lists each request. No WebSocket is
// injected, so a socket opened by mistake fails loudly.
function loadWithFetch(answer) {
  const calls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (url, init) => {
    const cmd = JSON.parse(init.body);
    calls.push({ url, method: init.method, headers: init.headers, cmd });
    const reply = answer(cmd);
    return Promise.resolve(reply instanceof Response ? reply : Response.json(reply));
  };
  return { d: newDaemon(), calls, restore: () => (globalThis.fetch = realFetch) };
}

test("observe sends its command over HTTP and opens no socket", async () => {
  const { d, calls, restore } = loadWithFetch(() => ({ status: "ok", entries: [] }));
  let sockets = 0;
  globalThis.WebSocket = class {
    constructor() {
      sockets += 1;
    }
  };
  try {
    const reply = await d.observe("tree.list", { repo: "o/r", path: "" });
    assert.deepEqual(reply, { status: "ok", entries: [] });
    assert.equal(sockets, 0);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "/api/command");
    assert.equal(calls[0].method, "POST");
    // The body the daemon reads (`fixtures/api-command--request.json`).
    assert.deepEqual(Object.keys(calls[0].cmd).sort(), Object.keys(fixture("api-command--request")).sort());
    assert.equal(calls[0].cmd.verb, "tree.list");
    assert.deepEqual(calls[0].cmd.payload, { repo: "o/r", path: "" });
  } finally {
    delete globalThis.WebSocket;
    restore();
  }
});

test("observe fans out an unknown checkout to the registered listeners, after the reply", async () => {
  const { d, restore } = loadWithFetch((cmd) => {
    if (cmd.payload.checkout === "gone") return { status: "error", message: "unknown checkout" };
    if (cmd.payload.checkout === "missing-file") return { status: "error", reason: "not found" };
    return { status: "ok", entries: [] };
  });
  try {
    const seen = [];
    d.onUnknownCheckout((repo, name) => seen.push([repo, name]));
    // A listener that throws must never break the read or the others.
    d.onUnknownCheckout(() => {
      throw new Error("boom");
    });
    const reply = await d.observe("tree.list", { repo: "o/r", path: "", checkout: "gone" });
    assert.equal(reply.message, "unknown checkout", "the reply still resolves");
    assert.deepEqual(seen, [], "the fan-out lands AFTER the resolve, not before it");
    await new Promise((r) => setTimeout(r, 5));
    assert.deepEqual(seen, [["o/r", "gone"]]);

    // NEGATIVE CONTROLS: another error, an ok reply, and a payload with no
    // checkout at all fire nothing.
    await d.observe("file.read", { repo: "o/r", path: "x", checkout: "missing-file" });
    await d.observe("tree.list", { repo: "o/r", path: "", checkout: "wt-a" });
    await d.observe("tree.list", { repo: "o/r", path: "" });
    await new Promise((r) => setTimeout(r, 5));
    assert.deepEqual(seen, [["o/r", "gone"]]);
  } finally {
    restore();
  }
});

test("observe rejects a command the daemon answers with an HTTP error", async () => {
  const { d, restore } = loadWithFetch(() => new Response("boom", { status: 500 }));
  try {
    await assert.rejects(d.observe("tree.list", { repo: "r", path: "" }), /HTTP 500/);
  } finally {
    restore();
  }
});

test("write and readImage send their command over HTTP", async () => {
  const { d, calls, restore } = loadWithFetch((cmd) =>
    cmd.verb === "file.image" ? { status: "ok", mediaType: "image/png", base64: "AAAA" } : { status: "ok" },
  );
  try {
    assert.deepEqual(await d.write("file.write", { repo: "r", path: "a.txt", content: "x" }), { status: "ok" });
    assert.equal(await d.readImage("r", "a.png", undefined, null), "data:image/png;base64,AAAA");
    assert.deepEqual(
      calls.map((c) => [c.url, c.method, c.cmd.verb]),
      [
        ["/api/command", "POST", "file.write"],
        ["/api/command", "POST", "file.image"],
      ],
    );
  } finally {
    restore();
  }
});

// --- observe: a command that gets no answer fails at its deadline ------------

// A `fetch` that never answers the command route; `settle(reply)` answers the
// pending request late. The timers are mocked, so a tick drives the deadline.
// Any other route (the stall report) is answered `204` at once.
function silentFetch({ checkout }) {
  const realFetch = globalThis.fetch;
  const pending = [];
  globalThis.fetch = (url) =>
    url === "/api/command"
      ? new Promise((resolve) => pending.push(resolve))
      : Promise.resolve(new Response(null, { status: 204 }));
  mock.timers.enable({ apis: ["setTimeout"] });
  const d = newDaemon();
  const result = { error: null, value: undefined };
  const p = d.observe("tree.list", { repo: "r", path: "", checkout }).then(
    (v) => (result.value = v),
    (e) => (result.error = e),
  );
  return {
    d,
    result,
    p,
    settle: (reply) => pending[0](Response.json(reply)),
    restore: () => {
      mock.timers.reset();
      globalThis.fetch = realFetch;
    },
  };
}

test("observe rejects a command with no answer at the command deadline", async () => {
  const s = silentFetch({});
  try {
    mock.timers.tick(69999);
    await flush();
    assert.equal(s.result.error, null, "still waiting at 69999 ms");
    mock.timers.tick(1);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    assert.equal(s.result.error.what, "tree.list");
    assert.equal(s.result.error.waitedMs, 70000);
  } finally {
    s.restore();
  }
});

test("a reply after the deadline neither resolves the read nor fans out", async () => {
  const s = silentFetch({ checkout: "gone" });
  try {
    let calls = 0;
    s.d.onUnknownCheckout(() => calls++);
    mock.timers.tick(70000);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    s.settle({ status: "error", message: "unknown checkout" });
    mock.timers.tick(10);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline, "still rejected");
    assert.equal(s.result.value, undefined);
    assert.equal(calls, 0);
  } finally {
    s.restore();
  }
});

// --- the page socket: one `/ws` socket, a channel per subscription ---------

// A frame the page sent, read: a bare `[0x02][JSON]` frame is channel 0, and a
// `[0x04][channel u32][flags][0x02][JSON]` frame is on its channel. Every frame
// here is small, so a frame is one piece.
function readSent(bytes) {
  if (bytes[0] === 0x04) {
    const channel = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(1);
    return { channel, ...JSON.parse(new TextDecoder().decode(bytes.subarray(7))) };
  }
  return { channel: 0, ...JSON.parse(new TextDecoder().decode(bytes.subarray(1))) };
}

function commandBytes(obj) {
  const body = new TextEncoder().encode(JSON.stringify(obj));
  const out = new Uint8Array(1 + body.length);
  out[0] = 0x02;
  out.set(body, 1);
  return out;
}

// The one channel frame that carries `inner` on `channel`.
function onChannel(channel, inner) {
  const out = new Uint8Array(6 + inner.length);
  out[0] = 0x04;
  new DataView(out.buffer).setUint32(1, channel);
  out.set(inner, 6);
  return out;
}

// The id the page gave its channel of `kind` on socket `s`.
function channelOf(s, kind) {
  const opens = s.sent.filter((f) => f.channel === 0 && f.verb === "channel.open" && f.payload.kind === kind);
  assert.equal(opens.length, 1, `one ${kind} channel on the socket`);
  return opens[0].payload.channel;
}

// The verbs sent on the channel of `kind`, with their path.
const sentOn = (s, kind = "tree") => {
  const id = channelOf(s, kind);
  return s.sent.filter((f) => f.channel === id).map((f) => [f.verb, f.payload.path]);
};

// The daemon door with fake sockets and timers. Timers are captured, never
// run: the handshake deadline and the 3s retry are fired by hand, so no test
// waits on a real clock.
function pageSockets() {
  const d = load();
  const sockets = [];
  const timers = [];
  const FakeSocket = class {
    constructor(url) {
      this.url = url;
      this.sent = [];
      this.readyState = 0;
      sockets.push(this);
    }
    send(bytes) {
      this.sent.push(readSent(bytes));
    }
    close() {
      this.readyState = 3;
    }
  };
  // Every socket is created with the fakes in place: a subscription, and a
  // reconnect.
  const withFakes = (fn) => {
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    globalThis.setTimeout = (callback, ms) => timers.push({ fn: callback, ms }) - 1;
    globalThis.clearTimeout = (id) => {
      if (timers[id]) timers[id].fn = () => {};
    };
    globalThis.WebSocket = FakeSocket;
    try {
      return fn();
    } finally {
      globalThis.setTimeout = realSetTimeout;
      globalThis.clearTimeout = realClearTimeout;
      delete globalThis.WebSocket;
    }
  };
  const open = (s) => {
    s.readyState = 1;
    s.onopen();
  };
  // A push of the daemon: bare on channel 0, or on the channel of `kind`.
  const push = (s, kind, verb, payload) => {
    const inner = commandBytes({ id: 0, verb, payload });
    const bytes = kind ? onChannel(channelOf(s, kind), inner) : inner;
    s.onmessage({ data: bytes.buffer });
  };
  // Drop the socket the way the browser does, then run whatever retry it armed.
  const drop = (s) =>
    withFakes(() => {
      s.readyState = 3;
      s.onclose?.();
    });
  const runRetries = () =>
    withFakes(() => {
      for (const t of timers.splice(0)) if (t.ms === 3000) t.fn();
    });
  return { d, sockets, timers, open, push, drop, runRetries, withFakes };
}

function treeSocket(checkout, onHead, onFailed) {
  const page = pageSockets();
  const dirty = [];
  const sub = page.withFakes(() =>
    page.d.subscribeTree("o/r", (rel) => dirty.push(rel), checkout, onHead, onFailed),
  );
  const ws = page.sockets[0];
  page.open(ws);
  const push = (verb, payload) => page.push(ws, "tree", verb, payload);
  return { ...page, sub, ws, dirty, push };
}

test("one socket carries presence, tree, runs and changes", () => {
  const { d, sockets, open, push, withFakes } = pageSockets();
  const beats = [];
  const runs = [];
  const changes = [];
  withFakes(() => {
    d.subscribePresence((p) => beats.push(p));
    d.subscribeTree("o/r", () => {}, null, undefined, undefined);
    d.subscribeRuns("o/r", () => runs.push("dirty"));
    d.subscribeChanges("o/r", (f) => changes.push(f.verb));
  });
  assert.equal(sockets.length, 1, "one WebSocket in total");
  assert.equal(sockets[0].url, "ws://127.0.0.1:7431/ws");
  open(sockets[0]);
  const ids = ["tree", "runs", "changes"].map((kind) => channelOf(sockets[0], kind));
  assert.equal(new Set(ids).size, 3, "a channel each");
  assert.deepEqual(sentOn(sockets[0], "runs"), [["runs.watch", ""]]);
  const beat = new Uint8Array([0x03, ...new TextEncoder().encode(JSON.stringify({ uptime_secs: 2 }))]);
  sockets[0].onmessage({ data: beat.buffer });
  push(sockets[0], "runs", "runs.dirty", { repo: "o/r" });
  push(sockets[0], "changes", "changes.dirty", { repo: "o/r" });
  assert.deepEqual(beats, [{ uptime_secs: 2 }]);
  assert.deepEqual(runs, ["dirty"]);
  assert.deepEqual(changes, ["changes.dirty"]);
});

test("a reopened socket re-sends the watches and reads once", () => {
  const { d, sockets, open, drop, runRetries, withFakes } = pageSockets();
  const dirty = [];
  let runReads = 0;
  const sub = withFakes(() => {
    d.subscribePresence(() => {});
    d.subscribeRuns("o/r", () => (runReads += 1));
    return d.subscribeTree("o/r", (rel) => dirty.push(rel), null, undefined, undefined);
  });
  open(sockets[0]);
  sub.watch("");
  sub.watch("src");
  assert.deepEqual(dirty, []);
  drop(sockets[0]);
  runRetries();
  assert.equal(sockets.length, 2, "one socket comes back, not one per subscription");
  open(sockets[1]);
  assert.deepEqual(sentOn(sockets[1]), [
    ["watch", ""],
    ["watch", "src"],
  ]);
  assert.deepEqual(sentOn(sockets[1], "runs"), [["runs.watch", ""]]);
  assert.deepEqual(dirty, ["", "src"], "each held dir is read once");
  assert.equal(runReads, 1);
});

test("a tree push on its channel reaches onDirty", () => {
  const { d, sockets, open, push, withFakes } = pageSockets();
  const dirty = [];
  withFakes(() => {
    d.subscribeRuns("o/r", () => {});
    d.subscribeTree("o/r", (rel) => dirty.push(rel), null, undefined, undefined);
  });
  open(sockets[0]);
  push(sockets[0], "tree", "tree.dirty", { repo: "o/r", path: "src" });
  assert.deepEqual(dirty, ["src"]);
  // NEGATIVE CONTROLS: the same push bare on channel 0, or on another
  // channel, is not the tree's.
  push(sockets[0], null, "tree.dirty", { repo: "o/r", path: "lib" });
  push(sockets[0], "runs", "tree.dirty", { repo: "o/r", path: "lib" });
  assert.deepEqual(dirty, ["src"]);
});

test("the page socket keeps the time its handshake took", () => {
  const { d, sockets, open, withFakes } = pageSockets();
  const realNow = Date.now;
  let now = 1000;
  Date.now = () => now;
  try {
    assert.equal(d.pageHandshakeMs(), null, "no socket opened yet");
    withFakes(() => d.subscribePresence(() => {}));
    now = 1240;
    open(sockets[0]);
  } finally {
    Date.now = realNow;
  }
  assert.equal(d.pageHandshakeMs(), 240);
});


test("subscribeTree holds the checkout's HEAD and routes head.dirty to onHead", () => {
  let heads = 0;
  const { ws, dirty, push } = treeSocket("wt-a", () => (heads += 1));
  const tree = channelOf(ws, "tree");
  assert.deepEqual(
    ws.sent.filter((f) => f.channel === tree).map((f) => [f.verb, f.payload]),
    [["head.watch", { repo: "o/r", path: "", checkout: "wt-a" }]],
  );
  push("head.dirty", { repo: "o/r", checkout: "wt-a" });
  assert.equal(heads, 1);
  assert.deepEqual(dirty, [], "a HEAD move is not a tree change");
  // NEGATIVE CONTROLS: another checkout's HEAD, and the primary's, are not ours.
  push("head.dirty", { repo: "o/r", checkout: "wt-b" });
  push("head.dirty", { repo: "o/r" });
  assert.equal(heads, 1);
  push("tree.dirty", { repo: "o/r", path: "src", checkout: "wt-a" });
  assert.deepEqual(dirty, ["src"]);
});

test("subscribeTree without onHead sends no head.watch", () => {
  const { ws, push } = treeSocket(null, undefined);
  assert.deepEqual(sentOn(ws), []);
  assert.doesNotThrow(() => push("head.dirty", { repo: "o/r" }));
});

test("subscribeTree routes tree.failed to onFailed, and a reopen clears it", () => {
  const failed = [];
  const { dirty, sockets, push, open, drop, runRetries, ws } = treeSocket(null, undefined, (r) =>
    failed.push(r),
  );
  const pushed = fixture("tree.failed");
  push("tree.failed", pushed);
  assert.deepEqual(failed, [pushed.reason]);
  assert.deepEqual(dirty, [], "a failed watch is not a tree change");
  // NEGATIVE CONTROL: another checkout's failure is not this tree's.
  push("tree.failed", { ...pushed, checkout: "other" });
  assert.deepEqual(failed, [pushed.reason]);
  drop(ws);
  runRetries();
  open(sockets[1]);
  assert.deepEqual(failed, [pushed.reason, null], "a new socket holds every dir again");
});

// --- subscribeTree: a lost socket comes back holding the same dirs ----------
// A new socket starts empty on the daemon, and nothing was pushed while the old
// one was down: the reopen must re-send the holds and re-read what they cover.

test("the tree socket reopens after a drop and holds the same dirs again", () => {
  const { sub, ws, sockets, open, drop, runRetries } = treeSocket(null, () => {});
  sub.watch("");
  sub.watch("src");
  sub.watch("src"); // held already: not sent twice
  sub.watch("docs");
  sub.unwatch("docs");
  assert.deepEqual(sentOn(ws), [
    ["head.watch", ""],
    ["watch", ""],
    ["watch", "src"],
    ["watch", "docs"],
    ["unwatch", "docs"],
  ]);
  drop(ws);
  // While disconnected only the held set changes; nothing is sent.
  sub.unwatch("src");
  sub.watch("lib");
  assert.equal(sentOn(ws).length, 5);
  assert.equal(sockets.length, 1, "the retry waits for its timer");
  runRetries();
  assert.equal(sockets.length, 2);
  open(sockets[1]);
  assert.deepEqual(sentOn(sockets[1]), [
    ["head.watch", ""],
    ["watch", ""],
    ["watch", "lib"],
  ]);
});

test("a reopen re-reads each held dir and the branch once; the first open reads nothing", () => {
  let heads = 0;
  const { sub, ws, sockets, dirty, open, drop, runRetries } = treeSocket(null, () => (heads += 1));
  sub.watch("");
  sub.watch("src");
  assert.deepEqual(dirty, []);
  assert.equal(heads, 0);
  drop(ws);
  runRetries();
  open(sockets[1]);
  assert.deepEqual(dirty, ["", "src"]);
  assert.equal(heads, 1);
});

test("a closed tree subscription never reopens", () => {
  const { sub, ws, sockets, drop, runRetries } = treeSocket(null, undefined);
  sub.watch("");
  sub.close();
  drop(ws);
  runRetries();
  assert.equal(sockets.length, 1);
});

test("resume(true) replaces an open tree socket and the new one replays the held set", () => {
  const { sub, ws, sockets, open, withFakes } = treeSocket(null, undefined);
  sub.watch("src");
  assert.equal(
    withFakes(() => sub.resume(true)),
    true,
  );
  assert.equal(sockets.length, 2);
  assert.equal(ws.onclose, null, "the retired socket can no longer schedule a retry");
  open(sockets[1]);
  assert.deepEqual(sentOn(sockets[1]), [["watch", "src"]]);
});

// --- subscribePresence: the presence socket also carries the daemon's pushes

test("subscribePresence hands a command frame to onPush and each open to onOpen", () => {
  const d = load();
  const sockets = [];
  const realSetTimeout = globalThis.setTimeout;
  globalThis.setTimeout = () => 0;
  globalThis.WebSocket = class {
    constructor() {
      sockets.push(this);
    }
    close() {}
  };
  const pushes = [];
  const opens = [];
  const beats = [];
  try {
    d.subscribePresence((p) => beats.push(p), {
      onPush: (verb, payload) => pushes.push([verb, payload]),
      onOpen: (reopened) => opens.push(reopened),
    });
  } finally {
    globalThis.setTimeout = realSetTimeout;
    delete globalThis.WebSocket;
  }
  const ws = sockets[0];
  ws.onopen();
  const frame = (tag, obj) => {
    const body = new TextEncoder().encode(JSON.stringify(obj));
    const out = new Uint8Array(1 + body.length);
    out[0] = tag;
    out.set(body, 1);
    return { data: out.buffer };
  };
  ws.onmessage(frame(0x02, { id: 0, verb: "sessions.dirty", payload: {} }));
  ws.onmessage(frame(0x02, { id: 0, verb: "desk.dirty", payload: { tab: "t1" } }));
  ws.onmessage(frame(0x03, { uptime_secs: 4 }));
  assert.deepEqual(pushes, [
    ["sessions.dirty", {}],
    ["desk.dirty", { tab: "t1" }],
  ]);
  assert.deepEqual(beats, [{ uptime_secs: 4 }], "a push is not a heartbeat");
  assert.deepEqual(opens, [false]);
});

test("spawn runs on a channel and opens no socket", () => {
  const { d, sockets, open, push, withFakes } = pageSockets();
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  const seen = [];
  const id = withFakes(() =>
    d.spawn("run", { repo: "o/r", agent: "claude" }, (st) => seen.push(st)),
  );
  assert.equal(sockets.length, 1, "the command opens no WebSocket of its own");
  const opens = sockets[0].sent.filter((f) => f.verb === "channel.open");
  assert.equal(opens.length, 1);
  assert.equal(opens[0].channel, 0);
  assert.equal(opens[0].payload.kind, "spawn");
  assert.deepEqual(opens[0].payload.command, {
    id,
    verb: "run",
    payload: { repo: "o/r", agent: "claude" },
  });
  push(sockets[0], "spawn", "run", { status: "spawned", pid: 7 });
  push(sockets[0], "spawn", "run", { status: "output", chunk: "hello" });
  push(sockets[0], "spawn", "run", { status: "exited", code: 0 });
  assert.deepEqual(
    seen.map((s) => s.status),
    ["spawned", "output", "exited"],
  );
  assert.equal(seen[1].chunk, "hello");
  // The run ended with `exited`: a late frame on the channel is not heard,
  // and the verb is not sent again.
  push(sockets[0], "spawn", "run", { status: "output", chunk: "late" });
  assert.equal(seen.length, 3);
  // NEGATIVE CONTROL: the same frame bare on channel 0 is not the command's.
  const other = withFakes(() => d.spawn("run", { repo: "o/r" }, (st) => seen.push(st)));
  assert.notEqual(other, id);
  push(sockets[0], null, "run", { status: "output", chunk: "bare" });
  assert.equal(seen.length, 3);
});

test("a command whose socket was lost is not run again on the next socket", () => {
  const { d, sockets, open, drop, runRetries, withFakes } = pageSockets();
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  const seen = [];
  withFakes(() => d.spawn("run", { repo: "o/r" }, (st) => seen.push(st.status)));
  drop(sockets[0]);
  runRetries();
  open(sockets[1]);
  const opens = sockets[1].sent.filter((f) => f.verb === "channel.open" && f.payload.kind === "spawn");
  assert.deepEqual(opens, [], "a new socket does not run the verb again");
  assert.deepEqual(seen, []);
});

test("an observe past its deadline reports a stall naming the verb", async () => {
  const stalls = [];
  forgetStalls();
  const s = silentFetch({});
  const silent = globalThis.fetch;
  globalThis.fetch = (url, init) => {
    if (url !== "/api/device/stall") return silent(url, init);
    stalls.push({ url, ...JSON.parse(init.body) });
    return Promise.resolve(new Response(null, { status: 204 }));
  };
  try {
    mock.timers.tick(70000);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    assert.deepEqual(
      stalls.map((r) => [r.url, r.kind, r.what]),
      [["/api/device/stall", "read-deadline", "tree.list"]],
    );
  } finally {
    s.restore();
  }
});

// ADR-0082 D3: a console is a `console` channel of the page socket. Its open
// waits for the socket, and carries the payload the console built.
test("openConsole opens a console channel with its payload once the page socket opens", () => {
  const { d, sockets, open, withFakes } = pageSockets();
  const frames = [];
  const channel = withFakes(() =>
    d.openConsole({ id: 5, since: 40, holder: "h1" }, {
      onFrame: (inner) => frames.push([...inner]),
      onClosed: () => assert.fail("the channel is not closed"),
    }),
  );
  assert.equal(sockets.length, 1, "the console opens the page socket, no socket of its own");
  assert.equal(sockets[0].url, "ws://127.0.0.1:7431/ws");
  assert.equal(channel.readyState, 0, "the open waits for the page socket");
  open(sockets[0]);
  const opens = sockets[0].sent.filter((f) => f.verb === "channel.open");
  assert.equal(opens.length, 1);
  const { channel: id, kind, ...payload } = opens[0].payload;
  assert.equal(kind, "console");
  assert.deepEqual(payload, { id: 5, since: 40, holder: "h1" });
  assert.equal(channel.readyState, 1);
  sockets[0].onmessage({ data: onChannel(id, new Uint8Array([0x01, 7])).buffer });
  assert.deepEqual(frames, [[0x01, 7]]);
  assert.equal(channel.send(commandBytes({ id: 0, verb: "resize", payload: { rows: 9, cols: 9 } })), true);
  assert.deepEqual([sockets[0].sent.at(-1).channel, sockets[0].sent.at(-1).verb], [id, "resize"]);
});

// The console decides when to open again, and with which byte count: the
// next socket does not open its channel by itself.
test("a lost page socket ends a console channel once, and the next socket does not open it again", () => {
  const { d, sockets, open, drop, runRetries, withFakes } = pageSockets();
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  const ends = [];
  const channel = withFakes(() =>
    d.openConsole({ id: 5 }, { onFrame() {}, onClosed: (reason) => ends.push(reason) }),
  );
  drop(sockets[0]);
  assert.deepEqual(ends, ["socket-lost"]);
  assert.equal(channel.readyState, 3);
  runRetries();
  open(sockets[1]);
  assert.deepEqual(
    sockets[1].sent.filter((f) => f.verb === "channel.open"),
    [],
    "the new socket opens no console channel",
  );
  assert.deepEqual(ends, ["socket-lost"]);
});

// A console alone on the page socket leaves it when its channel ends. The
// socket then stays closed until a user joins: a second socket would open
// every waiting channel twice.
test("a page socket that a drop left with no user does not reconnect", () => {
  const { d, sockets, open, drop, runRetries, withFakes } = pageSockets();
  withFakes(() => d.openConsole({ id: 5 }, { onFrame() {}, onClosed() {} }));
  open(sockets[0]);
  drop(sockets[0]);
  runRetries();
  assert.equal(sockets.length, 1, "no user, no socket");
  const again = withFakes(() => d.openConsole({ id: 5, since: 9 }, { onFrame() {}, onClosed() {} }));
  runRetries();
  assert.equal(sockets.length, 2, "the next open makes one socket");
  open(sockets[1]);
  assert.equal(again.readyState, 1);
  assert.equal(sockets[1].sent.filter((f) => f.verb === "channel.open").length, 1);
});
