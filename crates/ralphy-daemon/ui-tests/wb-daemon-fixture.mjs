// The fakes the wb-daemon tests share: a daemon door over fake page sockets
// and captured timers, and the frames the page and the daemon send.
import assert from "node:assert/strict";

import { load } from "./harness.mjs";
import { createDaemon } from "../assets/ui/wb-daemon.ts";

// Let every pending promise callback run; setImmediate is not under mock timers.
export const flush = () => new Promise((r) => { setImmediate(r); });

export function newDaemon() {
  return load(createDaemon, { deps: { messages: { flash() {} } } });
}

// A frame the page sent, read: a bare `[0x02][JSON]` frame is channel 0, and a
// `[0x04][channel u32][flags][0x02][JSON]` frame is on its channel. Every frame
// here is small, so a frame is one piece.
export function readSent(bytes) {
  if (bytes[0] === 0x04) {
    const channel = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength).getUint32(1);
    return { channel, ...JSON.parse(new TextDecoder().decode(bytes.subarray(7))) };
  }
  return { channel: 0, ...JSON.parse(new TextDecoder().decode(bytes.subarray(1))) };
}

export function commandBytes(obj) {
  const body = new TextEncoder().encode(JSON.stringify(obj));
  const out = new Uint8Array(1 + body.length);
  out[0] = 0x02;
  out.set(body, 1);
  return out;
}

// The one channel frame that carries `inner` on `channel`.
export function onChannel(channel, inner) {
  const out = new Uint8Array(6 + inner.length);
  out[0] = 0x04;
  new DataView(out.buffer).setUint32(1, channel);
  out.set(inner, 6);
  return out;
}

// The id the page gave its channel of `kind` on socket `s`.
export function channelOf(s, kind) {
  const opens = s.sent.filter((f) => f.channel === 0 && f.verb === "channel.open" && f.payload.kind === kind);
  assert.equal(opens.length, 1, `one ${kind} channel on the socket`);
  return opens[0].payload.channel;
}

// The verbs sent on the channel of `kind`, with their path.
export const sentOn = (s, kind = "tree") => {
  const id = channelOf(s, kind);
  return s.sent.filter((f) => f.channel === id).map((f) => [f.verb, f.payload.path]);
};

// A push of the daemon: bare on channel 0, or on the channel of `kind`.
export const pushFrame = (s, kind, verb, payload) => {
  const inner = commandBytes({ id: 0, verb, payload });
  const bytes = kind ? onChannel(channelOf(s, kind), inner) : inner;
  s.onmessage({ data: bytes.buffer });
};

export const openSocket = (s) => {
  s.readyState = 1;
  s.onopen();
};

// The daemon door with fake sockets and timers. Timers are captured, never
// run: the handshake deadline and the 3s retry are fired by hand, so no test
// waits on a real clock.
export function pageSockets(d = newDaemon()) {
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
  // Put the fakes in place; the returned function puts the real ones back.
  const install = () => {
    const realSetTimeout = globalThis.setTimeout;
    const realClearTimeout = globalThis.clearTimeout;
    globalThis.setTimeout = (callback, ms) => timers.push({ fn: callback, ms }) - 1;
    globalThis.clearTimeout = (id) => {
      if (timers[id]) timers[id].fn = () => {};
    };
    globalThis.WebSocket = FakeSocket;
    return () => {
      globalThis.setTimeout = realSetTimeout;
      globalThis.clearTimeout = realClearTimeout;
      delete globalThis.WebSocket;
    };
  };
  const withFakes = (fn) => {
    const restore = install();
    try {
      return fn();
    } finally {
      restore();
    }
  };
  // `withFakes` for an async `fn`: the fakes stay until it settles.
  const withFakesAsync = async (fn) => {
    const restore = install();
    try {
      return await fn();
    } finally {
      restore();
    }
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
  return { d, sockets, timers, open: openSocket, push: pushFrame, drop, runRetries, withFakes, withFakesAsync };
}

export const on = (bag) => (type, fn) => (bag[type] ||= []).push(fn);

// `pageSockets` on a window and a document that keep their listeners, so a
// test fires the page's own events. `deps` is added to the door's deps.
export function eventPage(deps = {}) {
  const listeners = { window: {}, document: {} };
  const window = { addEventListener: on(listeners.window) };
  const document = { visibilityState: "visible", addEventListener: on(listeners.document) };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  const d = createDaemon(window, document, location, { messages: { flash() {} }, ...deps });
  const page = pageSockets(d);
  const fire = (target, type) =>
    page.withFakes(() => {
      for (const fn of listeners[target][type] || []) fn({ type });
    });
  const setVisibility = (state) => {
    document.visibilityState = state;
    fire("document", "visibilitychange");
  };
  // A presence heartbeat on socket `s`.
  const beat = (s) => {
    const bytes = new Uint8Array([0x03, ...new TextEncoder().encode(JSON.stringify({ uptime_secs: 1 }))]);
    page.withFakes(() => s.onmessage({ data: bytes.buffer }));
  };
  // Run each captured timer of `ms` once, as the browser would when it ends.
  const runTimers = (ms) =>
    page.withFakes(() => {
      for (const t of page.timers.filter((x) => x.ms === ms)) {
        const fn = t.fn;
        t.fn = () => {};
        fn();
      }
    });
  const resumes = [];
  d.onPageResume?.((stale, trigger) => resumes.push([stale, trigger]));
  return {
    ...page,
    document,
    resumes,
    beat,
    runTimers,
    online: () => fire("window", "online"),
    hide: () => setVisibility("hidden"),
    show: () => setVisibility("visible"),
  };
}

// Three missed ~2 s heartbeats: the page's link is gone.
export const HEARTBEAT_LOST = 6000;

export function treeSocket(checkout, onHead, onFailed) {
  const page = pageSockets();
  const dirty = [];
  const sub = page.withFakes(() =>
    page.d.subscribeTree("o/r", (rel) => dirty.push(rel), checkout, { onHead, onFailed }),
  );
  const ws = page.sockets[0];
  page.open(ws);
  const push = (verb, payload) => page.push(ws, "tree", verb, payload);
  return { ...page, sub, ws, dirty, push };
}


// The ids of every channel of `kind` the page opened on socket `s`, in order.
export function channelsOf(s, kind) {
  return s.sent
    .filter((f) => f.channel === 0 && f.verb === "channel.open" && f.payload.kind === kind)
    .map((f) => f.payload.channel);
}

// The daemon ends channel `id` of socket `s` with `reason`.
export function closeChannel(page, s, id, reason) {
  const bytes = commandBytes({ id: 0, verb: "channel.closed", payload: { channel: id, reason } });
  page.withFakes(() => s.onmessage({ data: bytes.buffer }));
}
