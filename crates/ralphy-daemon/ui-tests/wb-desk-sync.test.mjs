// Unit tests for assets/ui/wb-desk-sync.ts — runs the real source with no DOM.
// The table of cases is the daemon's too (`desk::apply` tests): one rule in
// two languages, held by one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { WBDeskSync } from "../assets/ui/wb-desk-sync.ts";

const HERE = dirname(fileURLToPath(import.meta.url));

function load() {
  return WBDeskSync;
}

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

// The daemon's own reading of a desk: `ts` is its clock, which a page never
// sees, and an absent key, `null` and `false` are one value (an `Option` or a
// `bool` that is not serialised).
function normal(desk) {
  return JSON.parse(
    JSON.stringify(desk, (key, value) => (key === "ts" || value === null || value === false ? undefined : value)),
  );
}

test("every shared case gives the desk the daemon gives", () => {
  const S = load();
  const cases = fixture("api-desk--apply-cases");
  assert.ok(cases.length >= 15, `the table lost cases: ${cases.length}`);
  for (const c of cases) {
    const before = structuredClone(c.desk);
    let desk = c.desk;
    let changed = false;
    for (const change of c.changes) {
      const out = S.applyChange(desk, change);
      desk = out.desk;
      changed = changed || out.changed;
    }
    assert.deepEqual(normal(desk), normal(c.expect), c.name);
    assert.equal(changed, c.changed, `${c.name}: changed`);
    assert.deepEqual(c.desk, before, `${c.name}: the desk handed in is not mutated`);
  }
});

const W = (left, extra = {}) => ({
  id: "w-1",
  repo: "o/r",
  agent: "console",
  kind: "console",
  rect: { left, top: 0, width: 600, height: 400 },
  max: false,
  sessionId: 7,
  ...extra,
});
const moveTo = (left) => ({
  op: "set",
  type: "window",
  id: "w-1",
  fields: { rect: { left, top: 0, width: 600, height: 400 } },
});

// Figma's rule: a server value does not replace a change of this page the
// server has not answered. The view is the daemon's desk with this page's
// pending changes on top.
test("the view keeps a pending change over a newer desk, and takes the rest", () => {
  const s = load().createSync();
  // A change made before the desk is read queues, and the view holds it.
  assert.equal(s.phase(), "loading");
  s.emit({ op: "checkout", repo: "o/r", name: "mine" });
  assert.deepEqual(s.view().checkouts, { "o/r": "mine" });
  assert.equal(s.take({ rev: 1, windows: [W(100)], checkouts: { "o/r": "old", "o/s": "keep" } }), "taken");
  assert.deepEqual(s.view().checkouts, { "o/r": "mine", "o/s": "keep" });
  s.emit(moveTo(400));
  assert.equal(s.take({ rev: 2, windows: [W(200, { consoleName: "build" })] }), "taken");
  const w = s.view().windows[0];
  assert.equal(w.rect.left, 400, "this page's move stays until the daemon answers it");
  assert.equal(w.consoleName, "build", "another page's rename comes in");
  s.fail();
  assert.equal(s.phase(), "failed");
});

test("a desk with a lower rev than one already taken is not taken", () => {
  const s = load().createSync();
  s.take({ rev: 5, windows: [W(500)] });
  assert.equal(s.take({ rev: 3, windows: [W(300)] }), "stale");
  assert.equal(s.view().windows[0].rect.left, 500);
  assert.equal(s.take({ rev: 5, windows: [W(510)] }), "taken", "the same rev is taken again");
});

test("a newer generation means a restore: the page reloads and takes nothing more", () => {
  const s = load().createSync();
  s.take({ generation: 0, windows: [W(100)] });
  assert.equal(s.take({ generation: 9, windows: [] }), "reload");
  assert.equal(s.phase(), "restored");
  assert.equal(s.take({ generation: 9, windows: [] }), "ignored");
});

// The daemon ignores a `seq` it already took from a tab, so a batch resent
// after a lost reply is not applied twice.
test("a batch is resent with its seq until the daemon answers it, and the next batch counts on", () => {
  const s = load().createSync();
  assert.equal(s.nextBatch(), null, "nothing to send");
  s.take({ rev: 1, generation: 4, windows: [W(100)] });
  s.emit(moveTo(400));
  const first = s.nextBatch();
  assert.deepEqual(first, { seq: 1, generation: 4, changes: [moveTo(400)] });
  // A change made while the batch is on the wire waits for the next one.
  s.emit(moveTo(450));
  assert.deepEqual(s.nextBatch(), first, "a resend is the same batch with the same seq");
  assert.equal(s.acked({ rev: 2, generation: 4, windows: [W(400)] }), "taken");
  assert.deepEqual(s.nextBatch(), { seq: 2, generation: 4, changes: [moveTo(450)] });
  assert.equal(s.view().windows[0].rect.left, 450);
});

// `pagehide`: every change not yet answered, numbered past the batch on the
// wire, so that batch is ignored if it lands after this one.
test("the closing batch carries every pending change with a higher seq", () => {
  const s = load().createSync();
  s.take({ windows: [W(100)] });
  s.emit(moveTo(200));
  const onWire = s.nextBatch();
  s.emit(moveTo(300));
  const closing = s.closingBatch();
  assert.ok(closing.seq > onWire.seq);
  assert.deepEqual(closing.changes, [moveTo(200), moveTo(300)]);
});
