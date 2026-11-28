// The page socket of assets/ui/wb-daemon.ts: one `/ws` socket, a channel per
// subscription, and the tree subscription over it.
import { test } from "node:test";
import assert from "node:assert/strict";

import { fixture } from "./harness.mjs";
import { CONNECT_TIMEOUT_MS } from "../assets/ui/wb-resume.ts";
import {
  channelOf,
  sentOn,
  pageSockets,
  eventPage,
  treeSocket,
  channelsOf,
  closeChannel,
} from "./wb-daemon-fixture.mjs";

test("one socket carries presence, tree, runs and changes", () => {
  const { d, sockets, open, push, withFakes } = pageSockets();
  const beats = [];
  const runs = [];
  const changes = [];
  withFakes(() => {
    d.subscribePresence((p) => beats.push(p));
    d.subscribeTree("o/r", () => {}, null);
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
  // Under the fakes: a heartbeat starts the watch for the next one.
  withFakes(() => sockets[0].onmessage({ data: beat.buffer }));
  push(sockets[0], "runs", "runs.dirty", { repo: "o/r" });
  push(sockets[0], "changes", "changes.dirty", { repo: "o/r" });
  assert.deepEqual(beats, [{ uptime_secs: 2 }]);
  assert.deepEqual(runs, ["dirty"]);
  assert.deepEqual(changes, ["changes.dirty"]);
});

// #834: the daemon mints the device cookie on the page's session read, and
// reads the device once, at the upgrade. The first socket waits for the read.
test("the first page socket waits for the session read", async () => {
  const { d, sockets, withFakes, withFakesAsync } = pageSockets();
  let settle = null;
  const read = new Promise((resolve) => { settle = resolve; });
  withFakes(() => {
    d.afterSessionRead(read);
    d.subscribePresence(() => {});
    d.subscribeRuns("o/r", () => {});
  });
  assert.equal(sockets.length, 0, "no socket before the session read");
  await withFakesAsync(async () => {
    settle();
    await read;
  });
  assert.equal(sockets.length, 1, "one socket for both users once the read settled");
});

test("a session read that does not settle holds the first socket only until the handshake deadline", () => {
  const { d, sockets, timers, withFakes } = pageSockets();
  withFakes(() => {
    d.afterSessionRead(new Promise(() => {}));
    d.subscribePresence(() => {});
  });
  assert.equal(sockets.length, 0);
  const deadline = timers.find((t) => t.ms === CONNECT_TIMEOUT_MS);
  withFakes(() => deadline.fn());
  assert.equal(sockets.length, 1);
});

test("a reopened socket re-sends the watches and reads once", () => {
  const { d, sockets, open, drop, runRetries, withFakes } = pageSockets();
  const dirty = [];
  let runReads = 0;
  const sub = withFakes(() => {
    d.subscribePresence(() => {});
    d.subscribeRuns("o/r", () => (runReads += 1));
    return d.subscribeTree("o/r", (rel) => dirty.push(rel), null);
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
    d.subscribeTree("o/r", (rel) => dirty.push(rel), null);
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
  const { ws, push } = treeSocket(null);
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
  const { sub, ws, sockets, drop, runRetries } = treeSocket(null);
  sub.watch("");
  sub.close();
  drop(ws);
  runRetries();
  assert.equal(sockets.length, 1);
});

test("a tree channel the daemon closes is opened again after 3 s with its watches", () => {
  const page = treeSocket(null);
  const { sub, ws, dirty, runRetries } = page;
  sub.watch("");
  sub.watch("src");
  const [first] = channelsOf(ws, "tree");
  closeChannel(page, ws, first, "refused");
  assert.equal(channelsOf(ws, "tree").length, 1, "the open waits for its timer");
  runRetries();
  const ids = channelsOf(ws, "tree");
  assert.equal(ids.length, 2, "one new tree channel on the same socket");
  const resent = ws.sent.filter((f) => f.channel === ids[1]).map((f) => [f.verb, f.payload.path]);
  assert.deepEqual(resent, [
    ["watch", ""],
    ["watch", "src"],
  ]);
  assert.deepEqual(dirty, ["", "src"], "each held dir is read once");
});

test("a reopened tree channel the daemon closes again waits for the next socket", () => {
  const page = treeSocket(null);
  const { sub, ws, sockets, open, drop, runRetries } = page;
  sub.watch("src");
  closeChannel(page, ws, channelsOf(ws, "tree")[0], "refused");
  runRetries();
  const opens = channelsOf(ws, "tree");
  closeChannel(page, ws, opens.at(-1), "overflow");
  runRetries();
  assert.deepEqual(channelsOf(ws, "tree"), opens, "no open after the second close");
  drop(ws);
  runRetries();
  open(sockets[1]);
  assert.deepEqual(sentOn(sockets[1]), [["watch", "src"]], "the next socket opens it again");
});

// The page resumes in the door alone: a subscription has no resume of its
// own, so its channel opens again on the new socket as a reopen.
test("after a page resume the tree sends its watches again on the new socket", () => {
  const page = eventPage();
  const dirty = [];
  page.withFakes(() => {
    const sub = page.d.subscribeTree("o/r", (rel) => dirty.push(rel), null);
    page.open(page.sockets[0]);
    sub.watch("src");
  });
  const first = page.sockets[0];
  page.online();
  assert.equal(page.sockets.length, 2, "the page replaced its socket");
  assert.equal(first.onclose, null, "the retired socket can no longer schedule a retry");
  assert.deepEqual(dirty, [], "nothing is read before the new socket opens");
  page.open(page.sockets[1]);
  assert.deepEqual(sentOn(page.sockets[1]), [["watch", "src"]]);
  assert.deepEqual(dirty, ["src"], "the held dir is read once, after the reopen");
});
