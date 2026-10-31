// Unit tests for assets/ui/wb-console-terminal.ts — the console's terminal and
// its channel on the page socket. `createTerminal` is driven with fake `deps`,
// a fake channel opener, and fake xterm and observer classes, with no console
// and no DOM.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { createTerminal } from "../assets/ui/wb-console-terminal.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";
import { CONNECT_TIMEOUT_MS } from "../assets/ui/wb-resume.ts";
import {
  encodeDetach,
  encodeShown,
  encodeTerminal,
  TAG_COMMAND,
  TAG_TERMINAL,
} from "../assets/ui/wb-console-session.ts";

// An element that takes listeners and does nothing with them.
function fakeElement() {
  return { addEventListener() {}, closest: () => null };
}

// The page socket's door for consoles, faked: each `openConsole` call is one
// channel, which records its open payload and what the terminal sent on it.
// A channel opened while the page socket is down waits (readyState 0) and
// opens when the socket comes back.
function fakeDoor() {
  const channels = [];
  let up = true;
  function openConsole(payload, handlers) {
    const channel = {
      payload,
      readyState: up ? 1 : 0,
      sent: [],
      closes: 0,
      send(inner) {
        this.sent.push(inner);
        return this.readyState === 1;
      },
      close() {
        this.readyState = 3;
        this.closes += 1;
      },
      // One frame of the daemon on this channel.
      frame(bytes) {
        handlers.onFrame(bytes);
      },
      // The daemon ends the channel, or the page socket under it ends.
      end(reason) {
        this.readyState = 3;
        handlers.onClosed(reason);
      },
    };
    channels.push(channel);
    return channel;
  }
  return {
    channels,
    openConsole,
    dropSocket() {
      up = false;
      for (const channel of [...channels]) if (channel.readyState !== 3) channel.end("socket-lost");
    },
    reopenSocket() {
      up = true;
      for (const channel of channels) if (channel.readyState === 0) channel.readyState = 1;
    },
  };
}

// The page globals `attachTerminal` reaches: xterm and its addons and the fit
// observer, and the channel door its deps get. Each fake records what the
// terminal did with it.
function fakePage() {
  const terminals = [];
  const door = fakeDoor();
  class FakeTerminal {
    constructor(options) {
      this.options = { ...options };
      this.rows = 24;
      this.cols = 80;
      this.modes = {};
      this.buffer = { active: { viewportY: 0, type: "normal" } };
      this.textarea = fakeElement();
      this.element = null;
      this.osc = new Map();
      // What was written, as text, and how many times the terminal was reset.
      this.written = [];
      this.resets = 0;
      this.parser = { registerOscHandler: (code, handler) => this.osc.set(code, handler) };
      // xterm's width tables: only "6" is built in, an addon registers more,
      // and choosing one that is not registered throws, as xterm does.
      const versions = new Set(["6"]);
      let active = "6";
      this.unicode = {
        register: (version) => versions.add(version),
        get activeVersion() {
          return active;
        },
        set activeVersion(version) {
          if (!versions.has(version)) throw new Error(`unknown Unicode version "${version}"`);
          active = version;
        },
      };
      terminals.push(this);
    }
    loadAddon(addon) {
      addon.activate?.(this);
    }
    open(body) {
      this.element = body;
    }
    write(data, done) {
      this.written.push(typeof data === "string" ? data : Buffer.from(data).toString("utf8"));
      done?.();
    }
    attachCustomKeyEventHandler() {}
    // The data handler, so a test can type.
    onData(handler) {
      this.type = handler;
    }
    onResize() {}
    focus() {}
    reset() {
      this.resets += 1;
    }
    dispose() {}
  }
  // A console opens no WebSocket of its own: it opens a channel of the page socket.
  class NoSocket {
    static OPEN = 1;
    constructor() {
      throw new Error("a console opened a WebSocket of its own");
    }
  }
  const globals = {
    Terminal: FakeTerminal,
    FitAddon: { FitAddon: class { fit() {} } },
    WebLinksAddon: { WebLinksAddon: class {} },
    Unicode11Addon: {
      Unicode11Addon: class {
        activate(term) {
          term.unicode.register("11");
        }
      },
    },
    ResizeObserver: class {
      observe() {}
      disconnect() {}
    },
    WebSocket: NoSocket,
  };
  const saved = Object.fromEntries(Object.keys(globals).map((k) => [k, globalThis[k]]));
  Object.assign(globalThis, globals);
  // The backoff is a real timer; a mocked one never holds the run open.
  mock.timers.enable({ apis: ["setTimeout"] });
  const restore = () => {
    mock.timers.reset();
    Object.assign(globalThis, saved);
  };
  return {
    terminals,
    channels: door.channels,
    openConsole: door.openConsole,
    dropSocket: door.dropSocket,
    reopenSocket: door.reopenSocket,
    restore,
  };
}

