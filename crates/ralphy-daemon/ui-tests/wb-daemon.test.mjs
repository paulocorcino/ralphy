// Unit tests for assets/ui/wb-daemon.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-daemon.js"), "utf8");

function load() {
  // The two globals the module touches at LOAD time: `document.addEventListener`
  // (it subscribes to the `workbench:action` seam) and `location.protocol`/`host`
  // (WS_ORIGIN). No `WebSocket` is injected: nothing opens a socket at load, so
  // a constructor call moved to module scope fails LOUDLY here.
  const window = { addEventListener() {} };
  const document = { addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  new Function("window", "document", "location", SRC)(window, document, location);
  return window.WBDaemon;
}

// --- resumeDecision: the resume rule, shared with wb-console.js ------------
// A tablet suspends the tab: no JS runs while the link is torn down, so the
// sockets come back reporting OPEN with nothing ever arriving on them again.
// The fixed 3s retry only helps the ones that actually heard their close.

test("resumeDecision reconnects a socket that is gone, whatever the verdict", () => {
  const { resumeDecision } = load();
  for (const stale of [true, false]) {
    assert.equal(resumeDecision({ readyState: null, stale }), "reconnect");
    assert.equal(resumeDecision({ readyState: undefined, stale }), "reconnect");
    assert.equal(resumeDecision({ readyState: 2, stale }), "reconnect");
    assert.equal(resumeDecision({ readyState: 3, stale }), "reconnect");
  }
});

test("resumeDecision leaves a young CONNECTING socket alone — it IS the reconnect", () => {
  const { resumeDecision, CONNECT_TIMEOUT_MS } = load();
  for (const stale of [true, false]) {
    assert.equal(resumeDecision({ readyState: 0, stale }), "none");
    assert.equal(resumeDecision({ readyState: 0, stale, connectingMs: 0 }), "none");
    assert.equal(
      resumeDecision({ readyState: 0, stale, connectingMs: CONNECT_TIMEOUT_MS - 1 }),
      "none",
    );
  }
});

test("resumeDecision replaces a CONNECTING socket past the handshake deadline", () => {
  const { resumeDecision, CONNECT_TIMEOUT_MS } = load();
  // Opened before the suspend, or onto a link that was not up yet: its deadline
  // timer froze with the tab, so the resume is what retires it.
  for (const stale of [true, false]) {
    assert.equal(
      resumeDecision({ readyState: 0, stale, connectingMs: CONNECT_TIMEOUT_MS }),
      "reconnect",
    );
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

test("resumeDecision only churns an OPEN socket when the caller says it is stale", () => {
  const { resumeDecision } = load();
  assert.equal(resumeDecision({ readyState: 1, stale: true }), "reconnect");
  assert.equal(resumeDecision({ readyState: 1, stale: false }), "none");
});

// The two modules run the same rule because they resume on the same event. If
// they ever disagree, one half of the page comes back and the other does not.
test("the console and the daemon door answer the resume question identically", () => {
  const { resumeDecision } = load();
  const consoleSrc = readFileSync(join(UI, "wb-console.js"), "utf8");
  const window = { addEventListener() {} };
  const document = { readyState: "loading", addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  new Function("window", readFileSync(join(UI, "wb-geometry.js"), "utf8"))(window);
  new Function("window", readFileSync(join(UI, "wb-window-state.js"), "utf8"))(window);
  new Function("window", readFileSync(join(UI, "wb-desk-sink.js"), "utf8"))(window);
  new Function("window", readFileSync(join(UI, "wb-detach-link.js"), "utf8"))(window);
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    new Function("window", "document", "location", consoleSrc)(window, document, location);
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

test("the resume debounce is exported so both triggers share one threshold", () => {
  const d = load();
  // `visibilitychange` and `online` both land on one iOS resume; the second
  // must not tear down the socket the first just opened.
  assert.equal(typeof d.RESUME_DEBOUNCE_MS, "number");
  assert.ok(d.RESUME_DEBOUNCE_MS > 0);
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

test("onUnknownCheckout is a registration door", () => {
  const d = load();
  assert.equal(typeof d.onUnknownCheckout, "function");
  assert.doesNotThrow(() => d.onUnknownCheckout(() => {}));
});

// --- the unknown-checkout door: the one automated link from the daemon's
// reply to the shell's `checkoutGone`. Driven through the REAL `observe` over a
// fake WebSocket that answers one tagged frame.

function loadWithSocket(replyFor) {
  const window = { addEventListener() {} };
  const document = { addEventListener() {} };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  // `checkoutAfter`'s real rule, from the real module.
  new Function("window", readFileSync(join(UI, "wb-project.js"), "utf8"))(window);
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
  new Function("window", "document", "location", SRC)(window, document, location);
  return { d: window.WBDaemon, restore: () => (globalThis.WebSocket = realWS) };
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
