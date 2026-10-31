// Unit tests for assets/ui/wb-channels.ts: the channel frame, its pieces, the
// channel ids and the routing of what the socket receives. A fake socket-like
// object records what is sent; no WebSocket and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { MAX_PIECE, TAG_CHANNEL, pageChannels } from "../assets/ui/wb-channels.ts";

const OPEN = 1;
const CLOSED = 3;

function fakeSocket() {
  const sent = [];
  return {
    sent,
    readyState: OPEN,
    send(bytes) {
      sent.push(bytes);
    },
  };
}

// A bare command frame: [0x02][JSON {id, verb, payload}].
function bareCommand(verb, payload) {
  const body = new TextEncoder().encode(JSON.stringify({ id: 0, verb, payload }));
  return Uint8Array.from([0x02, ...body]);
}

function readCommand(bytes) {
  assert.equal(bytes[0], 0x02, "a bare command frame");
  return JSON.parse(new TextDecoder().decode(bytes.subarray(1)));
}

// A channel frame, written by hand from the wire description.
function channelFrame(channel, more, piece) {
  const out = new Uint8Array(6 + piece.length);
  out[0] = TAG_CHANNEL;
  new DataView(out.buffer).setUint32(1, channel);
  out[5] = more ? 1 : 0;
  out.set(piece, 6);
  return out;
}

function readChannelFrame(bytes) {
  assert.equal(bytes[0], 0x04, "a channel frame");
  return {
    channel: new DataView(bytes.buffer, bytes.byteOffset).getUint32(1),
    flags: bytes[5],
    piece: bytes.subarray(6),
  };
}

function pattern(length, seed) {
  return Uint8Array.from({ length }, (_, i) => (i * 7 + seed) % 251);
}

// The page layer over a fresh fake socket, with the bare frames it hands on.
function layer() {
  const socket = fakeSocket();
  const bare = [];
  const channels = pageChannels(socket, (bytes) => bare.push(bytes));
  return { socket, bare, channels };
}

test("a channel frame piece is at most 16384 bytes", () => {
  assert.equal(MAX_PIECE, 16384);
});

test("open sends channel.open on channel 0 with the id, the kind and the payload", () => {
  const { socket, channels } = layer();
  const channel = channels.open("console", { id: "abc", since: 12 });
  assert.equal(socket.sent.length, 1);
  assert.equal(readCommand(socket.sent[0]).verb, "channel.open");
  assert.deepEqual(readCommand(socket.sent[0]).payload, {
    channel: 1,
    kind: "console",
    id: "abc",
    since: 12,
  });
  assert.equal(channel.id, 1);
  assert.equal(channel.readyState, "open");
});

test("a payload field cannot replace the channel id or the kind", () => {
  const { socket, channels } = layer();
  channels.open("tree", { channel: 99, kind: "spawn" });
  const { payload } = readCommand(socket.sent[0]);
  assert.equal(payload.channel, 1);
  assert.equal(payload.kind, "tree");
});

test("cuts a large frame into pieces of 16 KiB", () => {
  const { socket, channels } = layer();
  const channel = channels.open("console", {});
  const inner = pattern(40 * 1024, 3);
  assert.equal(channel.send(inner), true);
  const pieces = socket.sent.slice(1).map(readChannelFrame);
  assert.equal(pieces.length, 3);
  assert.deepEqual(
    pieces.map((p) => p.flags),
    [1, 1, 0],
    "MORE on the first two pieces only",
  );
  for (const piece of pieces) {
    assert.equal(piece.channel, 1);
    assert.ok(piece.piece.length <= 16384, `piece of ${piece.piece.length} bytes`);
  }
  const joined = Uint8Array.from(pieces.flatMap((p) => [...p.piece]));
  assert.deepEqual(joined, inner);
});

test("a frame of exactly 16 KiB is one piece, and an empty frame is one empty piece", () => {
  const { socket, channels } = layer();
  const channel = channels.open("console", {});
  channel.send(pattern(16384, 1));
  channel.send(new Uint8Array(0));
  const pieces = socket.sent.slice(1).map(readChannelFrame);
  assert.deepEqual(
    pieces.map((p) => [p.flags, p.piece.length]),
    [
      [0, 16384],
      [0, 0],
    ],
  );
});

test("joins interleaved pieces per channel", () => {
  const { channels } = layer();
  const a = channels.open("console", {});
  const b = channels.open("console", {});
  const gotA = [];
  const gotB = [];
  a.onFrame = (bytes) => gotA.push(bytes);
  b.onFrame = (bytes) => gotB.push(bytes);
  const innerA = pattern(3000, 1);
  const innerB = pattern(2000, 2);
  channels.receive(channelFrame(a.id, true, innerA.subarray(0, 1000)));
  channels.receive(channelFrame(b.id, true, innerB.subarray(0, 500)));
  channels.receive(channelFrame(a.id, true, innerA.subarray(1000, 2000)));
  channels.receive(channelFrame(b.id, false, innerB.subarray(500)));
  assert.deepEqual(gotA, [], "a frame waits for its last piece");
  assert.deepEqual(gotB, [innerB]);
  channels.receive(channelFrame(a.id, false, innerA.subarray(2000)));
  assert.deepEqual(gotA, [innerA]);
  assert.deepEqual(gotB, [innerB]);
});