// Every member of `TerminalDeps`, the channel opener of `page` included; the
// clipboard write records its calls.
function fakeDeps(page, more = {}) {
  const copies = [];
  const deps = {
    window: {},
    openConsole: page.openConsole,
    fontSize: () => 15,
    stage: () => null,
    workspace: () => null,
    cancelSlide() {},
    writeClipboard: (text, term) => copies.push({ text, term }),
    readClipboard: () => Promise.resolve({ text: "" }),
    ...more,
  };
  return { deps, copies };
}

test("a terminal takes its font size from deps, and opens its channel through the deps opener", () => {
  const page = fakePage();
  try {
    const { deps } = fakeDeps(page, { fontSize: () => 17 });
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { console: true });
    assert.equal(page.terminals[0].options.fontSize, 17);
    assert.equal(page.channels.length, 1);
    assert.equal(page.channels[0].payload.console, 1);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a terminal counts character widths by Unicode 11, so an emoji takes two columns", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { console: true });
    assert.equal(page.terminals[0].unicode.activeVersion, "11");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("an OSC 52 copy reaches deps.writeClipboard with its control characters removed", () => {
  const page = fakePage();
  try {
    const { deps, copies } = fakeDeps(page);
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { console: true });
    const term = page.terminals[0];
    // A BEL (C0) and a NEL (C1) inside the copied text.
    const payload = Buffer.from("a\u0007b\u0085c", "utf8").toString("base64");
    assert.equal(term.osc.get(52)("c;" + payload), true);
    assert.deepEqual(copies, [{ text: "abc", term }]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// A terminal frame of the console channel: the tag, the 8-byte session id, the bytes.
function terminalFrame(id, text) {
  const frame = new Uint8Array(9 + text.length);
  frame[0] = TAG_TERMINAL;
  new DataView(frame.buffer).setBigUint64(1, BigInt(id));
  frame.set(Buffer.from(text, "utf8"), 9);
  return frame;
}

test("an OSC 52 copy is refused while the terminal replays or watches, as it is now", () => {
  const page = fakePage();
  try {
    const { deps, copies } = fakeDeps(page);
    const copy = "c;" + Buffer.from("x").toString("base64");
    // A reattach: the backlog replays first, so a copy in it is refused.
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { id: 5 });
    const osc = page.terminals[0].osc.get(52);
    osc(copy);
    assert.deepEqual(copies, []);
    // The replay is written: the next copy reaches the clipboard.
    page.channels[0].frame(terminalFrame(5, "a"));
    osc(copy);
    assert.equal(copies.length, 1);
    handle.dispose();

    // A watcher: the replay ends, and the copy is still refused.
    const watcher = createTerminal(deps).attachTerminal(fakeElement(), { id: 6, watch: true });
    page.channels[1].frame(terminalFrame(6, "a"));
    page.terminals[1].osc.get(52)(copy);
    assert.equal(copies.length, 1);
    watcher.dispose();
  } finally {
    page.restore();
  }
});

test("a pasted image is dropped under the terminal's repo", async () => {
  const page = fakePage();
  const savedReader = globalThis.FileReader;
  globalThis.FileReader = class {
    readAsDataURL() {
      this.result = "data:image/png;base64,AAAA";
      this.onload();
    }
  };
  try {
    const writes = [];
    const { deps } = fakeDeps(page, {
      window: {
        WBDaemon: {
          write: (verb, args) => {
            writes.push({ verb, args });
            return Promise.resolve({ path: "/tmp/drop.png" });
          },
        },
      },
    });
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { repo: "acme/app", agent: "claude" });
    const pasted = [];
    page.terminals[0].paste = (text) => pasted.push(text);
    assert.equal(handle.pasteImage({ type: "image/png", size: 3 }), true);
    await Promise.resolve();
    assert.deepEqual(writes, [{ verb: "image.write", args: { repo: "acme/app", base64: "AAAA" } }]);
    assert.deepEqual(pasted, ["/tmp/drop.png"]);
    handle.dispose();
  } finally {
    globalThis.FileReader = savedReader;
    page.restore();
  }
});

test("a deps member left out fails the call that needs it", () => {
  const page = fakePage();
  try {
    const noFont = fakeDeps(page).deps;
    delete noFont.fontSize;
    assert.throws(() => createTerminal(noFont).attachTerminal(fakeElement(), { console: true }), TypeError);

    const noCopy = fakeDeps(page).deps;
    delete noCopy.writeClipboard;
    const handle = createTerminal(noCopy).attachTerminal(fakeElement(), { console: true });
    const osc = page.terminals[page.terminals.length - 1].osc.get(52);
    assert.throws(() => osc("c;" + Buffer.from("x").toString("base64")), TypeError);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("detachSocket retires a channel: an open channel says why, and it closes", () => {
  const page = fakePage();
  try {
    const { detachSocket } = createTerminal(fakeDeps(page).deps);
    const channel = (readyState) => ({
      readyState,
      sent: [],
      closes: 0,
      send(data) {
        this.sent.push(data);
        return true;
      },
      close() {
        this.closes += 1;
      },
    });

    const opened = channel(1);
    detachSocket(opened, "reconnect");
    assert.deepEqual(opened.sent, [encodeDetach("reconnect")]);
    assert.equal(opened.closes, 1);

    // A channel that waits for the page socket cannot send: it only closes.
    const waiting = channel(0);
    detachSocket(waiting, "reconnect");
    assert.deepEqual(waiting.sent, []);
    assert.equal(waiting.closes, 1);

    // No channel yet: nothing to retire.
    assert.doesNotThrow(() => detachSocket(null, "reconnect"));
  } finally {
    page.restore();
  }
});

// A command frame of the console channel, as the daemon pushes it.
function commandFrame(verb, payload) {
  const body = Buffer.from(JSON.stringify({ id: 0, verb, payload }), "utf8");
  const frame = new Uint8Array(1 + body.length);
  frame[0] = TAG_COMMAND;
  frame.set(body, 1);
  return frame;
}

function gzipFrame(id, text) {
  const data = gzipSync(Buffer.from(text, "utf8"));
  const frame = new Uint8Array(9 + data.length);
  frame[0] = TAG_TERMINAL;
  new DataView(frame.buffer).setBigUint64(1, BigInt(id));
  frame.set(data, 9);
  return frame;
}

// Give `channel` the `session-open` of session 5, with `payload`.
function open(channel, payload) {
  channel.frame(commandFrame("session-open", { session: 5, ...payload }));
}

// The byte count a channel's open sent, or null when it sent none.
const sinceOf = (channel) => channel.payload.since ?? null;

// ADR-0079 D8: the terminal counts the session's bytes it shows, from the
// `offset` of `session-open` plus each live frame, and a reconnect sends it.
test("a reconnect sends the offset plus the live frames, and a resume keeps the terminal", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    assert.equal(sinceOf(page.channels[0]), null, "a new terminal shows nothing to resume");
    open(page.channels[0], { offset: 100, replay: true });
    page.channels[0].frame(terminalFrame(5, "SNAP"));
    page.channels[0].frame(terminalFrame(5, "abc"));
    assert.equal(handle.resume(true), true);
    assert.equal(sinceOf(page.channels[1]), 103, "the replay is in the offset already");

    open(page.channels[1], { offset: 110, replay: true, resume: true });
    page.channels[1].frame(terminalFrame(5, "defghij"));
    page.channels[1].frame(terminalFrame(5, "xy"));
    assert.equal(term.resets, 0, "a resume keeps what the terminal shows");
    assert.deepEqual(term.written.slice(-2), ["defghij", "xy"]);
    handle.takeOver();
    assert.equal(sinceOf(page.channels[2]), 112);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a reconnect that does not resume resets the terminal, and a peer with no offset gets no since", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    open(page.channels[0], { offset: 40, replay: true });
    assert.equal(term.resets, 0, "the first open of a new terminal clears nothing");
    handle.takeOver();
    assert.equal(sinceOf(page.channels[1]), 40);
    open(page.channels[1], { offset: 90, replay: true, resume: false });
    assert.equal(term.resets, 1, "a snapshot repaints a cleared terminal");
    handle.takeOver();
    assert.equal(sinceOf(page.channels[2]), 90, "the snapshot sets the count");

    // A peer of an older build: no offset, no resume.
    open(page.channels[2], { replay: true });
    assert.equal(term.resets, 2);
    handle.takeOver();
    assert.equal(sinceOf(page.channels[3]), null);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// A daemon whose live stream lagged sends `session-open` again on the same
// socket, with the screen of now: the terminal is cleared and takes the new count.
test("a second session-open on one socket resets the terminal and sets the count", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    open(page.channels[0], { offset: 40, replay: true });
    page.channels[0].frame(terminalFrame(5, "SNAP"));
    page.channels[0].frame(terminalFrame(5, "abc"));
    page.channels[0].frame(commandFrame("session-open", { session: 5, offset: 900, replay: true, resume: false }));
    page.channels[0].frame(terminalFrame(5, "SCREEN"));
    page.channels[0].frame(terminalFrame(5, "de"));
    assert.equal(term.resets, 1, "the screen of now replaces what the terminal shows");
    handle.takeOver();
    assert.equal(sinceOf(page.channels[1]), 902);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a notice of the page in the terminal forgets the offset", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    open(page.channels[0], { offset: 40 });
    handle.term.write("a notice");
    handle.takeOver();
    assert.equal(sinceOf(page.channels[1]), null, "the terminal no longer matches the session");
    handle.dispose();
  } finally {
    page.restore();
  }
});

// The link notice is a strip beside the terminal: the terminal still matches
// the session, so the reconnect resumes.
test("a dropped link keeps the count, writes nothing, and the strip shows until a channel opens", () => {
  const page = fakePage();
  try {
    const links = [];
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      id: 5,
      onLink: (state) => links.push(state),
    });
    open(page.channels[0], { offset: 40, replay: true });
    page.channels[0].frame(terminalFrame(5, "SNAP"));
    page.channels[0].frame(terminalFrame(5, "abc"));
    const written = page.terminals[0].written.length;
    page.dropSocket();
    assert.deepEqual(links, ["lost"]);
    assert.equal(page.channels.length, 2, "the open waits for the page socket");
    assert.equal(sinceOf(page.channels[1]), 43, "the count is kept");
    assert.equal(page.terminals[0].written.length, written, "nothing is written to the terminal");
    page.reopenSocket();
    open(page.channels[1], { offset: 43, replay: true, resume: true });
    assert.deepEqual(links, ["lost", "back"]);
    handle.dispose();
    assert.deepEqual(links, ["lost", "back"], "a link that is back is not told again");
  } finally {
    page.restore();
  }
});

// A first connect while the page socket cannot open has no channel that
// ends, so the handshake deadline is what shows the strip.
test("a first connect that still waits at the handshake deadline shows the strip until its open", () => {
  const page = fakePage();
  try {
    page.dropSocket();
    const links = [];
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      id: 5,
      onLink: (state) => links.push(state),
    });
    assert.equal(page.channels[0].readyState, 0, "the open waits for the page socket");
    mock.timers.tick(CONNECT_TIMEOUT_MS - 1);
    assert.deepEqual(links, [], "the strip waits for the deadline");
    mock.timers.tick(1);
    assert.deepEqual(links, ["lost"]);
    assert.equal(page.channels.length, 1, "the deadline opens no second channel");
    page.reopenSocket();
    open(page.channels[0], { offset: 0, replay: true });
    assert.deepEqual(links, ["lost", "back"]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// #751: a key typed while the socket is down brings the reconnect forward
// and shows the link strip; it never vanishes in silence.
test("a key typed during the backoff reconnects now, and a key while the open waits opens no second channel", () => {
  const page = fakePage();
  try {
    const links = [];
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      id: 5,
      onLink: (state) => links.push(state),
    });
    open(page.channels[0], { offset: 40, replay: true });
    // The daemon ends the channel and names no reason: a flaky link.
    page.channels[0].end("done");
    assert.equal(page.channels.length, 1, "the backoff waits");
    page.terminals[0].type("x");
    assert.equal(page.channels.length, 2, "the key reconnects now");
    assert.equal(sinceOf(page.channels[1]), 40, "the count is kept");
    // The page socket goes: the next open waits for it.
    page.dropSocket();
    assert.equal(page.channels.length, 3);
    page.terminals[0].type("y");
    assert.equal(page.channels.length, 3, "an open that waits is already the reconnect");
    assert.deepEqual(links, ["lost"]);
    page.reopenSocket();
    open(page.channels[2], { offset: 40, replay: true, resume: true });
    assert.deepEqual(links, ["lost", "back"]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a gzip tail is not counted, and the live frames after it are", async () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    open(page.channels[0], { offset: 7 });
    handle.takeOver();
    open(page.channels[1], { offset: 20, replay: true, resume: true, replay_encoding: "gzip" });
    page.channels[1].frame(gzipFrame(5, "the missing bytes"));
    page.channels[1].frame(terminalFrame(5, "live"));
    // The decode runs on real promises and a real worker thread; the mocked
    // timers do not hold it. A count of turns was too few on a slow CI runner.
    const until = performance.now() + 5000;
    while (!page.terminals[0].written.includes("live") && performance.now() < until) await new Promise(setImmediate);
    assert.deepEqual(page.terminals[0].written.slice(-2), ["the missing bytes", "live"]);
    handle.takeOver();
    assert.equal(sinceOf(page.channels[2]), 24);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// A launch that joined a session another page drives is read-only from the
// start: the daemon says so in `session-open`, and the window is told once.
test("a session-open that says watch makes the terminal a watcher, and tells the window once", () => {
  const page = fakePage();
  try {
    const parks = [];
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      console: true,
      onPark: (why) => parks.push(why),
    });
    const term = page.terminals[0];
    open(page.channels[0], { watch: true });
    assert.equal(handle.watching, true);
    assert.deepEqual(parks, ["joined"]);
    assert.ok(term.written.includes("\r\n[read-only: another window has control]\r\n"), JSON.stringify(term.written));
    page.channels[0].frame(commandFrame("session-open", { session: 5, watch: true }));
    assert.deepEqual(parks, ["joined"], "a watcher is not told again");

    // A session-open without `watch` leaves a writer as it is.
    const writer = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      console: true,
      onPark: (why) => parks.push(why),
    });
    open(page.channels[1], {});
    assert.equal(writer.watching, false);
    assert.deepEqual(parks, ["joined"]);
    writer.dispose();
    handle.dispose();
  } finally {
    page.restore();
  }
});

// A watcher paints at the size of the session it watches.
test("a session-size frame resizes a watcher's terminal to the session", () => {
  const page = fakePage();
  try {
    const body = { ...fakeElement(), classList: { toggle() {} } };
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(body, { id: 5, watch: true });
    const term = page.terminals[0];
    const sizes = [];
    term.resize = (cols, rows) => sizes.push([cols, rows]);
    open(page.channels[0], { replay: true });
    page.channels[0].frame(commandFrame("session-size", { rows: 30, cols: 100 }));
    assert.deepEqual(sizes, [[100, 30]]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// The daemon names why it ends a session before it closes the socket; the
// close is read with that reason, so the window ends instead of reconnecting.
test("a session-end frame names the reason the close is read with", () => {
  const page = fakePage();
  try {
    const ends = [];
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      id: 5,
      onEnded: (reason, message) => ends.push([reason, message]),
    });
    open(page.channels[0], { replay: true });
    page.channels[0].frame(commandFrame("session-end", { reason: "refused", message: "no such project" }));
    page.channels[0].end("done");
    assert.deepEqual(ends, [["refused", "no such project"]]);
    mock.timers.tick(20000);
    assert.equal(page.channels.length, 1, "an ended session is not reconnected");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("notePage tells an open channel that the page went hidden or came back, and a waiting one nothing", () => {
  const page = fakePage();
  try {
    page.dropSocket();
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const channel = page.channels[0];
    handle.notePage(true);
    assert.deepEqual(channel.sent, [], "a channel that waits for the page socket cannot send");
    page.reopenSocket();
    handle.notePage(true);
    handle.notePage(false);
    assert.deepEqual(channel.sent, [encodeDetach("hidden"), encodeShown()]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("the channel open names the network the browser reports, and nothing when it reports none", () => {
  const saved = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const navigatorWith = (value) => Object.defineProperty(globalThis, "navigator", { value, configurable: true });
  const page = fakePage();
  try {
    navigatorWith({ connection: { effectiveType: "3g", rtt: 300, downlink: 0.7 } });
    const chromium = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const { net, net_rtt, net_down } = page.channels[0].payload;
    assert.deepEqual([net, net_rtt, net_down], ["3g", "300", "0.7"]);
    chromium.dispose();
    navigatorWith({});
    const safari = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    assert.ok(!Object.keys(page.channels[1].payload).some((k) => k.startsWith("net")));
    safari.dispose();
  } finally {
    page.restore();
    if (saved) Object.defineProperty(globalThis, "navigator", saved);
    else delete globalThis.navigator;
  }
});

test("a console that gives up reports a stall", () => {
  const page = fakePage();
  const prev = globalThis.fetch;
  const reports = [];
  globalThis.fetch = (_url, init) => {
    reports.push(JSON.parse(init.body));
    return Promise.resolve(new Response(null, { status: 204 }));
  };
  forgetStalls();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 6, onEnded() {} });
    open(page.channels[0], { replay: true });
    page.channels[0].frame(commandFrame("session-end", { reason: "refused", message: "no such project" }));
    page.channels[0].end("done");
    assert.deepEqual(
      reports.map((r) => [r.kind, r.what]),
      [["console-gave-up", "refused"]],
    );
    handle.dispose();
  } finally {
    globalThis.fetch = prev;
    page.restore();
  }
});

// ADR-0082 D3: a console is a `console` channel of the page socket, and its
// open payload carries what a reattach resumes by (ADR-0079 D8).
test("a console opens a channel with its resume fields", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5, repo: "o/r" });
    open(page.channels[0], { offset: 100, replay: true });
    page.channels[0].frame(terminalFrame(5, "SNAP"));
    page.channels[0].frame(terminalFrame(5, "abcd"));
    page.dropSocket();
    const { repo, id, since, holder } = page.channels[1].payload;
    assert.deepEqual({ repo, id, since }, { repo: "o/r", id: 5, since: 104 });
    assert.match(holder, /^[A-Za-z0-9_-]{1,64}$/);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// ADR-0082 D4: after a resume the page holds one socket, and each awake
// console sends one open on it, with the count of the bytes it shows.
test("a reopened page socket resumes every console by its byte count", () => {
  const page = fakePage();
  try {
    const handles = [5, 6, 7].map((id, i) => {
      const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id });
      open(page.channels[i], { session: id, offset: 10 * (i + 1), replay: true });
      page.channels[i].frame(terminalFrame(id, "SNAP"));
      page.channels[i].frame(terminalFrame(id, "x".repeat(i + 1)));
      return handle;
    });
    page.dropSocket();
    page.reopenSocket();
    const opens = page.channels.slice(3);
    assert.deepEqual(
      opens.map((channel) => [channel.payload.id, channel.payload.since, channel.readyState]),
      [
        [5, 11, 1],
        [6, 22, 1],
        [7, 33, 1],
      ],
      "one open each, with its own count",
    );
    opens.forEach((channel, i) => {
      open(channel, { session: 5 + i, offset: 11 * (i + 1), replay: true, resume: true });
      channel.frame(terminalFrame(5 + i, "new"));
    });
    assert.deepEqual(
      page.terminals.map((term) => [term.resets, term.written.at(-1)]),
      [
        [0, "new"],
        [0, "new"],
        [0, "new"],
      ],
      "each terminal keeps what it shows",
    );
    mock.timers.tick(20000);
    assert.equal(page.channels.length, 6, "no second open for any console");
    for (const handle of handles) handle.dispose();
  } finally {
    page.restore();
  }
});

// ADR-0082 D2: one channel per console, and the input is sent at once, not
// behind the output that arrives on the same channel.
test("input goes out on the console channel while output arrives", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    const channel = page.channels[0];
    open(channel, { offset: 0 });
    const typed = () => channel.sent.filter((frame) => frame[0] === TAG_TERMINAL);
    channel.frame(terminalFrame(5, "output 1"));
    page.terminals[0].type("q");
    assert.deepEqual(typed(), [encodeTerminal("q")], "the key went out before the next output");
    channel.frame(terminalFrame(5, "output 2"));
    page.terminals[0].type("w");
    assert.deepEqual(typed(), [encodeTerminal("q"), encodeTerminal("w")]);
    assert.deepEqual(page.terminals[0].written.slice(-2), ["output 1", "output 2"]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

// A refused channel is a reopen that failed: three in a row before any
// `session-open`, and the console settles for watching.
test("a refused channel counts as a failed reopen", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), { id: 5 });
    for (let i = 0; i < 3; i += 1) {
      page.channels[i].end("refused");
      mock.timers.tick(20000);
    }
    assert.equal(page.channels.length, 4);
    assert.deepEqual(
      page.channels.map((channel) => channel.payload.watch ?? 0),
      [0, 0, 0, 1],
      "the third failed reopen parks the console as a watcher",
    );
    handle.dispose();
  } finally {
    page.restore();
  }
});
