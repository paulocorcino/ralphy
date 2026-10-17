// Unit tests for assets/ui/wb-api.ts — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { apiFetch, ReadDeadline } from "../assets/ui/wb-api.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";

// Let every pending promise callback run; setImmediate is not under mock timers.
const flush = () => new Promise((r) => setImmediate(r));

// The abort signal is the deadline's own; the rest of the init is what the caller gave.
function withoutSignal(calls) {
  return calls.map(({ url, init: { signal, ...init } }) => {
    assert.ok(signal instanceof AbortSignal, "every request carries the deadline's signal");
    return { url, init };
  });
}

// A fetch that never answers; the deadline is driven by mock timers.
function neverAnswers(route, init) {
  const prev = globalThis.fetch;
  globalThis.fetch = () => new Promise(() => {});
  mock.timers.enable({ apis: ["setTimeout"] });
  const out = { error: null };
  const done = apiFetch(route, init).catch((e) => {
    out.error = e;
  });
  return {
    out,
    done,
    restore: () => {
      mock.timers.reset();
      globalThis.fetch = prev;
    },
  };
}

test("apiFetch rejects a request with no answer at its deadline", async () => {
  const w = neverAnswers("GET /api/repos");
  try {
    mock.timers.tick(19999);
    await flush();
    assert.equal(w.out.error, null, "still waiting at 19999 ms");
    mock.timers.tick(1);
    await flush();
    assert.ok(w.out.error instanceof ReadDeadline, "rejected at 20000 ms");
  } finally {
    w.restore();
  }
});

test("apiFetch gives each route its deadline", async () => {
  // [case, route, init, deadline]
  const rows = [
    ["default route", "GET /api/repos", undefined, 20000],
    ["nudge", "POST /api/fleet/nudge", undefined, 40000],
    ["explicit deadline", "GET /api/repos", { deadlineMs: 500 }, 500],
  ];
  for (const [name, route, init, ms] of rows) {
    const w = neverAnswers(route, init);
    try {
      mock.timers.tick(ms - 1);
      await flush();
      assert.equal(w.out.error, null, `${name}: waits until ${ms - 1} ms`);
      mock.timers.tick(1);
      await flush();
      assert.ok(w.out.error instanceof ReadDeadline, `${name}: fails at ${ms} ms`);
    } finally {
      w.restore();
    }
  }
});

test("a read past its deadline fails with ReadDeadline naming what waited", async () => {
  const w = neverAnswers("GET /api/repos");
  try {
    mock.timers.tick(20000);
    await flush();
    assert.ok(w.out.error instanceof ReadDeadline);
    assert.equal(w.out.error.what, "GET /api/repos");
    assert.equal(w.out.error.waitedMs, 20000);
  } finally {
    w.restore();
  }
});

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
      assert.deepEqual(withoutSignal(calls), [{ url, init: { method } }], name);
    }
    // The rest of the init goes to fetch as it was given.
    calls.length = 0;
    await apiFetch("POST /api/desk/history", { headers: { "Content-Type": "application/json" }, body: "{}" });
    assert.deepEqual(withoutSignal(calls), [
      {
        url: "/api/desk/history",
        init: { headers: { "Content-Type": "application/json" }, body: "{}", method: "POST" },
      },
    ]);
  } finally {
    globalThis.fetch = prev;
  }
});

test("a read past its deadline reports a stall", async () => {
  const prev = globalThis.fetch;
  const stalls = [];
  globalThis.fetch = (url, init) => {
    if (url === "/api/device/stall") stalls.push(JSON.parse(init.body));
    return new Promise(() => {});
  };
  forgetStalls();
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const done = apiFetch("GET /api/repos").catch(() => {});
    mock.timers.tick(20000);
    await done;
    assert.deepEqual(
      stalls.map((s) => [s.kind, s.what, s.waited_ms]),
      [["read-deadline", "GET /api/repos", 20000]],
    );
  } finally {
    mock.timers.reset();
    globalThis.fetch = prev;
  }
});
