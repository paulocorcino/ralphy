// Unit tests for assets/ui/wb-daemon.ts — runs the real module with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
// oxlint-disable-next-line eslint/max-classes-per-file -- a fake class per object the test stands in for
import { test, mock } from "node:test";
import assert from "node:assert/strict";

import { fixture, swapFetch } from "./harness.mjs";
import { ReadDeadline } from "../assets/ui/wb-api.ts";
import { forgetStalls } from "../assets/ui/wb-stall.ts";
import { CONNECT_TIMEOUT_MS } from "../assets/ui/wb-resume.ts";
import { flush, newDaemon } from "./wb-daemon-fixture.mjs";

test("a handshake that never opens is closed at the deadline, and nothing else is", () => {
  const realSetTimeout = globalThis.setTimeout;
  const timers = [];
  globalThis.setTimeout = (fn, ms) => timers.push({ fn, ms });
  const sockets = [];
  globalThis.WebSocket = class {
    constructor() {
      this.readyState = 0;
      this.closes = 0;
      sockets.push(this);
    }
    close() {
      this.closes += 1;
    }
  };
  // Two pages: the socket of the second opens in time.
  try {
    for (const d of [newDaemon(), newDaemon()]) {
      d.subscribePresence(() => {});
      d.subscribeRuns("r", () => {});
      d.subscribeChanges("r", () => {});
    }
  } finally {
    globalThis.setTimeout = realSetTimeout;
    delete globalThis.WebSocket;
  }
  assert.equal(sockets.length, 2, "one socket per page");
  const deadlines = timers.filter((t) => t.ms === CONNECT_TIMEOUT_MS);
  assert.equal(deadlines.length, 2, "one handshake deadline per page socket");
  sockets[1].readyState = 1;
  for (const t of deadlines) t.fn();
  assert.deepEqual(
    sockets.map((s) => s.closes),
    [1, 0],
  );
});

// --- detachSocket: retiring a socket so its queued events cannot reach us ---

test("detachSocket nulls EVERY handler, onmessage included, then closes", () => {
  const { detachSocket } = newDaemon();
  const ws = {
    readyState: 1,
    closed: false,
    onopen: () => {},
    onmessage: () => {},
    onerror: () => {},
    onclose: () => {},
    close() {
      this.closed = true;
    },
  };
  detachSocket(ws);
  // `onmessage` matters as much as `onclose`: a `session-end` still queued on
  // the outgoing socket would land after the replacement is wired and be read
  // as the NEW connection's news.
  for (const h of ["onopen", "onmessage", "onerror", "onclose"]) {
    assert.equal(ws[h], null, `${h} must be detached before the close`);
  }
  assert.equal(ws.closed, true);
});

test("detachSocket does not call close on an already-closed socket", () => {
  const { detachSocket } = newDaemon();
  for (const readyState of [2, 3]) {
    let calls = 0;
    detachSocket({ readyState, close: () => (calls += 1) });
    assert.equal(calls, 0, `readyState ${readyState} is already past closing`);
  }
});

test("detachSocket tolerates no socket and a throwing close", () => {
  const { detachSocket } = newDaemon();
  // Both run on a resume path that must never throw into the operator's face.
  assert.doesNotThrow(() => detachSocket(null));
  assert.doesNotThrow(() => detachSocket());
  assert.doesNotThrow(() =>
    detachSocket({
      readyState: 1,
      close() {
        throw new Error("InvalidStateError");
      },
    }),
  );
});

// --- withCheckout: the optional `checkout` argument on a verb payload --------

test("withCheckout adds the key only for a real name", () => {
  const { withCheckout } = newDaemon();
  assert.deepEqual(withCheckout({ repo: "r", path: "" }, "wt-a"), {
    repo: "r",
    path: "",
    checkout: "wt-a",
  });
  // NEGATIVE CONTROL: with no selection the payload is byte-identical to the
  // pre-#406 one — no `checkout: null` key, so an older daemon never sees it.
  for (const none of [null, "", undefined]) {
    const out = withCheckout({ repo: "r", path: "" }, none);
    assert.deepEqual(out, { repo: "r", path: "" });
    assert.ok(!("checkout" in out), `no key for ${String(none)}`);
  }
  // The input is copied, never mutated.
  const input = { repo: "r", path: "x" };
  const out = withCheckout(input, "wt-a");
  assert.ok(!("checkout" in input));
  assert.notEqual(out, input);
  assert.deepEqual(withCheckout(null, "wt-a"), { checkout: "wt-a" });
});

// --- observe: a command with one reply goes over `POST /api/command` ----------


// A fake `fetch` for the command route. `answer(cmd)` gives the reply payload,
// or a `Response` to send as it is. `calls` lists each request. No WebSocket is
// injected, so a socket opened by mistake fails loudly.
function loadWithFetch(t, answer) {
  const calls = [];
  swapFetch(t, (url, init) => {
    const cmd = JSON.parse(init.body);
    calls.push({ url, method: init.method, headers: init.headers, cmd });
    const reply = answer(cmd);
    return Promise.resolve(reply instanceof Response ? reply : Response.json(reply));
  });
  return { d: newDaemon(), calls };
}

test("observe sends its command over HTTP and opens no socket", async (t) => {
  const { d, calls } = loadWithFetch(t, () => ({ status: "ok", entries: [] }));
  let sockets = 0;
  globalThis.WebSocket = class {
    constructor() {
      sockets += 1;
    }
    close() {}
  };
  try {
    const reply = await d.observe("tree.list", { repo: "o/r", path: "" });
    assert.deepEqual(reply, { status: "ok", entries: [] });
    assert.equal(sockets, 0);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, "/api/command");
    assert.equal(calls[0].method, "POST");
    // The body the daemon reads (`fixtures/api-command--request.json`).
    assert.deepEqual(Object.keys(calls[0].cmd).toSorted(), Object.keys(fixture("api-command--request")).toSorted());
    assert.equal(calls[0].cmd.verb, "tree.list");
    assert.deepEqual(calls[0].cmd.payload, { repo: "o/r", path: "" });
  } finally {
    delete globalThis.WebSocket;
  }
});

