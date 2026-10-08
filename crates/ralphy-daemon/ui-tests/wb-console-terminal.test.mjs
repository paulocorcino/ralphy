// Unit tests for assets/ui/wb-console-terminal.ts — the console's terminal and
// its session socket. `createTerminal` is driven with fake `deps` and fake
// xterm, observer and socket classes, with no console and no DOM.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { createTerminal } from "../assets/ui/wb-console-terminal.ts";
import { encodeDetach } from "../assets/ui/wb-console-session.ts";

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
      this.parser = { registerOscHandler: (code, handler) => this.osc.set(code, handler) };
      terminals.push(this);
    }
    loadAddon() {}
    open(body) {
      this.element = body;
    }
    write(_data, done) {
      done?.();
    }
    attachCustomKeyEventHandler() {}
    onData() {}
    onResize() {}
    focus() {}
    reset() {}
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

  const open = socket(1);
  detachSocket(open, "reconnect");
  assert.deepEqual(handlers(open), [null, null, null, null]);
  assert.deepEqual(open.sent, [encodeDetach("reconnect")]);
  assert.equal(open.closed, true);

  // A socket still connecting cannot send: it only closes.
  const connecting = socket(0);
  detachSocket(connecting, "reconnect");
  assert.deepEqual(handlers(connecting), [null, null, null, null]);
  assert.deepEqual(connecting.sent, []);
  assert.equal(connecting.closed, true);

  // No socket yet: nothing to retire.
  assert.doesNotThrow(() => detachSocket(null, "reconnect"));
});
