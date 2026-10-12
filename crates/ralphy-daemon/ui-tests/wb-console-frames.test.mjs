// Unit tests for assets/ui/wb-console-frames.ts: one socket message folded
// into a SocketEvent. Pure function, no socket and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readSocketEvent } from "../assets/ui/wb-console-frames.ts";
import { TAG_COMMAND, TAG_TERMINAL } from "../assets/ui/wb-console-session.ts";

function terminalFrame(id, text) {
  const data = new TextEncoder().encode(text);
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
