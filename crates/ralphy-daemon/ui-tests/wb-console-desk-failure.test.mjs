// A desk the daemon cannot read, and a full desk, in assets/ui/wb-console.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { idLookups } from "./fake-document.mjs";
import { swapFetch } from "./harness.mjs";
import { load } from "./wb-console-fixture.mjs";

// ADR-0070 D4: a desk the daemon cannot read is a failure with a reason, and
// this page never uploads over it.
test("an unreadable desk is a failure, and no flush PUTs over it", async (t) => {
  const calls = [];
  swapFetch(t, (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    return Promise.resolve({
      ok: false,
      status: 409,
      json: () => Promise.resolve({ state: "unreadable", error: "parsing desk layout C:/x/desk.toml" }),
    });
  });
  const c = load();
  await c.whenDeskLoaded();
  assert.equal(c.deskFailure(), "the file is damaged");
  c.setCheckout("o/r", "wt-a");
  await new Promise((r) => { setTimeout(r, 400); });
  assert.deepEqual(
    calls.filter((call) => call.method === "PUT"),
    [],
    "no PUT over a desk the daemon cannot read",
  );
});

// A desk that becomes unreadable after a good load: the daemon refuses the
// PUT, the page shows the failure and sends nothing more.
test("a PUT the daemon refuses as unreadable shows the failure and stops the sending", async (t) => {
  const calls = [];
  let unreadable = false;
  swapFetch(t, (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    if (unreadable) {
      return Promise.resolve({ ok: false, status: 409, json: () => Promise.resolve({ state: "unreadable", error: "parsing desk layout" }) });
    }
    return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({ windows: [], fences: [], notes: [] }) });
  });
  const c = load();
  await c.whenDeskLoaded();
  assert.equal(c.deskFailure(), "", "the first read was good");
  unreadable = true;
  calls.length = 0;
  c.setCheckout("o/r", "wt-a");
  await new Promise((r) => { setTimeout(r, 400); });
  assert.equal(c.deskFailure(), "the file is damaged");
  c.setCheckout("o/r", "wt-b");
  await new Promise((r) => { setTimeout(r, 400); });
  assert.equal(
    calls.filter((call) => call.method === "PUT").length,
    1,
    "one PUT, refused, and nothing after it",
  );
});

// Another tab may start the new desk first: the daemon then answers 409
// "readable", and this tab reads the desk like any other.
test("start a new desk treats a desk another tab already started as done", async (t) => {
  let started = false;
  swapFetch(t, (url, init = {}) => {
    if (init.method === "POST") {
      started = true;
      return Promise.resolve({ ok: false, status: 409, json: () => Promise.resolve({ state: "readable" }) });
    }
    if (!started) {
      return Promise.resolve({ ok: false, status: 409, json: () => Promise.resolve({ state: "unreadable", error: "parsing desk layout" }) });
    }
    return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve({ windows: [], fences: [], notes: [] }) });
  });
  // The empty new desk is restored, which looks for the stage.
  const c = load({}, { ...idLookups(() => null) });
  await c.whenDeskLoaded();
  assert.equal(c.deskFailure(), "the file is damaged");
  await c.startNewDesk();
  assert.equal(c.deskFailure(), "", "the desk is readable now");
});

// NEGATIVE CONTROL: any other refusal is a failure that names the status.
test("start a new desk fails with the status on any other refusal", async (t) => {
  swapFetch(t, (url, init = {}) =>
    Promise.resolve(
      init.method === "POST"
        ? { ok: false, status: 500, json: () => Promise.resolve({}) }
        : { ok: false, status: 409, json: () => Promise.resolve({ state: "unreadable", error: "x" }) },
    ),
  );
  const c = load();
  await c.whenDeskLoaded();
  await assert.rejects(c.startNewDesk(), /the daemon answered 500/u);
});

// NEGATIVE CONTROL: a transport failure is not a broken desk, so it offers no
// new desk.
test("a desk read that fails in transport sets no desk failure", async (t) => {
  swapFetch(t, () => Promise.reject(new Error("offline")));
  const c = load();
  await c.whenDeskLoaded();
  assert.equal(c.deskFailure(), "");
});

const deskOfSize = (n) => ({
  windows: Array.from({ length: n }, (_, i) => ({
    id: `w${i}`,
    repo: "o/r",
    agent: "console",
    kind: "console",
    consoleName: `r #${i + 1}`,
    ts: i,
  })),
  fences: [],
});

// A full desk refuses a NEW console instead of cutting one in silence.
test("atDeskCap is true once the desk holds the cap", async () => {
  const realFetch = globalThis.fetch;
  try {
    globalThis.fetch = () => Promise.resolve({ ok: true, json: () => Promise.resolve(deskOfSize(29)) }); // own: the fake changes inside the test
    const wb = load({}, {}, { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } });
    await wb.whenDeskLoaded();
    assert.equal(wb.DESK_MAX, 30);
    assert.equal(wb.atDeskCap(), false, "29 records leave room for one more");
    globalThis.fetch = () => Promise.resolve({ ok: true, json: () => Promise.resolve(deskOfSize(30)) }); // own: the fake changes inside the test
    await wb.reloadDesk();
    assert.equal(wb.atDeskCap(), true, "30 records are a full desk");
  } finally {
    globalThis.fetch = realFetch; // own: the fake changes inside the test
  }
});
