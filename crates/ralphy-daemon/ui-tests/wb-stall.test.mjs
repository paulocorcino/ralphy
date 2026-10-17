// Unit tests for assets/ui/wb-stall.ts: the stall report the page posts to
// its daemon. No DOM; `fetch` is replaced, as the module reads it at call time.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { forgetStalls, reportStall, STALL_REPEAT_MS } from "../assets/ui/wb-stall.ts";

function fixture(name) {
  return JSON.parse(readFileSync(new URL(`fixtures/${name}.json`, import.meta.url), "utf8"));
}

function recordFetch() {
  const prev = globalThis.fetch;
  const calls = [];
  globalThis.fetch = (url, init) => {
    calls.push({ url, init });
    return Promise.resolve(new Response(null, { status: 204 }));
  };
  forgetStalls();
  return { calls, restore: () => (globalThis.fetch = prev) };
}

test("reportStall posts the stall to the daemon", async () => {
  const f = recordFetch();
  try {
    await reportStall("read-deadline", "GET /api/repos", 20000);
    assert.equal(f.calls.length, 1);
    const { url, init } = f.calls[0];
    assert.equal(url, "/api/device/stall");
    assert.equal(init.method, "POST");
    assert.equal(init.headers["content-type"], "application/json");
    const body = JSON.parse(init.body);
    // The body the daemon reads (`fixtures/api-device-stall--report.json`).
    assert.deepEqual(Object.keys(body).sort(), Object.keys(fixture("api-device-stall--report")).sort());
    assert.equal(body.kind, "read-deadline");
    assert.equal(body.what, "GET /api/repos");
    assert.equal(body.waited_ms, 20000);
    assert.equal(typeof body.holder, "string");
  } finally {
    f.restore();
  }
});

test("reportStall sends one report per kind and what each minute", async () => {
  const f = recordFetch();
  mock.timers.enable({ apis: ["Date", "setTimeout"], now: 1_000_000 });
  try {
    await reportStall("read-deadline", "GET /api/repos", 20000);
    await reportStall("read-deadline", "GET /api/repos", 20000);
    await reportStall("read-deadline", "GET /api/usage", 20000);
    mock.timers.tick(STALL_REPEAT_MS - 1);
    await reportStall("read-deadline", "GET /api/repos", 20000);
    mock.timers.tick(1);
    await reportStall("read-deadline", "GET /api/repos", 20000);
    const rows = f.calls.map((c) => JSON.parse(c.init.body).what);
    assert.deepEqual(rows, ["GET /api/repos", "GET /api/usage", "GET /api/repos"]);
  } finally {
    mock.timers.reset();
    f.restore();
  }
});
