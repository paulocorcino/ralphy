// Unit tests for assets/ui/wb-console-frames.ts: one socket message folded
// into a SocketEvent, and the queue that keeps live frames behind a gzip
// replay. No socket and no DOM.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { DECODE_DEADLINE_MS, frameQueue, readSocketEvent } from "../assets/ui/wb-console-frames.ts";
import { TAG_COMMAND, TAG_TERMINAL } from "../assets/ui/wb-console-session.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

// A reply a Rust test wrote to `fixtures/` (ADR-0070 Compliance, shared replies).
function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

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

test("session-size: the size fold reads the payload the daemon sends", () => {
  const size = fixture("session-size");
  assert.deepEqual(readSocketEvent(commandFrame("session-size", size)), {
    kind: "size",
    rows: size.rows,
    cols: size.cols,
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
  const { take } = frameQueue(apply);
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
  const { take } = frameQueue(apply);
  take(commandFrame("session-open", { session: 1 }));
  take(terminalFrame(1, "raw screen"));
  take(terminalFrame(1, "live"));
  assert.deepEqual(seen, ["open", "raw screen", "live"], "nothing waits");
});

test("a replay that is not gzip is dropped and the live frames still come", async () => {
  const { seen, apply } = recorder();
  const { take } = frameQueue(apply);
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
  assert.deepEqual(seen, ["open", "replay-lost", "live 1", "live 2"], "the terminal is told the replay is lost");
  assert.equal(warned.length, 1, "the decode error is logged");
});

// A close is decided after the frames that wait behind a decode: a
// `session-end` (taken over) that came before the close names its reason.
test("whenIdle runs after the frames that wait behind a gzip replay", async () => {
  const { seen, apply } = recorder();
  const { take, whenIdle } = frameQueue(apply);
  take(commandFrame("session-open", { session: 1, replay_encoding: "gzip" }));
  take(terminalBytes(1, await gzip("the screen")));
  take(commandFrame("session-end", { reason: "taken-over" }));
  whenIdle(() => seen.push("close"));
  assert.deepEqual(seen, ["open"], "the close waits");
  await settle();
  assert.deepEqual(seen, ["open", "the screen", "end", "close"]);
  whenIdle(() => seen.push("at once"));
  assert.equal(seen.at(-1), "at once", "an idle queue runs it at once");
});

// A queued frame that throws costs that frame only: the close that waits for
// the queue must still run, or the console never reconnects.
test("a queued frame that throws does not stop the later frames or whenIdle", async () => {
  const seen = [];
  const apply = (event) => {
    const label = event.kind === "output" ? new TextDecoder().decode(event.bytes) : event.kind;
    if (label === "bad") throw new Error("apply failed");
    seen.push(label);
  };
  const { take, whenIdle } = frameQueue(apply);
  const warn = console.warn;
  const warned = [];
  console.warn = (...args) => warned.push(args);
  try {
    take(commandFrame("session-open", { session: 1, replay_encoding: "gzip" }));
    take(terminalBytes(1, await gzip("the screen")));
    take(terminalFrame(1, "bad"));
    take(terminalFrame(1, "live 1"));
    take(terminalFrame(1, "live 2"));
    whenIdle(() => seen.push("close"));
    await settle();
  } finally {
    console.warn = warn;
  }
  assert.deepEqual(seen, ["open", "the screen", "live 1", "live 2", "close"]);
  assert.equal(warned.length, 1, "the error of the bad frame is logged");
});

// A decode the test settles by hand, or never.
function gzipOpen(take) {
  take(commandFrame("session-open", { session: 1, replay_encoding: "gzip" }));
  take(terminalBytes(1, new Uint8Array([1, 2, 3])));
}

test("a replay decode with no end is lost at its deadline", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { seen, apply } = recorder();
    const { take } = frameQueue(apply, () => new Promise(() => {}));
    gzipOpen(take);
    mock.timers.tick(DECODE_DEADLINE_MS - 1);
    assert.deepEqual(seen, ["open"]);
    mock.timers.tick(1);
    assert.deepEqual(seen, ["open", "replay-lost"]);
    mock.timers.tick(DECODE_DEADLINE_MS);
    assert.deepEqual(seen, ["open", "replay-lost"], "once");
  } finally {
    mock.timers.reset();
  }
});

test("frames behind a decode that passed its deadline are applied in order, and the close runs", () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { seen, apply } = recorder();
    const { take, whenIdle } = frameQueue(apply, () => new Promise(() => {}));
    gzipOpen(take);
    take(terminalFrame(1, "a"));
    take(terminalFrame(1, "b"));
    take(terminalFrame(1, "c"));
    let closed = 0;
    whenIdle(() => closed++);
    assert.equal(closed, 0);
    mock.timers.tick(DECODE_DEADLINE_MS);
    assert.deepEqual(seen, ["open", "replay-lost", "a", "b", "c"]);
    assert.equal(closed, 1);
  } finally {
    mock.timers.reset();
  }
});

test("a decode that ends after its deadline writes nothing", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { seen, apply } = recorder();
    let finish = () => {};
    const { take } = frameQueue(apply, () => new Promise((resolve) => (finish = () => resolve(new Uint8Array([65])))));
    gzipOpen(take);
    mock.timers.tick(DECODE_DEADLINE_MS);
    finish();
    await Promise.resolve();
    await Promise.resolve();
    assert.deepEqual(seen, ["open", "replay-lost"]);
    assert.equal(seen.filter((k) => k === "A").length, 0);
  } finally {
    mock.timers.reset();
  }
});

test("a replay decode past its deadline reports a stall", () => {
  const prev = globalThis.fetch;
  const kinds = [];
  globalThis.fetch = (_url, init) => {
    kinds.push(JSON.parse(init.body).kind);
    return Promise.resolve(new Response(null, { status: 204 }));
  };
  forgetStalls();
  const warn = console.warn;
  console.warn = () => {};
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const { apply } = recorder();
    const { take } = frameQueue(apply, () => new Promise(() => {}));
    gzipOpen(take);
    mock.timers.tick(DECODE_DEADLINE_MS);
    assert.deepEqual(kinds, ["decode-deadline"]);
  } finally {
    mock.timers.reset();
    console.warn = warn;
    globalThis.fetch = prev;
  }
});
