// Unit tests for assets/ui/wb-console-probe.ts — the scroll probe (ADR-0084).
// `createScrollProbe` is driven with a fake clock, fake frames and a fake
// daemon, with no console and no DOM.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createScrollProbe, frameFigures, END_AFTER_MS } from "../assets/ui/wb-console-probe.ts";

// A page with a clock the test moves, frames the test runs, and a daemon that
// answers every `probe.scroll` with `status`.
function fakePage(status) {
  let now = 0;
  let frames = [];
  const sent = [];
  const probe = createScrollProbe({
    send: (records) => {
      sent.push(records);
      return Promise.resolve({ status, message: status === "ok" ? undefined : "unknown verb" });
    },
    now: () => now,
    requestAnimationFrame: (callback) => {
      frames.push(callback);
      return frames.length;
    },
  });
  // Moves the clock by `ms` and runs the frames that were asked for.
  const frame = (ms) => {
    now += ms;
    const due = frames;
    frames = [];
    for (const callback of due) callback(now);
  };
  return { probe, sent, frame, pending: () => frames.length };
}

const term = { rows: 40, cols: 90, buffer: { active: { baseY: 1200 } } };
const place = () => ({ view: "maximized", renderer: "dom", dpr: 3 });
// The microtasks of the fake daemon's replies.
const settle = () => new Promise((done) => setTimeout(done, 0));

test("frameFigures counts the slow frames, and those with output in them", () => {
  const figures = frameFigures([16, 17, 40, 16, 90], [false, true, true, false, false]);
  assert.deepEqual(figures, {
    frames: 5,
    slow_frames: 2,
    slow_with_output: 1,
    p50_ms: 17,
    p95_ms: 90,
    max_ms: 90,
  });
});

test("a scroll is one record, sent only after it ends", async () => {
  const page = fakePage("ok");
  const watch = page.probe.watch(term, place);
  await settle();
  assert.deepEqual(page.sent, [[]], "the first command only asks");
  watch.moved("touch");
  page.frame(16);
  watch.step(2);
  watch.wrote(500);
  page.frame(40);
  watch.moved("glide");
  page.frame(16);
  assert.equal(page.sent.length, 1, "nothing is sent while it scrolls");
  // Nothing moves any more: the frames run on until END_AFTER_MS.
  while (page.pending()) page.frame(16);
  await settle();
  assert.equal(page.sent.length, 2);
  const [record] = page.sent[1];
  assert.equal(record.input, "glide");
  assert.equal(record.slow_frames, 1);
  assert.equal(record.slow_with_output, 1);
  assert.equal(record.output_bytes, 500);
  assert.equal(record.step_ms, 2);
  assert.equal(record.history, 1200);
  assert.equal(record.view, "maximized");
  assert.ok(record.duration_ms >= 56 && record.duration_ms < 56 + END_AFTER_MS, String(record.duration_ms));
});

test("a daemon without the probe gets the ask and nothing more", async () => {
  const page = fakePage("error");
  const watch = page.probe.watch(term, place);
  await settle();
  watch.moved("wheel");
  while (page.pending()) page.frame(16);
  await settle();
  assert.deepEqual(page.sent, [[]]);
  assert.equal(page.pending(), 0, "no frame is asked for");
});
