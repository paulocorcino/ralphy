// Unit tests for assets/ui/wb-console-terminal.ts — the console's terminal and
// its session socket. `createTerminal` is driven with fake `deps` and fake
// xterm, observer and socket classes, with no console and no DOM.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { createTerminal } from "../assets/ui/wb-console-terminal.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";
import { encodeDetach, encodeShown, TAG_COMMAND, TAG_TERMINAL } from "../assets/ui/wb-console-session.ts";

// An element that takes listeners and does nothing with them.
function fakeElement() {
  return { addEventListener() {}, closest: () => null };
}

// The page globals `attachTerminal` reaches: xterm and its addons, the fit
// observer and the socket. Each fake records what the terminal did with it.
function fakePage() {
  const terminals = [];
  const sockets = [];
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
  class FakeSocket {
    static OPEN = 1;
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      sockets.push(this);
    }
    send() {}
    close() {
      this.readyState = 3;
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
    WebSocket: FakeSocket,
  };
  const saved = Object.fromEntries(Object.keys(globals).map((k) => [k, globalThis[k]]));
  Object.assign(globalThis, globals);
  // The connect timeout is a real timer; a mocked one never holds the run open.
  mock.timers.enable({ apis: ["setTimeout"] });
  const restore = () => {
    mock.timers.reset();
    Object.assign(globalThis, saved);
  };
  return { terminals, sockets, restore };
}

