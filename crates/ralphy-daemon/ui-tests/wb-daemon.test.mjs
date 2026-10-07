// Unit tests for assets/ui/wb-daemon.ts — runs the real module with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
import { createDaemon } from "../assets/ui/wb-daemon.ts";
import { createConsole } from "../assets/ui/wb-console.ts";

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
  return createDaemon(window, document, location);
}

// --- resumeDecision: the resume rule, shared with wb-console.ts ------------
// A tablet suspends the tab: no JS runs while the link is torn down, so the
// sockets come back reporting OPEN with nothing ever arriving on them again.
// The fixed 3s retry only helps the ones that actually heard their close.

test("resumeDecision reconnects exactly the sockets the resume must replace", () => {
  const { resumeDecision, CONNECT_TIMEOUT_MS: T } = load();
  // [case, socket, the stale verdicts it is asked under, expected]
  const rows = [
    // A socket that is gone reconnects, whatever the verdict.
    ["gone (null)", { readyState: null }, [true, false], "reconnect"],
    ["gone (undefined)", { readyState: undefined }, [true, false], "reconnect"],
    ["CLOSING", { readyState: 2 }, [true, false], "reconnect"],
    ["CLOSED", { readyState: 3 }, [true, false], "reconnect"],
    // A young CONNECTING socket is left alone — it IS the reconnect.
    ["young CONNECTING", { readyState: 0 }, [true, false], "none"],
    ["CONNECTING for 0 ms", { readyState: 0, connectingMs: 0 }, [true, false], "none"],
    [
      "CONNECTING just before the deadline",
      { readyState: 0, connectingMs: T - 1 },
      [true, false],
      "none",
    ],
    // Opened before the suspend, or onto a link that was not up yet: its
    // deadline timer froze with the tab, so the resume is what retires it.
    ["CONNECTING at the deadline", { readyState: 0, connectingMs: T }, [true, false], "reconnect"],
    // An OPEN socket churns only when the caller says it is stale.
    ["OPEN and stale", { readyState: 1 }, [true], "reconnect"],
    ["OPEN and not stale", { readyState: 1 }, [false], "none"],
  ];
  for (const [name, socket, stales, want] of rows) {
    for (const stale of stales) {
      assert.equal(resumeDecision({ ...socket, stale }), want, `${name}, stale=${stale}`);
    }
  }
});

test("a handshake that never opens is closed at the deadline, and nothing else is", async () => {
  const d = load();
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
  try {
    d.subscribePresence(() => {});
    d.subscribeRuns("r", () => {});
    d.subscribeChanges("r", () => {});
  } finally {
    globalThis.setTimeout = realSetTimeout;
    delete globalThis.WebSocket;
  }
  assert.equal(sockets.length, 3);
  const deadlines = timers.filter((t) => t.ms === d.CONNECT_TIMEOUT_MS);
  assert.equal(deadlines.length, 3, "every subscription arms a handshake deadline");
  sockets[1].readyState = 1; // this one opened in time
  for (const t of deadlines) t.fn();
  assert.deepEqual(
    sockets.map((s) => s.closes),
    [1, 0, 1],
  );
});

