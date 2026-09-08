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

test("resumeDecision leaves a CONNECTING socket alone — it IS the reconnect", () => {
  const { resumeDecision } = load();
  assert.equal(resumeDecision({ readyState: 0, stale: true }), "none");
  assert.equal(resumeDecision({ readyState: 0, stale: false }), "none");
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
  for (const readyState of [null, 0, 1, 2, 3]) {
    for (const stale of [true, false]) {
      assert.equal(
        resumeDecision({ readyState, stale }),
        other({ readyState, stale }),
        `readyState=${readyState} stale=${stale}`,
      );
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
