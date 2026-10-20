// Unit tests for assets/ui/wb-uploads-store.ts, the Alpine store `uploads`:
// the queue that sends each file to the daemon in parts (`file.upload`). It
// is built here without Alpine (ADR-0073 D3, D6), with a fake `send` that
// answers each part as the daemon would.
import { test } from "node:test";
import assert from "node:assert/strict";
import { PART_BYTES, refusalText, toBase64, uploadsStore } from "../assets/ui/wb-uploads-store.ts";

// `answer(payload, sent)` gives the reply to each part; `sent` lists every
// payload so far. The default is the daemon of a healthy project.
function store(answer = daemon()) {
  const sent = [];
  const flashed = [];
  const s = uploadsStore({
    messages: { flash: (m) => flashed.push(m) },
    send: async (payload) => {
      sent.push(payload);
      return answer(payload, sent);
    },
  });
  return { s, sent, flashed };
}

// A daemon that holds what it was sent and answers like `uploads::accept`.
function daemon() {
  const held = new Map();
  return (p) => {
    const have = held.get(p.id) || 0;
    if (p.offset !== have) return { status: "error", reason: "offset", expected: have };
    const end = have + Buffer.from(p.data, "base64").length;
    held.set(p.id, end);
    return end < p.total ? { status: "ok", received: end } : { status: "ok", path: p.path, result: "created" };
  };
}

const bytes = (n) => Uint8Array.from({ length: n }, (_, i) => i % 251);
const file = (n, name = "a.bin") => new File([bytes(n)], name);
const decode = (p) => Uint8Array.from(Buffer.from(p.data, "base64"));

test("a file larger than one part goes in parts, in order, and lands at its folder", async () => {
  const { s, sent } = store();
  await s.add([{ file: file(PART_BYTES + 3), repo: "o/r", folder: "docs", onConflict: "keep_both" }]);
  assert.deepEqual(
    sent.map((p) => [p.repo, p.path, p.offset, p.total, decode(p).length]),
    [
      ["o/r", "docs/a.bin", 0, PART_BYTES + 3, PART_BYTES],
      ["o/r", "docs/a.bin", PART_BYTES, PART_BYTES + 3, 3],
    ],
  );
  assert.equal(sent[0].id, sent[1].id, "one id for one file");
  assert.match(sent[0].id, /^[0-9a-f]{32}$/);
  assert.equal("on_conflict" in sent[0], false, "only the last part names the choice");
  assert.equal(sent[1].on_conflict, "keep_both");
  assert.deepEqual(decode(sent[1]), bytes(PART_BYTES + 3).subarray(PART_BYTES));
  assert.deepEqual(
    s.items.map((i) => [i.state, i.path, s.percent(i)]),
    [["done", "docs/a.bin", 100]],
  );
});

test("a part at a wrong offset is sent again from the offset the daemon names", async () => {
  const { s, sent } = store((p) =>
    p.offset === 0
      ? { status: "error", reason: "offset", expected: 2 }
      : { status: "ok", path: p.path, result: "created" },
  );
  await s.add([{ file: file(5), repo: "o/r", folder: "", onConflict: "fail" }]);
  assert.deepEqual(
    sent.map((p) => [p.offset, Array.from(decode(p))]),
    [
      [0, [0, 1, 2, 3, 4]],
      [2, [2, 3, 4]],
    ],
  );
  assert.equal(s.items[0].state, "done");
});

test("an offset named twice in a row stops the file", async () => {
  const { s, sent } = store(() => ({ status: "error", reason: "offset", expected: 0 }));
  await s.add([{ file: file(5), repo: "o/r", folder: "", onConflict: "fail" }]);
  assert.equal(sent.length, 2, "no endless loop");
  assert.equal(s.items[0].state, "failed");
});

test("a failed request stops the file, and Try again goes on from the confirmed offset", async () => {
  const healthy = daemon();
  let down = false;
  const { s, sent } = store((p, all) => {
    if (down) throw new Error("socket closed");
    const reply = healthy(p, all);
    if (reply.received) down = true;
    return reply;
  });
  await s.add([{ file: file(PART_BYTES * 2), repo: "o/r", folder: "", onConflict: "fail" }]);
  const [item] = s.items;
  assert.equal(item.state, "failed");
  assert.equal(item.sent, PART_BYTES);
  assert.match(item.error, /Try again/);

  down = false;
  await s.retry(item.key);
  assert.deepEqual(
    sent.map((p) => p.offset),
    [0, PART_BYTES, PART_BYTES],
    "the retry starts at the confirmed offset, not at 0",
  );
  assert.equal(item.state, "done");
  assert.equal(item.error, "");
});

test("files go one after another, each in its own project", async () => {
  const { s, sent } = store();
  await s.add([
    { file: file(3, "one.txt"), repo: "o/r", folder: "", onConflict: "fail" },
    { file: file(3, "two.txt"), repo: "o/other", folder: "x", onConflict: "replace" },
  ]);
  assert.deepEqual(
    sent.map((p) => [p.repo, p.path, p.on_conflict]),
    [
      ["o/r", "one.txt", "fail"],
      ["o/other", "x/two.txt", "replace"],
    ],
  );
});

test("cancel stops a file between two parts, and a reply after it changes nothing", async () => {
  let s;
  const healthy = daemon();
  const ref = store((p, all) => {
    const reply = healthy(p, all);
    if (p.offset === 0) s.cancel(s.items[0].key);
    return reply;
  });
  s = ref.s;
  await s.add([{ file: file(PART_BYTES * 3), repo: "o/r", folder: "", onConflict: "fail" }]);
  assert.equal(ref.sent.length, 1, "no part after the cancel");
  assert.equal(s.items[0].state, "cancelled");

  // The cancel lands while the last part is on its way: its reply is not a "done".
  const last = store((p, all) => {
    const reply = daemon()(p, all);
    last.s.cancel(last.s.items[0].key);
    return reply;
  });
  await last.s.add([{ file: file(3), repo: "o/r", folder: "", onConflict: "fail" }]);
  assert.equal(last.s.items[0].state, "cancelled");
  s.clearFinished();
  assert.deepEqual(s.items, []);
});

test("a name the daemon chose is said in a flash", async () => {
  const { s, flashed } = store((p) => ({ status: "ok", path: "r (1).pdf", result: "created" }));
  await s.add([{ file: file(1, "r.pdf"), repo: "o/r", folder: "", onConflict: "keep_both" }]);
  assert.deepEqual(flashed, ["Uploaded as r (1).pdf"]);
});

test("an older daemon, a limit and a conflict each have their own sentence", () => {
  assert.match(refusalText({ status: "error", message: "unknown verb" }), /older daemon/);
  assert.match(refusalText({ status: "error", reason: "too large" }), /upload limit/);
  assert.match(refusalText({ status: "error", reason: "conflict" }), /exists now/);
  assert.match(refusalText(null), /did not answer/);
});

test("toBase64 matches the base64 of the bytes past one slice", () => {
  const b = bytes(0x8000 * 2 + 5);
  assert.equal(toBase64(b), Buffer.from(b).toString("base64"));
});
