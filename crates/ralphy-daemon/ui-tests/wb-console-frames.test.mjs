// Unit tests for assets/ui/wb-console-frames.ts: one socket message folded
// into a SocketEvent, and the queue that keeps live frames behind a gzip
// replay. No socket and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { frameQueue, readSocketEvent } from "../assets/ui/wb-console-frames.ts";
import { TAG_COMMAND, TAG_TERMINAL } from "../assets/ui/wb-console-session.ts";

function terminalFrame(id, text) {
  return terminalBytes(id, new TextEncoder().encode(text));
}

function terminalBytes(id, data) {
  const out = new Uint8Array(9 + data.length);
  out[0] = TAG_TERMINAL;
  new DataView(out.buffer).setBigUint64(1, BigInt(id));
  out.set(data, 9);
  return out;
}

function commandFrame(verb, payload) {
  const body = new TextEncoder().encode(JSON.stringify({ id: 0, verb, payload }));
  const out = new Uint8Array(1 + body.length);
  out[0] = TAG_COMMAND;
  out.set(body, 1);
  return out;
}

test("a terminal frame is the big-endian session id and the bytes after it", () => {
  const event = readSocketEvent(terminalFrame(258, "hi"));
  assert.equal(event.kind, "output");
  assert.equal(event.sessionId, 258);
  assert.equal(new TextDecoder().decode(event.bytes), "hi");
});

test("a session-open frame carries its payload", () => {
  const event = readSocketEvent(commandFrame("session-open", { id: 7, watch: true }));
  assert.equal(event.kind, "open");
  assert.deepEqual(event.payload, { id: 7, watch: true });
});

test("a session-size frame carries the rows and the columns", () => {
  assert.deepEqual(readSocketEvent(commandFrame("session-size", { rows: 30, cols: 100 })), {
    kind: "size",
    rows: 30,
    cols: 100,
  });
});

test("a session-size frame without a positive whole size is other", () => {
  for (const payload of [{}, { rows: 30 }, { rows: 0, cols: 100 }, { rows: 30, cols: 1.5 }, { rows: "30", cols: 100 }]) {
    assert.deepEqual(readSocketEvent(commandFrame("session-size", payload)), { kind: "other" }, JSON.stringify(payload));
  }
});

test("a session-end frame carries the reason and the message", () => {
  assert.deepEqual(readSocketEvent(commandFrame("session-end", { reason: "refused", message: "no" })), {
    kind: "end",
    reason: "refused",
    message: "no",
  });
});

test("a session-end frame without a reason is a child exit", () => {
  assert.deepEqual(readSocketEvent(commandFrame("session-end", {})), {
    kind: "end",
    reason: "child-exited",
    message: null,
  });
});

test("another verb, a bad body, a short frame and an unknown tag are other", () => {
  assert.deepEqual(readSocketEvent(commandFrame("resize", {})), { kind: "other" });
  assert.deepEqual(readSocketEvent(Uint8Array.of(TAG_COMMAND, 0x7b)), { kind: "other" });
  assert.deepEqual(readSocketEvent(Uint8Array.of(TAG_TERMINAL, 0, 0)), { kind: "other" });
  assert.deepEqual(readSocketEvent(new Uint8Array(0)), { kind: "other" });
  assert.deepEqual(readSocketEvent(Uint8Array.of(0x09, 1, 2)), { kind: "other" });
});

async function gzip(text) {
  const stream = new Blob([new TextEncoder().encode(text)]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

// What the queue applied, as short labels: the text of an output, the kind of
// anything else.
function recorder() {
  const seen = [];
  const apply = (event) => seen.push(event.kind === "output" ? new TextDecoder().decode(event.bytes) : event.kind);
  return { seen, apply };
}

// Every pending decode and drain has run.
const settle = () => new Promise((resolve) => setTimeout(resolve, 50));

test("live frames that arrive while a gzip replay decodes are applied after it, in order", async () => {
  const { seen, apply } = recorder();
  const take = frameQueue(apply);
  take(commandFrame("session-open", { session: 1, replay_encoding: "gzip" }));
  take(commandFrame("session-size", { rows: 24, cols: 80 }));
  take(terminalBytes(1, await gzip("the screen")));
  take(terminalFrame(1, "live 1"));
  take(terminalFrame(1, "live 2"));
  await settle();
  assert.deepEqual(seen, ["open", "size", "the screen", "live 1", "live 2"]);
});

test("a raw replay is applied at once, and only the frame after a gzip open is decoded", async () => {
  const { seen, apply } = recorder();
  const take = frameQueue(apply);
  take(commandFrame("session-open", { session: 1 }));
  take(terminalFrame(1, "raw screen"));
  take(terminalFrame(1, "live"));
  assert.deepEqual(seen, ["open", "raw screen", "live"], "nothing waits");
});

test("a replay that is not gzip is dropped and the live frames still come", async () => {
  const { seen, apply } = recorder();
  const take = frameQueue(apply);
  const warn = console.warn;
  const warned = [];
  console.warn = (...args) => warned.push(args);
  try {
    take(commandFrame("session-open", { session: 1, replay_encoding: "gzip" }));
    take(terminalFrame(1, "not gzip"));
    take(terminalFrame(1, "live 1"));
    await settle();
    take(terminalFrame(1, "live 2"));
  } finally {
    console.warn = warn;
  }
  assert.deepEqual(seen, ["open", "live 1", "live 2"]);
  assert.equal(warned.length, 1, "the decode error is logged");
});