// The two modules run the same rule because they resume on the same event. If
// they ever disagree, one half of the page comes back and the other does not.
test("the console and the daemon door answer the resume question identically", () => {
  const { resumeDecision } = load();
  const window = { addEventListener() {} };
  const document = { readyState: "loading", addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    window.WBConsole = createConsole(window, document, location, {});
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
  const other = window.WBConsole.resumeDecision;
  const { CONNECT_TIMEOUT_MS } = load();
  assert.equal(window.WBConsole.CONNECT_TIMEOUT_MS, CONNECT_TIMEOUT_MS);
  for (const readyState of [null, 0, 1, 2, 3]) {
    for (const stale of [true, false]) {
      for (const connectingMs of [undefined, 0, CONNECT_TIMEOUT_MS - 1, CONNECT_TIMEOUT_MS]) {
        assert.equal(
          resumeDecision({ readyState, stale, connectingMs }),
          other({ readyState, stale, connectingMs }),
          `readyState=${readyState} stale=${stale} connectingMs=${connectingMs}`,
        );
      }
    }
  }
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

// --- the unknown-checkout door: the one automated link from the daemon's
// reply to the shell's `checkoutGone`. Driven through the REAL `observe` over a
// fake WebSocket that answers one tagged frame.

function loadWithSocket(replyFor) {
  const window = { addEventListener() {} };
  const document = { addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  class FakeSocket {
    constructor() {
      this.binaryType = "";
      setTimeout(() => this.onopen?.(), 0);
    }
    send(bytes) {
      const cmd = JSON.parse(new TextDecoder().decode(bytes.subarray(1)));
      const reply = replyFor(cmd);
      const body = new TextEncoder().encode(JSON.stringify({ id: cmd.id, verb: cmd.verb, payload: reply }));
      const out = new Uint8Array(1 + body.length);
      out[0] = 0x02;
      out.set(body, 1);
      setTimeout(() => this.onmessage?.({ data: out.buffer }), 0);
    }
    close() {}
  }
  const realWS = globalThis.WebSocket;
  globalThis.WebSocket = FakeSocket;
  return { d: createDaemon(window, document, location), restore: () => (globalThis.WebSocket = realWS) };
}

test("observe fans out an unknown checkout to the registered listeners, after the reply", async () => {
  const { d, restore } = loadWithSocket((cmd) =>
    cmd.payload.checkout === "gone"
      ? { status: "error", message: "unknown checkout" }
      : cmd.payload.checkout === "missing-file"
        ? { status: "error", reason: "not found" }
        : { status: "ok", entries: [] },
  );
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

// --- subscribeTree: the tree socket also holds the checkout's HEAD ---------

// Timers are captured, never run: the handshake deadline and the 3s retry are
// fired by hand, so no test waits on a real clock.
function treeSocket(checkout, onHead, onFailed) {
  const d = load();
  const sockets = [];
  const timers = [];
  const FakeSocket = class {
    constructor() {
      this.sent = [];
      this.readyState = 0;
      sockets.push(this);
    }
    send(bytes) {
      this.sent.push(JSON.parse(new TextDecoder().decode(bytes.subarray(1))));
    }
    close() {
      this.readyState = 3;
    }
  };
  // Every socket is created with the fakes in place: the first one here, and a
  // reconnect inside `withFakes` below.
  const withFakes = (fn) => {
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    globalThis.setTimeout = (fn, ms) => timers.push({ fn, ms }) - 1;
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
  const dirty = [];
  const sub = withFakes(() =>
    d.subscribeTree("o/r", (rel) => dirty.push(rel), checkout, onHead, onFailed),
  );
  const ws = sockets[0];
  const open = (s) => {
    s.readyState = 1;
    s.onopen();
  };
  open(ws);
  const push = (verb, payload) => {
    const body = new TextEncoder().encode(JSON.stringify({ id: 0, verb, payload }));
    const out = new Uint8Array(1 + body.length);
    out[0] = 0x02;
    out.set(body, 1);
    ws.onmessage({ data: out.buffer });
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
  return { d, sub, ws, sockets, dirty, push, open, drop, runRetries, withFakes };
}

const sentOn = (s) => s.sent.map((f) => [f.verb, f.payload.path]);

test("subscribeTree holds the checkout's HEAD and routes head.dirty to onHead", () => {
  let heads = 0;
  const { ws, dirty, push } = treeSocket("wt-a", () => (heads += 1));
  assert.deepEqual(
    ws.sent.map((f) => [f.verb, f.payload]),
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
  assert.deepEqual(ws.sent, []);
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
  assert.equal(ws.sent.length, 5);
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