test("observe fans out an unknown checkout to the registered listeners, after the reply", async (t) => {
  const { d } = loadWithFetch(t, (cmd) => {
    if (cmd.payload.checkout === "gone") return { status: "error", message: "unknown checkout" };
    if (cmd.payload.checkout === "missing-file") return { status: "error", reason: "not found" };
    return { status: "ok", entries: [] };
  });
  const seen = [];
  d.onUnknownCheckout((repo, name) => seen.push([repo, name]));
  // A listener that throws must never break the read or the others.
  d.onUnknownCheckout(() => {
    throw new Error("boom");
  });
  const reply = await d.observe("tree.list", { repo: "o/r", path: "", checkout: "gone" });
  assert.equal(reply.message, "unknown checkout", "the reply still resolves");
  assert.deepEqual(seen, [], "the fan-out lands AFTER the resolve, not before it");
  await new Promise((r) => { setTimeout(r, 5); });
  assert.deepEqual(seen, [["o/r", "gone"]]);

  // NEGATIVE CONTROLS: another error, an ok reply, and a payload with no
  // checkout at all fire nothing.
  await d.observe("file.read", { repo: "o/r", path: "x", checkout: "missing-file" });
  await d.observe("tree.list", { repo: "o/r", path: "", checkout: "wt-a" });
  await d.observe("tree.list", { repo: "o/r", path: "" });
  await new Promise((r) => { setTimeout(r, 5); });
  assert.deepEqual(seen, [["o/r", "gone"]]);
});

test("observe rejects a command the daemon answers with an HTTP error", async (t) => {
  const { d } = loadWithFetch(t, () => new Response("boom", { status: 500 }));
  await assert.rejects(d.observe("tree.list", { repo: "r", path: "" }), /HTTP 500/u);
});

test("write and readImage send their command over HTTP", async (t) => {
  const { d, calls } = loadWithFetch(t, (cmd) =>
    cmd.verb === "file.image" ? { status: "ok", mediaType: "image/png", base64: "AAAA" } : { status: "ok" },
  );
  assert.deepEqual(await d.write("file.write", { repo: "r", path: "a.txt", content: "x" }), { status: "ok" });
  assert.equal(await d.readImage("r", "a.png", undefined, null), "data:image/png;base64,AAAA");
  assert.deepEqual(
    calls.map((c) => [c.url, c.method, c.cmd.verb]),
    [
      ["/api/command", "POST", "file.write"],
      ["/api/command", "POST", "file.image"],
    ],
  );
});

// --- observe: a command that gets no answer fails at its deadline ------------

// A `fetch` that never answers the command route; `settle(reply)` answers the
// pending request late. The timers are mocked, so a tick drives the deadline.
// Any other route (the stall report) is answered `204` at once.
function silentFetch(t, { checkout }) {
  const pending = [];
  swapFetch(t, (url) =>
    url === "/api/command"
      ? new Promise((resolve) => { pending.push(resolve); })
      : Promise.resolve(new Response(null, { status: 204 })),
  );
  mock.timers.enable({ apis: ["setTimeout"] });
  const d = newDaemon();
  const result = { error: null, value: undefined };
  const p = d.observe("tree.list", { repo: "r", path: "", checkout }).then(
    (v) => (result.value = v),
    (e) => (result.error = e),
  );
  return {
    d,
    result,
    p,
    settle: (reply) => pending[0](Response.json(reply)),
    restore: () => mock.timers.reset(),
  };
}

test("observe rejects a command with no answer at the command deadline", async (t) => {
  const s = silentFetch(t, {});
  try {
    mock.timers.tick(69999);
    await flush();
    assert.equal(s.result.error, null, "still waiting at 69999 ms");
    mock.timers.tick(1);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    assert.equal(s.result.error.what, "tree.list");
    assert.equal(s.result.error.waitedMs, 70000);
  } finally {
    s.restore();
  }
});

test("a reply after the deadline neither resolves the read nor fans out", async (t) => {
  const s = silentFetch(t, { checkout: "gone" });
  try {
    let calls = 0;
    s.d.onUnknownCheckout(() => calls++);
    mock.timers.tick(70000);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    s.settle({ status: "error", message: "unknown checkout" });
    mock.timers.tick(10);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline, "still rejected");
    assert.equal(s.result.value, undefined);
    assert.equal(calls, 0);
  } finally {
    s.restore();
  }
});

test("an observe past its deadline reports a stall naming the verb", async (t) => {
  const stalls = [];
  forgetStalls();
  const s = silentFetch(t, {});
  const silent = globalThis.fetch;
  swapFetch(t, (url, init) => {
    if (url !== "/api/device/stall") return silent(url, init);
    stalls.push({ url, ...JSON.parse(init.body) });
    return Promise.resolve(new Response(null, { status: 204 }));
  });
  try {
    mock.timers.tick(70000);
    await flush();
    assert.ok(s.result.error instanceof ReadDeadline);
    assert.deepEqual(
      stalls.map((r) => [r.url, r.kind, r.what]),
      [["/api/device/stall", "read-deadline", "tree.list"]],
    );
  } finally {
    s.restore();
  }
});

// ADR-0082 D3: a console is a `console` channel of the page socket. Its open
// waits for the socket, and carries the payload the console built.
