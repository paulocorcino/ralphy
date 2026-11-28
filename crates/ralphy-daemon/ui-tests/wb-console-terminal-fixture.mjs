// The fakes of the terminal tests: `createTerminal` is driven with fake
// `deps`, a fake channel opener, and fake xterm and observer classes, with
// no console and no DOM.
// oxlint-disable-next-line eslint/max-classes-per-file -- a fake class per object the test stands in for
import { mock } from "node:test";
import { TAG_TERMINAL } from "../assets/ui/wb-console-session.ts";

// An element that takes listeners and does nothing with them.
export function fakeElement() {
  return { addEventListener() {}, closest: () => null };
}

// The page socket's door for consoles, faked: each `openConsole` call is one
// channel, which records its open payload and what the terminal sent on it.
// A channel opened while the page socket is down waits (readyState 0) and
// opens when the socket comes back.
export function fakeDoor() {
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
export function fakePage() {
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
    FitAddon: {
      FitAddon: class {
        fit() {}
      },
    },
    WebLinksAddon: {
      WebLinksAddon: class {
        dispose() {}
      },
    },
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
export function fakeDeps(page, more = {}) {
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

// A terminal frame of the console channel: the tag, the 8-byte session id, the bytes.
export function terminalFrame(id, text) {
  const frame = new Uint8Array(9 + text.length);
  frame[0] = TAG_TERMINAL;
  new DataView(frame.buffer).setBigUint64(1, BigInt(id));
  frame.set(Buffer.from(text, "utf8"), 9);
  return frame;
}
