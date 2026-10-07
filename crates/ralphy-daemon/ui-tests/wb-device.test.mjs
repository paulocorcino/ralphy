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
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(bodies.length, 1, "the load sends once");
  onAction({ detail: { action: "toast" } });
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(bodies.length, 1, "another action sends nothing");
  onAction({ detail: { action: "login" } });
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(bodies.length, 2, "the login sends again");
  assert.match(bodies[0].holder, /^[A-Za-z0-9_-]{1,64}$/, "the facts name the tab's holder");
  assert.equal(bodies[1].holder, bodies[0].holder);
  onAction({ detail: { action: "login" } });
  await new Promise((r) => setTimeout(r, 20));
  assert.equal(bodies.length, 2, "a sent report is not sent again");
});

test("report: a 409 before the device cookie sends again later, a few times at most", async () => {
  const D = load();
  const statuses = [409, 409, 204];
  let sends = 0;
  const win = {
    document: { addEventListener() {} },
    setTimeout: (fn) => setTimeout(fn, 0),
    fetch: async () => {
      sends += 1;
      const status = statuses.shift();
      return { ok: status < 300, status };
    },
  };
  D.report(win);
  await new Promise((r) => setTimeout(r, 50));
  assert.equal(sends, 3, "two refusals, then the report");

  let refused = 0;
  D.report({
    document: { addEventListener() {} },
    setTimeout: (fn) => setTimeout(fn, 0),
    fetch: async () => {
      refused += 1;
      return { ok: false, status: 409 };
    },
  });
  await new Promise((r) => setTimeout(r, 100));
  assert.equal(refused, 6, "the first send and five more, then it stops");
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