// Every member of `TerminalDeps`; the clipboard write records its calls.
function fakeDeps(more = {}) {
  const copies = [];
  const deps = {
    window: {},
    WS_ORIGIN: "ws://127.0.0.1:7431",
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

test("a terminal takes its font size from deps, and its socket dials the deps origin", () => {
  const page = fakePage();
  try {
    const { deps } = fakeDeps({ fontSize: () => 17, WS_ORIGIN: "wss://example.test" });
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { console: true });
    assert.equal(page.terminals[0].options.fontSize, 17);
    assert.ok(
      page.sockets[0].url.startsWith("wss://example.test/ws/session?"),
      page.sockets[0].url,
    );
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a terminal counts character widths by Unicode 11, so an emoji takes two columns", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { console: true });
    assert.equal(page.terminals[0].unicode.activeVersion, "11");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("an OSC 52 copy reaches deps.writeClipboard with its control characters removed", () => {
  const page = fakePage();
  try {
    const { deps, copies } = fakeDeps();
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

// A terminal frame of the session socket: the tag, the 8-byte session id, the bytes.
function terminalFrame(id, text) {
  const frame = new Uint8Array(9 + text.length);
  frame[0] = TAG_TERMINAL;
  new DataView(frame.buffer).setBigUint64(1, BigInt(id));
  frame.set(Buffer.from(text, "utf8"), 9);
  return { data: frame.buffer };
}

test("an OSC 52 copy is refused while the terminal replays or watches, as it is now", () => {
  const page = fakePage();
  try {
    const { deps, copies } = fakeDeps();
    const copy = "c;" + Buffer.from("x").toString("base64");
    // A reattach: the backlog replays first, so a copy in it is refused.
    const handle = createTerminal(deps).attachTerminal(fakeElement(), { id: 5 });
    const osc = page.terminals[0].osc.get(52);
    osc(copy);
    assert.deepEqual(copies, []);
    // The replay is written: the next copy reaches the clipboard.
    page.sockets[0].onmessage(terminalFrame(5, "a"));
    osc(copy);
    assert.equal(copies.length, 1);
    handle.dispose();

    // A watcher: the replay ends, and the copy is still refused.
    const watcher = createTerminal(deps).attachTerminal(fakeElement(), { id: 6, watch: true });
    page.sockets[1].onmessage(terminalFrame(6, "a"));
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
    const { deps } = fakeDeps({
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
    const noFont = fakeDeps().deps;
    delete noFont.fontSize;
    assert.throws(() => createTerminal(noFont).attachTerminal(fakeElement(), { console: true }), TypeError);

    const noCopy = fakeDeps().deps;
    delete noCopy.writeClipboard;
    const handle = createTerminal(noCopy).attachTerminal(fakeElement(), { console: true });
    const osc = page.terminals[page.terminals.length - 1].osc.get(52);
    assert.throws(() => osc("c;" + Buffer.from("x").toString("base64")), TypeError);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("detachSocket retires a socket: no handler is left, an open socket says why, and it closes", () => {
  const { detachSocket } = createTerminal(fakeDeps().deps);
  const socket = (readyState) => {
    const ws = {
      readyState,
      sent: [],
      closed: false,
      send(data) {
        this.sent.push(data);
      },
      close() {
        this.closed = true;
      },
    };
    for (const h of ["onclose", "onmessage", "onopen", "onerror"]) ws[h] = () => {};
    return ws;
  };
  const handlers = (ws) => [ws.onclose, ws.onmessage, ws.onopen, ws.onerror];

  const connected = socket(1);
  detachSocket(connected, "reconnect");
  assert.deepEqual(handlers(connected), [null, null, null, null]);
  assert.deepEqual(connected.sent, [encodeDetach("reconnect")]);
  assert.equal(connected.closed, true);

  // A socket still connecting cannot send: it only closes.
  const connecting = socket(0);
  detachSocket(connecting, "reconnect");
  assert.deepEqual(handlers(connecting), [null, null, null, null]);
  assert.deepEqual(connecting.sent, []);
  assert.equal(connecting.closed, true);

  // No socket yet: nothing to retire.
  assert.doesNotThrow(() => detachSocket(null, "reconnect"));
});

// A command frame of the session socket, as the daemon pushes it.
function commandFrame(verb, payload) {
  const body = Buffer.from(JSON.stringify({ id: 0, verb, payload }), "utf8");
  const frame = new Uint8Array(1 + body.length);
  frame[0] = TAG_COMMAND;
  frame.set(body, 1);
  return { data: frame.buffer };
}

function gzipFrame(id, text) {
  const data = gzipSync(Buffer.from(text, "utf8"));
  const frame = new Uint8Array(9 + data.length);
  frame[0] = TAG_TERMINAL;
  new DataView(frame.buffer).setBigUint64(1, BigInt(id));
  frame.set(data, 9);
  return { data: frame.buffer };
}

// Open `socket` and give it `session-open` with `payload` for session 5.
function open(socket, payload) {
  socket.readyState = 1;
  socket.onopen();
  socket.onmessage(commandFrame("session-open", { session: 5, ...payload }));
}

const sinceOf = (socket) => new URL(socket.url).searchParams.get("since");

// ADR-0079 D8: the terminal counts the session's bytes it shows, from the
// `offset` of `session-open` plus each live frame, and a reconnect sends it.
test("a reconnect sends the offset plus the live frames, and a resume keeps the terminal", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    assert.equal(sinceOf(page.sockets[0]), null, "a new terminal shows nothing to resume");
    open(page.sockets[0], { offset: 100, replay: true });
    page.sockets[0].onmessage(terminalFrame(5, "SNAP"));
    page.sockets[0].onmessage(terminalFrame(5, "abc"));
    assert.equal(handle.resume(true), true);
    assert.equal(sinceOf(page.sockets[1]), "103", "the replay is in the offset already");

    open(page.sockets[1], { offset: 110, replay: true, resume: true });
    page.sockets[1].onmessage(terminalFrame(5, "defghij"));
    page.sockets[1].onmessage(terminalFrame(5, "xy"));
    assert.equal(term.resets, 0, "a resume keeps what the terminal shows");
    assert.deepEqual(term.written.slice(-2), ["defghij", "xy"]);
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[2]), "112");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a reconnect that does not resume resets the terminal, and a peer with no offset gets no since", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    open(page.sockets[0], { offset: 40, replay: true });
    assert.equal(term.resets, 0, "the first open of a new terminal clears nothing");
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[1]), "40");
    open(page.sockets[1], { offset: 90, replay: true, resume: false });
    assert.equal(term.resets, 1, "a snapshot repaints a cleared terminal");
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[2]), "90", "the snapshot sets the count");

    // A peer of an older build: no offset, no resume.
    open(page.sockets[2], { replay: true });
    assert.equal(term.resets, 2);
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[3]), null);
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
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    const term = page.terminals[0];
    open(page.sockets[0], { offset: 40, replay: true });
    page.sockets[0].onmessage(terminalFrame(5, "SNAP"));
    page.sockets[0].onmessage(terminalFrame(5, "abc"));
    page.sockets[0].onmessage(commandFrame("session-open", { session: 5, offset: 900, replay: true, resume: false }));
    page.sockets[0].onmessage(terminalFrame(5, "SCREEN"));
    page.sockets[0].onmessage(terminalFrame(5, "de"));
    assert.equal(term.resets, 1, "the screen of now replaces what the terminal shows");
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[1]), "902");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a notice of the page in the terminal forgets the offset", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    open(page.sockets[0], { offset: 40 });
    handle.term.write("a notice");
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[1]), null, "the terminal no longer matches the session");
    handle.dispose();
  } finally {
    page.restore();
  }
});

// The link notice is a strip beside the terminal: the terminal still matches
// the session, so the reconnect resumes.
test("a dropped link keeps the count, writes nothing, and the strip shows until a socket opens", () => {
  const page = fakePage();
  try {
    const links = [];
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), {
      id: 5,
      onLink: (state) => links.push(state),
    });
    open(page.sockets[0], { offset: 40, replay: true });
    page.sockets[0].onmessage(terminalFrame(5, "SNAP"));
    page.sockets[0].onmessage(terminalFrame(5, "abc"));
    const written = page.terminals[0].written.length;
    page.sockets[0].onclose({});
    assert.deepEqual(links, ["lost"]);
    mock.timers.tick(20000);
    assert.equal(page.sockets.length, 2, "the backoff reconnects");
    assert.equal(sinceOf(page.sockets[1]), "43", "the count is kept");
    assert.equal(page.terminals[0].written.length, written, "nothing is written to the terminal");
    open(page.sockets[1], { offset: 43, replay: true, resume: true });
    assert.deepEqual(links, ["lost", "back"]);
    handle.dispose();
    assert.deepEqual(links, ["lost", "back"], "a link that is back is not told again");
  } finally {
    page.restore();
  }
});

