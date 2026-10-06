// A detached file popup comes home however it closes: the Re-attach button and
// the popup's unload both send `wb-reattach` with the edited bytes, and a popup
// that dies without an unload is found by the shell's `closed` poll. Driven
// through the module-level `message` listener and the poll app.ts registers at
// load; the popup is a fake window, since only the shell's fold is under test.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

const ORIGIN = "http://127.0.0.1:7431";
const DESC = {
  project: "owner/repo",
  path: "docs/a.md",
  ftype: "markdown",
  content: "at detach",
  checkout: null,
  encoding: "utf-8",
  bom: false,
};

function fakePopup() {
  return {
    closed: false,
    closeCalls: 0,
    posted: [],
    close() {
      this.closeCalls += 1;
      this.closed = true;
    },
    postMessage(m) {
      this.posted.push(m);
    },
  };
}

// A shell with one popup detached. `tick()` runs the `closed` poll once;
// `send(from, data)` delivers a message the way the browser would.
function detached() {
  const popup = fakePopup();
  let onMessage = null;
  let poll = null;
  const urls = [];
  const { state, window } = loadShell({
    window: {
      location: { protocol: "http:", host: "127.0.0.1:7431", pathname: "/", search: "", origin: ORIGIN },
      addEventListener: (type, fn) => type === "message" && (onMessage = fn),
      setInterval: (fn) => ((poll = fn), 1),
      clearInterval: () => (poll = null),
      open: (url) => (urls.push(url), popup),
    },
  });
  const opened = [];
  state.openTab = (d) => opened.push(d);
  state.closeTab = () => {};
  state.activate = () => {};
  window.getShell = () => state;
  state.detachFile(DESC);
  return {
    popup,
    urls,
    opened,
    tick: () => poll?.(),
    polling: () => poll !== null,
    send: (source, data) => onMessage({ origin: ORIGIN, source, data }),
  };
}

test("the popup's re-attach opens the tab with the popup's bytes, once", () => {
  const d = detached();
  assert.deepEqual(d.urls, ["popup"], "the daemon serves the popup at its route");
  const edited = { ...DESC, content: "edited in the popup" };
  d.send(d.popup, { type: "wb-reattach", desc: edited });
  assert.equal(d.opened.length, 1);
  assert.equal(d.opened[0].content, "edited in the popup");
  assert.equal(d.opened[0].title, "a.md");
  assert.equal(d.popup.closeCalls, 1, "the shell closes a popup that went home");
  // The button's own `window.close()` fires the popup's unload: a second
  // `wb-reattach` from the same window.
  d.send(d.popup, { type: "wb-reattach", desc: edited });
  assert.equal(d.opened.length, 1, "the second signal is dropped");
  assert.equal(d.polling(), false, "no popup left, no poll");
});

test("a popup that dies without a message comes home with the detach-time bytes", () => {
  const d = detached();
  d.popup.closed = true;
  d.tick();
  assert.equal(d.opened.length, 0, "the first tick only marks it: the unload message may still be queued");
  d.tick();
  assert.equal(d.opened.length, 1);
  assert.equal(d.opened[0].content, "at detach");
  d.tick();
  assert.equal(d.opened.length, 1);
});

test("the unload message wins over the poll when both arrive in the same tick", () => {
  const d = detached();
  d.popup.closed = true;
  d.tick();
  d.send(d.popup, { type: "wb-reattach", desc: { ...DESC, content: "edited" } });
  d.tick();
  assert.equal(d.opened.length, 1);
  assert.equal(d.opened[0].content, "edited");
});

// NEGATIVE CONTROL: membership in the shell's popup map is the authorisation.
test("a re-attach from a window the shell did not open is ignored", () => {
  const d = detached();
  d.send(fakePopup(), { type: "wb-reattach", desc: { ...DESC, content: "forged" } });
  assert.equal(d.opened.length, 0);
});
