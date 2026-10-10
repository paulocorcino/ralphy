// Unit tests for assets/ui/wb-api.ts — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { apiFetch } from "../assets/ui/wb-api.ts";

test("apiFetch sends the route's method to its path, with the query encoded", async () => {
  // [case, route, init, url, method]
  const rows = [
    ["no query", "GET /api/repos", undefined, "/api/repos", "GET"],
    ["an empty query", "GET /api/agents", { query: {} }, "/api/agents", "GET"],
    ["a value to encode", "GET /api/agents", { query: { repo: "peer/repo" } }, "/api/agents?repo=peer%2Frepo", "GET"],
    [
      "two values, in order",
      "GET /api/spend",
      { query: { project: "o/r", period: "7 d" } },
      "/api/spend?project=o%2Fr&period=7%20d",
      "GET",
    ],
    ["a number", "GET /api/audit/events", { query: { device: "ab", limit: 50 } }, "/api/audit/events?device=ab&limit=50", "GET"],
    ["a key that names its parameter", "GET /api/desk/history?id", { query: { id: 7 } }, "/api/desk/history?id=7", "GET"],
    ["another method", "PUT /api/desk", { query: { tab: "t&1" } }, "/api/desk?tab=t%261", "PUT"],
  ];
  const prev = globalThis.fetch;
  const calls = [];
  const answer = { ok: true, status: 200 };
  globalThis.fetch = (url, init) => {
    calls.push({ url, init });
    return Promise.resolve(answer);
  };
  try {
    for (const [name, route, init, url, method] of rows) {
      calls.length = 0;
      const r = await apiFetch(route, init);
      assert.equal(r, answer, `${name}: the answer of fetch, as it came`);
      assert.deepEqual(calls, [{ url, init: { method } }], name);
    }
    // The rest of the init goes to fetch as it was given.
    calls.length = 0;
    await apiFetch("POST /api/desk/history", { headers: { "Content-Type": "application/json" }, body: "{}" });
    assert.deepEqual(calls, [
      {
        url: "/api/desk/history",
        init: { headers: { "Content-Type": "application/json" }, body: "{}", method: "POST" },
      },
    ]);
  } finally {
    globalThis.fetch = prev;
  }
});
