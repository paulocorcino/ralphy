// Unit tests for assets/ui/wb-device.ts — the device facts a page reports for
// the audit log (ADR-0074). Runs the real module against stub windows.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
import { WBDevice } from "../assets/ui/wb-device.ts";

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

// Importing the module sends nothing: only `report` does.
function load() {
  return WBDevice;
}

// One macrotask. `report` awaits only promises that are already settled (the
// stub window has no real I/O), so every send it started has run its course
// when this resolves.
function settle() {
  return new Promise((r) => setImmediate(r));
}

// Waits for `cond`, bounded, so a broken `report` fails instead of hanging.
async function until(cond, what) {
  for (let i = 0; i < 1000 && !cond(); i++) await settle();
  assert.ok(cond(), `timed out waiting for ${what}`);
}

test("collect: a window with no API reports null facts and never throws", async () => {
  const facts = await load().collect({});
  assert.equal(facts.ua, null);
  assert.equal(facts.ua_data, null);
  assert.equal(facts.gpu, null);
  assert.equal(facts.voices, null);
  assert.equal(facts.battery, null);
  assert.equal(facts.media.pointer, null);
  assert.equal(facts.engine_signals.ios_standalone, null);
});

test("collect: the voices are a count and the first five, never the list", async () => {
  const names = Array.from({ length: 9 }, (_, i) => ({ name: `v${i}`, lang: "pt-BR", localService: true }));
  const win = {
    navigator: { userAgent: "UA", maxTouchPoints: 5 },
    speechSynthesis: { getVoices: () => names },
  };
  const facts = await load().collect(win);
  assert.equal(facts.ua, "UA");
  assert.equal(facts.touch_points, 5);
  assert.deepEqual(facts.voices, {
    count: 9,
    sample: ["v0|pt-BR|1", "v1|pt-BR|1", "v2|pt-BR|1", "v3|pt-BR|1", "v4|pt-BR|1"],
  });
});

test("report: a 401 before login waits for the login action, then sends once", async () => {
  const D = load();
  const statuses = [401, 204, 204];
  const bodies = [];
  let onAction = null;
  const win = {
    document: {
      addEventListener(type, fn) {
        if (type === "workbench:action") onAction = fn;
      },
    },
    fetch: async (url, init) => {
      bodies.push(JSON.parse(init.body));
      return { ok: statuses.shift() < 300 };
    },
  };
  D.report(win);
  await until(() => bodies.length === 1, "the first send");
  await settle();
  assert.equal(bodies.length, 1, "the load sends once");
  onAction({ detail: { action: "toast" } });
  await settle();
  assert.equal(bodies.length, 1, "another action sends nothing");
  onAction({ detail: { action: "login" } });
  await until(() => bodies.length === 2, "the send after login");
  assert.match(bodies[0].holder, /^[A-Za-z0-9_-]{1,64}$/, "the facts name the tab's holder");
  assert.equal(bodies[1].holder, bodies[0].holder);
  await settle();
  onAction({ detail: { action: "login" } });
  await settle();
  assert.equal(bodies.length, 2, "a sent report is not sent again");
});

// The retries wait on the window's own `setTimeout`, so the test holds each
// one and runs it: no real time passes, and a retry that is not scheduled is
// seen as an empty list.
test("report: a 409 before the device cookie sends again later, a few times at most", async () => {
  const D = load();
  const statuses = [409, 409, 204];
  let sends = 0;
  const timers = [];
  D.report({
    document: { addEventListener() {} },
    setTimeout: (fn) => timers.push(fn),
    fetch: async () => {
      sends += 1;
      const status = statuses.shift();
      return { ok: status < 300, status };
    },
  });
  for (let n = 1; n <= 2; n++) {
    await until(() => timers.length === 1, `retry ${n}`);
    timers.shift()();
  }
  await until(() => sends === 3, "the third send");
  await settle();
  assert.equal(timers.length, 0, "two refusals, then the report, and no more retries");

  let refused = 0;
  const held = [];
  D.report({
    document: { addEventListener() {} },
    setTimeout: (fn) => held.push(fn),
    fetch: async () => {
      refused += 1;
      return { ok: false, status: 409 };
    },
  });
  for (let n = 1; n <= 5; n++) {
    await until(() => held.length === 1, `retry ${n}`);
    held.shift()();
  }
  await until(() => refused === 6, "the sixth send");
  await settle();
  assert.equal(held.length, 0, "the first send and five more, then it stops");
});

// The body the daemon reads (`fixtures/api-device-facts--report.json`, a
// measured Android phone): the page sends exactly its fields, so a field
// renamed here is not dropped without a word on the other side.
test("collect: the report has the fields of the shared request body", async () => {
  const body = fixture("api-device-facts--report");
  const facts = { ...(await load().collect({})), holder: null };
  assert.deepEqual(Object.keys(facts).sort(), Object.keys(body).sort());
  assert.deepEqual(Object.keys(facts.media).sort(), Object.keys(body.media).sort());
  assert.deepEqual(Object.keys(facts.engine_signals).sort(), Object.keys(body.engine_signals).sort());
});
