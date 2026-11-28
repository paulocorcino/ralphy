// Unit tests for assets/ui/wb-console-terminal.ts: the terminal's options,
// its clipboard and paste, and its deps.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createTerminal } from "../assets/ui/wb-console-terminal.ts";
import { encodeDetach } from "../assets/ui/wb-console-session.ts";
import { fakeDeps, fakeElement, fakePage, terminalFrame } from "./wb-console-terminal-fixture.mjs";

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
    const handle = createTerminal(fakeDeps(page).deps).attachTerminal(fakeElement(), {
      console: true,
    });
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
    const handle = createTerminal(deps).attachTerminal(fakeElement(), {
      repo: "acme/app",
      agent: "claude",
    });
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
    assert.throws(
      () => createTerminal(noFont).attachTerminal(fakeElement(), { console: true }),
      TypeError,
    );

    const noCopy = fakeDeps(page).deps;
    delete noCopy.writeClipboard;
    const handle = createTerminal(noCopy).attachTerminal(fakeElement(), { console: true });
    const osc = page.terminals.at(-1).osc.get(52);
    assert.throws(() => osc("c;" + Buffer.from("x").toString("base64")), TypeError);
    handle.dispose();
  } finally {
    page.restore();
  }
});

const fakeChannel = (readyState) => ({
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

test("detachSocket retires a channel: an open channel says why, and it closes", () => {
  const page = fakePage();
  try {
    const { detachSocket } = createTerminal(fakeDeps(page).deps);

    const opened = fakeChannel(1);
    detachSocket(opened, "reconnect");
    assert.deepEqual(opened.sent, [encodeDetach("reconnect")]);
    assert.equal(opened.closes, 1);

    // A channel that waits for the page socket cannot send: it only closes.
    const waiting = fakeChannel(0);
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
