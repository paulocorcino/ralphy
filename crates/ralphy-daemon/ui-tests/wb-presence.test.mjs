// Unit tests for assets/ui/wb-presence.ts: the reports a page sends the
// daemon about the operator's presence (whether the tab is visible, and the
// last input), on the `/ws` socket.
import { test } from "node:test";
import assert from "node:assert/strict";
import { QUIET_MS, startPresence } from "../assets/ui/wb-presence.ts";

// A page with a clock, its document's listeners, and the reports it sent.
function page() {
  const listeners = new Map();
  const reports = [];
  const opens = [];
  let beat = null;
  const clock = { now: 1_000_000 };
  const window = {
    location: { origin: "https://ralphy.example" },
    setInterval: (fn, ms) => {
      assert.equal(ms, QUIET_MS);
      beat = fn;
      return 1;
    },
  };
  const document = {
    visibilityState: "visible",
    addEventListener: (name, fn, options) => {
      if (name !== "visibilitychange") assert.deepEqual(options, { capture: true, passive: true }, name);
      listeners.set(name, fn);
    },
  };
  startPresence(window, document, {
    send: (report) => {
      reports.push(report);
      return true;
    },
    onOpen: (fn) => opens.push(fn),
    now: () => clock.now,
  });
  const fire = (name) => listeners.get(name)();
  return { document, reports, opens, clock, fire, beat: () => beat() };
}

test("presence reports on a visibility change and on the first key after a quiet minute, not on every key", () => {
  const p = page();
  assert.equal(p.reports.length, 1, "one report at the start");
  assert.deepEqual(p.reports[0], { visible: true, lastInput: null, now: 1_000_000, origin: "https://ralphy.example" });

  p.clock.now += 1000;
  p.fire("keydown");
  assert.equal(p.reports.length, 2, "the first key is reported");
  assert.equal(p.reports[1].lastInput, p.clock.now);
  for (let n = 0; n < 30; n++) {
    p.clock.now += 1000;
    p.fire(n % 2 ? "pointerdown" : "keydown");
  }
  assert.equal(p.reports.length, 2, "not every key");

  p.clock.now += QUIET_MS;
  p.fire("touchstart");
  assert.equal(p.reports.length, 3, "the first input after a quiet minute");

  p.document.visibilityState = "hidden";
  p.fire("visibilitychange");
  assert.equal(p.reports.length, 4);
  assert.equal(p.reports[3].visible, false, "a hidden tab is reported");
});

test("presence reports each minute while visible, and when the socket opens", () => {
  const p = page();
  p.beat();
  assert.equal(p.reports.length, 2, "a visible tab reports each minute");
  p.document.visibilityState = "hidden";
  p.beat();
  assert.equal(p.reports.length, 2, "a hidden tab does not");
  assert.equal(p.opens.length, 1);
  p.opens[0]();
  assert.equal(p.reports.length, 3, "a new socket hears the page at once");
});