test("never reuses a channel id on one socket", () => {
  const { socket, channels } = layer();
  const first = channels.open("tree", {});
  first.close();
  const second = channels.open("tree", {});
  assert.ok(second.id > first.id, `${second.id} is above ${first.id}`);
  const ids = socket.sent
    .map(readCommand)
    .filter((c) => c.verb === "channel.open")
    .map((c) => c.payload.channel);
  assert.deepEqual(ids, [1, 2]);
});

test("close sends channel.close, and the channel sends nothing after it", () => {
  const { socket, channels } = layer();
  const channel = channels.open("spawn", {});
  channel.close();
  assert.equal(channel.readyState, "closed");
  const close = readCommand(socket.sent[1]);
  assert.equal(close.verb, "channel.close");
  assert.deepEqual(close.payload, { channel: 1 });
  assert.equal(channel.send(Uint8Array.of(1)), false);
  channel.close();
  assert.equal(socket.sent.length, 2, "a second close sends nothing");
});

test("hands channel.closed to its channel", () => {
  const { bare, channels } = layer();
  const a = channels.open("console", {});
  const b = channels.open("console", {});
  const closedA = [];
  const closedB = [];
  a.onClosed = (reason) => closedA.push(reason);
  b.onClosed = (reason) => closedB.push(reason);
  channels.receive(bareCommand("channel.closed", { channel: b.id, reason: "refused" }));
  assert.deepEqual(closedA, []);
  assert.deepEqual(closedB, ["refused"]);
  assert.equal(b.readyState, "closed");
  assert.equal(a.readyState, "open");
  assert.deepEqual(bare, [], "channel 0 does not see a channel.closed");
});

test("a channel.closed with overflow reaches its channel as overflow", () => {
  const { channels } = layer();
  const channel = channels.open("tree", {});
  const reasons = [];
  channel.onClosed = (reason) => reasons.push(reason);
  channels.receive(bareCommand("channel.closed", { channel: channel.id, reason: "overflow" }));
  assert.deepEqual(reasons, ["overflow"]);
  assert.equal(channel.readyState, "closed");
});

test("a frame that arrives after channel.closed is dropped", () => {
  const { channels } = layer();
  const channel = channels.open("console", {});
  const frames = [];
  channel.onFrame = (bytes) => frames.push(bytes);
  channels.receive(bareCommand("channel.closed", { channel: channel.id, reason: "done" }));
  channels.receive(channelFrame(channel.id, false, Uint8Array.of(1, 2)));
  assert.deepEqual(frames, []);
});

test("a bare frame goes to the channel 0 handler as it came", () => {
  const { bare, channels } = layer();
  const push = bareCommand("tree.dirty", { dir: "" });
  const presence = Uint8Array.of(0x03, 123, 125);
  channels.receive(push);
  channels.receive(presence);
  assert.deepEqual(bare, [push, presence]);
});

test("a channel frame for an id no channel holds is dropped", () => {
  const { bare, channels } = layer();
  channels.receive(channelFrame(7, false, Uint8Array.of(1)));
  assert.deepEqual(bare, []);
});

test("a channel frame with an unknown flag is dropped", () => {
  const { channels } = layer();
  const channel = channels.open("console", {});
  const frames = [];
  channel.onFrame = (bytes) => frames.push(bytes);
  const bad = channelFrame(channel.id, false, Uint8Array.of(1));
  bad[5] = 0x02;
  channels.receive(bad);
  assert.deepEqual(frames, []);
});

test("a socket that is gone closes every open channel with socket-lost", () => {
  const { channels } = layer();
  const a = channels.open("console", {});
  const b = channels.open("runs", {});
  b.close();
  const reasons = [];
  a.onClosed = (reason) => reasons.push(["a", reason]);
  b.onClosed = (reason) => reasons.push(["b", reason]);
  channels.socketLost();
  assert.deepEqual(reasons, [["a", "socket-lost"]]);
  assert.equal(a.readyState, "closed");
});

test("a channel opened on a socket that is not open starts closed and sends nothing", () => {
  const { socket, channels } = layer();
  socket.readyState = CLOSED;
  const channel = channels.open("console", {});
  assert.equal(channel.readyState, "closed");
  assert.equal(channel.send(Uint8Array.of(1)), false);
  assert.equal(socket.sent.length, 0);
});
