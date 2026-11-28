// The presence, command and console channels of assets/ui/wb-daemon.ts, and
// the resume triggers of its page socket.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  newDaemon,
  commandBytes,
  onChannel,
  pageSockets,
  eventPage,
  HEARTBEAT_LOST,
} from "./wb-daemon-fixture.mjs";

const frame = (tag, obj) => {
  const body = new TextEncoder().encode(JSON.stringify(obj));
  const out = new Uint8Array(1 + body.length);
  out[0] = tag;
  out.set(body, 1);
  return { data: out.buffer };
};

// --- subscribePresence: the presence socket also carries the daemon's pushes

test("subscribePresence hands a command frame to onPush and each open to onOpen", () => {
  const d = newDaemon();
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
  ws.onmessage(frame(0x02, { id: 0, verb: "sessions.dirty", payload: {} }));
  ws.onmessage(frame(0x02, { id: 0, verb: "desk.dirty", payload: { tab: "t1" } }));
  // A heartbeat starts the watch for the next one: no real timer.
  globalThis.setTimeout = () => 0;
  try {
    ws.onmessage(frame(0x03, { uptime_secs: 4 }));
  } finally {
    globalThis.setTimeout = realSetTimeout;
  }
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

test("a command on an open page socket outlives the handshake deadline", () => {
  const { d, sockets, timers, open, push, withFakes } = pageSockets();
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  const seen = [];
  withFakes(() => d.spawn("run", { repo: "o/r" }, (st) => seen.push(st.status)));
  push(sockets[0], "spawn", "run", { status: "spawned", pid: 7 });
  // A run longer than the handshake deadline keeps streaming.
  withFakes(() => {
    for (const t of timers) if (t.ms === 8000) t.fn();
  });
  push(sockets[0], "spawn", "run", { status: "output", chunk: "late" });
  push(sockets[0], "spawn", "run", { status: "exited", code: 0 });
  assert.deepEqual(seen, ["spawned", "output", "exited"]);
});

test("a command whose page socket misses the handshake deadline runs nothing", () => {
  const { d, sockets, timers, open, runRetries, withFakes } = pageSockets();
  withFakes(() => d.subscribePresence(() => {}));
  const seen = [];
  withFakes(() => d.spawn("run", { repo: "o/r" }, (st) => seen.push(st.status)));
  // The handshake deadline passes with the page socket still connecting.
  withFakes(() => {
    for (const t of timers) if (t.ms === 8000) t.fn();
  });
  runRetries();
  open(sockets.at(-1));
  const spawnOpens = sockets.flatMap((s) =>
    s.sent.filter((f) => f.verb === "channel.open" && f.payload.kind === "spawn"),
  );
  assert.equal(spawnOpens.length, 0, "a late socket does not run the command");
  assert.deepEqual(seen, []);
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

// A resume ends the console's channel, and a console that reopens on
// `socket-lost` joins the page socket again inside that end. The resume
// leaves one socket: a second one would open the console twice.
test("a resume of a socket whose only user is a console makes one socket and opens the console once", () => {
  const { d, sockets, open, withFakes, online, resumes } = eventPage();
  const reopen = (since) =>
    d.openConsole({ id: 5, since }, {
      onFrame() {},
      onClosed: (reason) => {
        if (reason === "socket-lost") reopen(since + 1);
      },
    });
  withFakes(() => reopen(0));
  open(sockets[0]);
  online();
  assert.deepEqual(resumes, [[true, "online"]], "a new link is always stale");
  assert.equal(sockets.length, 2, "one new socket");
  for (const s of sockets.slice(1)) if (s.readyState === 0) open(s);
  const opens = sockets.slice(1).flatMap((s) => s.sent.filter((f) => f.verb === "channel.open"));
  assert.deepEqual(
    opens.map((f) => [f.payload.kind, f.payload.since]),
    [["console", 1]],
  );
});

// #834: the daemon reads the frames sent before a close, so a console names
// its leave on the old socket before the resume closes it.
test("a resume lets each console send its last frame on the old socket", () => {
  const { d, sockets, open, withFakes, online } = eventPage();
  const states = [];
  withFakes(() =>
    d.openConsole({ id: 5 }, {
      onFrame() {},
      onClosed() {},
      onResume: () => states.push(sockets[0].readyState),
    }),
  );
  open(sockets[0]);
  online();
  assert.deepEqual(states, [1], "called once, while the old socket is open");
});

// --- the resume triggers: the door hears each one once and judges "stale" once

// #751: a desktop that slept with the tab in front fires no page event, and
// its socket reads OPEN with no byte to come. The heartbeat that stops is the
// trigger, and it opens one socket, not one per module that heard it.
test("one lost heartbeat opens one new page socket", () => {
  const page = eventPage({ resumeDebounceMs: 0 });
  const { d, sockets, open, withFakes } = page;
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  page.beat(sockets[0]);
  page.runTimers(HEARTBEAT_LOST);
  for (const s of sockets.slice(1)) open(s);
  page.runTimers(HEARTBEAT_LOST);
  assert.equal(sockets.length, 2, "exactly one new page socket");
  assert.equal(sockets[0].readyState, 3, "the silent socket is closed");
  assert.deepEqual(page.resumes, [[true, "heartbeat"]], "the consoles hear it once");
});

// One resume per loss, so a daemon that is down is not dialled again and
// again. A page that never had a heartbeat lost nothing; a hidden one
// resumes on its return instead.
test("a heartbeat loss resumes once, and never on a page that is hidden or never heard one", () => {
  const silent = eventPage({ resumeDebounceMs: 0 });
  silent.withFakes(() => silent.d.subscribePresence(() => {}));
  silent.open(silent.sockets[0]);
  silent.runTimers(HEARTBEAT_LOST);
  assert.equal(silent.sockets.length, 1, "no heartbeat yet: nothing was lost");

  const page = eventPage({ resumeDebounceMs: 0 });
  page.withFakes(() => page.d.subscribePresence(() => {}));
  page.open(page.sockets[0]);
  page.beat(page.sockets[0]);
  page.document.visibilityState = "hidden";
  page.runTimers(HEARTBEAT_LOST);
  assert.equal(page.sockets.length, 1, "a hidden page does not resume on the loss");
  assert.deepEqual(page.resumes, []);
});

// A desktop tab on another tab for minutes still hears the daemon: its
// socket is alive, and replacing it would only churn the consoles.
test("a live heartbeat keeps the socket after a long hide", (t) => {
  let now = 1_000_000;
  t.mock.method(Date, "now", () => now);
  const page = eventPage();
  const { d, sockets, open, withFakes } = page;
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  page.beat(sockets[0]);
  now += 1000;
  page.hide();
  now += 119_000;
  page.beat(sockets[0]);
  now += 1000;
  page.show();
  assert.equal(sockets.length, 1, "no new socket");
  assert.equal(sockets[0].readyState, 1, "the socket stays open");
  assert.deepEqual(page.resumes, [[false, "visible"]]);
});

// The other side of the rule: the page heard no heartbeat for longer than the
// loss limit while it was hidden, so its socket reads OPEN with no byte to
// come. The hidden page fires no loss watch, so the return is what judges it.
test("a heartbeat older than the loss limit makes a page stale when it is shown", (t) => {
  let now = 1_000_000;
  t.mock.method(Date, "now", () => now);
  const page = eventPage();
  const { d, sockets, open, withFakes } = page;
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  page.beat(sockets[0]);
  now += 1000;
  page.hide();
  now += 119_000;
  page.show();
  assert.equal(sockets.length, 2, "one new page socket");
  assert.equal(sockets[0].readyState, 3, "the silent socket is closed");
  assert.deepEqual(page.resumes, [[true, "visible"]]);
});

// Each heartbeat ends the watch of the one before. A watch left behind per
// beat would fire 6 s after its own beat and replace the socket of a page
// that hears the daemon.
test("a page that hears a heartbeat every 2 s never resumes on a loss", (t) => {
  let now = 1_000_000;
  t.mock.method(Date, "now", () => now);
  const page = eventPage({ resumeDebounceMs: 0 });
  const { d, sockets, open, withFakes } = page;
  withFakes(() => d.subscribePresence(() => {}));
  open(sockets[0]);
  for (let i = 0; i < 5; i += 1) {
    page.beat(sockets[0]);
    now += 2000;
  }
  // 10 s passed since the first beat; the last one is 2 s old. The only watch
  // that may still be armed is the last one: run every other one.
  const watches = page.timers.filter((x) => x.ms === HEARTBEAT_LOST);
  assert.equal(watches.length, 5, "one watch armed per heartbeat");
  withFakes(() => {
    for (const w of watches.slice(0, -1)) w.fn();
  });
  assert.equal(sockets.length, 1, "no new socket");
  assert.deepEqual(page.resumes, []);
});

// The detached fence page subscribes to no presence, so it has no heartbeat
// to ask: how long it was hidden is its verdict.
test("a page with no heartbeat is stale after 60 s hidden", (t) => {
  let now = 1_000_000;
  t.mock.method(Date, "now", () => now);
  const shownAfter = (hiddenMs) => {
    const page = eventPage();
    const reopen = (since) =>
      page.d.openConsole({ id: 5, since }, {
        onFrame() {},
        onClosed: (reason) => {
          if (reason === "socket-lost") reopen(since + 1);
        },
      });
    page.withFakes(() => reopen(0));
    page.open(page.sockets[0]);
    page.hide();
    now += hiddenMs;
    page.show();
    return { sockets: page.sockets.length, resumes: page.resumes };
  };
  assert.deepEqual(shownAfter(61_000), { sockets: 2, resumes: [[true, "visible"]] });
  assert.deepEqual(shownAfter(59_000), { sockets: 1, resumes: [[false, "visible"]] });
});