// #751: a key typed while the socket is down brings the reconnect forward
// and shows the link strip; it never vanishes in silence.
test("a key typed during the backoff reconnects now, and a key while connecting opens no second socket", () => {
  const page = fakePage();
  try {
    const links = [];
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), {
      id: 5,
      onLink: (state) => links.push(state),
    });
    open(page.sockets[0], { offset: 40, replay: true });
    page.sockets[0].readyState = 3;
    page.sockets[0].onclose({});
    assert.equal(page.sockets.length, 1, "the backoff waits");
    page.terminals[0].type("x");
    assert.equal(page.sockets.length, 2, "the key reconnects now");
    assert.equal(sinceOf(page.sockets[1]), "40", "the count is kept");
    page.terminals[0].type("y");
    assert.equal(page.sockets.length, 2, "a connecting socket is already the reconnect");
    assert.deepEqual(links, ["lost"]);
    open(page.sockets[1], { offset: 40, replay: true, resume: true });
    assert.deepEqual(links, ["lost", "back"]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("a gzip tail is not counted, and the live frames after it are", async () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    open(page.sockets[0], { offset: 7 });
    handle.takeOver();
    open(page.sockets[1], { offset: 20, replay: true, resume: true, replay_encoding: "gzip" });
    page.sockets[1].onmessage(gzipFrame(5, "the missing bytes"));
    page.sockets[1].onmessage(terminalFrame(5, "live"));
    // The decode runs on real promises and a real worker thread; the mocked
    // timers do not hold it. A count of turns was too few on a slow CI runner.
    const until = performance.now() + 5000;
    while (!page.terminals[0].written.includes("live") && performance.now() < until) await new Promise(setImmediate);
    assert.deepEqual(page.terminals[0].written.slice(-2), ["the missing bytes", "live"]);
    handle.takeOver();
    assert.equal(sinceOf(page.sockets[2]), "24");
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
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), {
      console: true,
      onPark: (why) => parks.push(why),
    });
    const term = page.terminals[0];
    open(page.sockets[0], { watch: true });
    assert.equal(handle.watching, true);
    assert.deepEqual(parks, ["joined"]);
    assert.ok(term.written.includes("\r\n[read-only: another window has control]\r\n"), JSON.stringify(term.written));
    page.sockets[0].onmessage(commandFrame("session-open", { session: 5, watch: true }));
    assert.deepEqual(parks, ["joined"], "a watcher is not told again");

    // A session-open without `watch` leaves a writer as it is.
    const writer = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), {
      console: true,
      onPark: (why) => parks.push(why),
    });
    open(page.sockets[1], {});
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
    const handle = createTerminal(fakeDeps().deps).attachTerminal(body, { id: 5, watch: true });
    const term = page.terminals[0];
    const sizes = [];
    term.resize = (cols, rows) => sizes.push([cols, rows]);
    open(page.sockets[0], { replay: true });
    page.sockets[0].onmessage(commandFrame("session-size", { rows: 30, cols: 100 }));
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
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), {
      id: 5,
      onEnded: (reason, message) => ends.push([reason, message]),
    });
    open(page.sockets[0], { replay: true });
    page.sockets[0].onmessage(commandFrame("session-end", { reason: "refused", message: "no such project" }));
    page.sockets[0].onclose({});
    assert.deepEqual(ends, [["refused", "no such project"]]);
    mock.timers.tick(20000);
    assert.equal(page.sockets.length, 1, "an ended session is not reconnected");
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("notePage tells an open socket that the page went hidden or came back, and a connecting one nothing", () => {
  const page = fakePage();
  try {
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    const ws = page.sockets[0];
    const sent = [];
    ws.send = (data) => sent.push(data);
    handle.notePage(true);
    assert.deepEqual(sent, [], "a socket still connecting cannot send");
    ws.readyState = 1;
    handle.notePage(true);
    handle.notePage(false);
    assert.deepEqual(sent, [encodeDetach("hidden"), encodeShown()]);
    handle.dispose();
  } finally {
    page.restore();
  }
});

test("the socket names the network the browser reports, and nothing when it reports none", () => {
  const saved = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  const navigatorWith = (value) => Object.defineProperty(globalThis, "navigator", { value, configurable: true });
  const page = fakePage();
  try {
    navigatorWith({ connection: { effectiveType: "3g", rtt: 300, downlink: 0.7 } });
    const chromium = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    assert.match(page.sockets[0].url, /&net=3g&net_rtt=300&net_down=0\.7$/);
    chromium.dispose();
    navigatorWith({});
    const safari = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 5 });
    assert.ok(!page.sockets[1].url.includes("net"), page.sockets[1].url);
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
    const handle = createTerminal(fakeDeps().deps).attachTerminal(fakeElement(), { id: 6, onEnded() {} });
    open(page.sockets[0], { replay: true });
    page.sockets[0].onmessage(commandFrame("session-end", { reason: "refused", message: "no such project" }));
    page.sockets[0].onclose({});
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
