// Unit tests for assets/ui/wb-console.ts — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { WBColumns } from "../assets/ui/wb-columns.ts";
import { createConsole } from "../assets/ui/wb-console.ts";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
// The console's source text, for the few pins on how it is written.
const SRC = readFileSync(join(UI, "wb-console.ts"), "utf8");

// `extras` is merged into the stub `window` BEFORE the console is created, so
// a test can supply a browser global (`matchMedia`) or a page instance that
// the entry module creates. The default has no `WBNotes` and no `WBDaemon`,
// and several tests pin the fallback that produces. `opts` is what an entry module passes `createConsole`.
function load(extras = {}, docExtras = {}, opts = {}) {
  // The three globals the console touches when it is created:
  // `window.addEventListener` (the pagehide flush), `document.addEventListener`
  // and `location.protocol`/`host` (WS_ORIGIN). `boot` is not called: the
  // stub document has no stage. No `ResizeObserver` is injected ON PURPOSE:
  // the surviving one lives inside `attachTerminal`, which this harness never
  // reaches, so an observer created with the console alongside a clamp fails
  // LOUDLY here.
  const window = { addEventListener() {}, ...extras };
  const document = { readyState: "loading", addEventListener() {}, ...docExtras };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  // The sink's hold lives once per document (the module), and each `load()` is
  // a new page: a hold an earlier test set must not hold this page's writes.
  WBDeskSink.setHold(false);
  // Node 22 ships a REAL `BroadcastChannel`, and `createConsole` subscribes —
  // an open channel per `load()` holds the event loop open and `node --test`
  // never exits. Hidden for the call only; the channel's own behaviour is
  // covered against a fake in wb-detach-link.test.mjs.
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    window.WBConsole = createConsole(window, document, location, opts);
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
  return window.WBConsole;
}

test("sessionPresentation applies session-open environment and persists its owner metadata", () => {
  const got = load().sessionPresentation(
    "console",
    "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared",
    { daemonId: null, environment: null },
    {
      daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      environment: "WSL: Ubuntu-22.04",
    },
  );
  assert.deepEqual(got, {
    daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
    environment: "WSL: Ubuntu-22.04",
    name: null,
    checkout: null,
    // The TOOLTIP names the project, then the environment (ADR-0066 §5); the
    // routing head is a key and never shows.
    tooltip: "owner/shared\nWSL: Ubuntu-22.04",
  });
});

// ADR-0066 §5: the TOOLTIP holds, one per line: the project (never the
// routing head), the environment, and the vendor's session name when the
// launch had one.
test("sessionPresentation puts the project, the environment and the name in the tooltip", () => {
  // Through the REAL `WBFleet.refSlug`. A hand-written stub here
  // (`ref.split("/").slice(-2).join("/")`) passed while saying nothing about the
  // production fold, which strips a head only when it is a ULID — so a broken
  // `refSlug` would have sailed through this and every other test.
  const console_ = load();
  assert.deepEqual(
    console_.sessionPresentation(
      "claude",
      "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared",
      { daemonId: null, environment: null },
      {
        daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
        environment: "WSL: Ubuntu-22.04",
        name: "reviewer",
      },
    ),
    {
      daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
      environment: "WSL: Ubuntu-22.04",
      name: "reviewer",
      checkout: null,
      tooltip: "owner/shared\nWSL: Ubuntu-22.04\nreviewer",
    },
  );
});

// A remoteless repo is keyed `path-<hash>`: once the shell has shared the
// project names, the tooltip shows the folder and the console prefix is the
// folder name, never the key.
test("ingestProjects names a remoteless repo by its folder in the tooltip and the console prefix", () => {
  const c = load();
  const ref = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/path-8ee0b8b587ea7891";
  // Before the shell shares the names, the key is all the module has.
  assert.equal(c.consolePrefix(ref), "path-8ee0b8b587ea7891");
  c.ingestProjects([{ ref, name: "widget", title: "/home/me/widget" }]);
  assert.equal(c.consolePrefix(ref), "widget");
  const got = c.sessionPresentation("claude", ref, { daemonId: null, environment: null }, {
    daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
    environment: "WSL: Ubuntu-22.04",
  });
  assert.equal(got.tooltip, "/home/me/widget\nWSL: Ubuntu-22.04");
});

// NEGATIVE CONTROL: the name has no desk fallback — it dies with the child, so a
// restored window that has not re-opened its socket must show none, not a stale
// one. A `prior.name` must NOT resurrect it.
test("sessionPresentation never restores a session name from the desk", () => {
  const got = load().sessionPresentation(
    "claude",
    "owner/shared",
    { daemonId: "local", environment: "Windows", name: "reviewer" },
    null,
  );
  assert.equal(got.name, null);
  assert.equal(got.tooltip, "owner/shared\nWindows");
});

test("sessionPresentation keeps backward-compatible saved metadata before session-open", () => {
  assert.deepEqual(
    load().sessionPresentation(
      "claude",
      "owner/shared",
      { daemonId: "local", environment: "Windows" },
      null,
    ),
    {
      daemonId: "local",
      environment: "Windows",
      name: null,
      checkout: null,
      tooltip: "owner/shared\nWindows",
    },
  );
});

// ADR-0063 §3: the worktree the console lives in rides the presentation, and
// `renderTitle` draws it right after the label.
test("sessionPresentation carries the announced checkout", () => {
  assert.deepEqual(
    load().sessionPresentation(
      "claude",
      "owner/shared",
      { daemonId: "local", environment: "Windows" },
      { daemon_id: "local", environment: "Windows", checkout: "wt-a" },
    ),
    {
      daemonId: "local",
      environment: "Windows",
      name: null,
      checkout: "wt-a",
      tooltip: "owner/shared\nWindows",
    },
  );
});

// NEGATIVE CONTROL: the checkout has no desk fallback and never reads the
// picker's selection — only the daemon's announcement names the tree the child
// runs in, so a selection change cannot retitle a live console.
test("sessionPresentation never restores a checkout from the desk or the selection", () => {
  const got = load().sessionPresentation(
    "claude",
    "owner/shared",
    { daemonId: "local", environment: "Windows", checkout: "wt-a" },
    null,
  );
  assert.equal(got.checkout, null);
  assert.equal("title" in got, false, "the title is drawn from the window, not the presentation");
});

// The rows the switcher offers: `primary` first, then the listing's worktrees
// in order with their dirty flag; the console's own tree is the one marked
// current — `null` (a console on the primary) marks `primary`.
test("checkoutMenuRows puts primary first and marks the console's own tree", () => {
  const wb = load();
  const listing = {
    primary: "/p",
    worktrees: [
      { name: "wt-a", branch: "wt-a", dirty: true },
      { name: "wt-b", branch: "feat", dirty: false },
    ],
  };
  assert.deepEqual(
    wb.checkoutMenuRows(listing, "wt-b").map((r) => [r.name, r.branch, r.dirty, r.primary, r.current]),
    [
      ["primary", "", false, true, false],
      ["wt-a", "wt-a", true, false, false],
      ["wt-b", "feat", false, false, true],
    ],
  );
  assert.deepEqual(
    wb.checkoutMenuRows(listing, null).map((r) => [r.name, r.current]),
    [
      ["primary", true],
      ["wt-a", false],
      ["wt-b", false],
    ],
  );
  // A listing that never answered still offers the primary row — the menu
  // is only ever opened from a switchable title, so this is the guard, not
  // the expected path.
  assert.deepEqual(
    wb.checkoutMenuRows(null, null).map((r) => r.name),
    ["primary"],
  );
});

// With the shell's session rows the menu folds the agent's state per tree
// (ADR-0059 §5): `primary` is the sessions with no checkout, a worktree row
// its own; `waiting` outranks `working`; a tree with no session says nothing.
test("checkoutMenuRows carries the agent state per tree when given sessions", () => {
  // The fold is `WBProject.worktreeStates`, the real module the console imports.
  const wb = load();
  const listing = { primary: "/p", worktrees: [{ name: "wt-a" }, { name: "wt-b" }] };
  const sessions = [
    { id: 1, repo: "o/r", agent_state: { state: "working" } },
    { id: 2, repo: "o/r", checkout: "wt-a", agent_state: { state: "working" } },
    { id: 3, repo: "o/r", checkout: "wt-a", agent_state: { state: "waiting" } },
  ];
  assert.deepEqual(
    wb.checkoutMenuRows(listing, "wt-b", sessions).map((r) => [r.name, r.state, r.current]),
    [
      ["primary", "working", false],
      ["wt-a", "waiting", false],
      ["wt-b", null, true],
    ],
  );
  // No sessions given → no state field is populated.
  assert.deepEqual(
    wb.checkoutMenuRows(listing, null).map((r) => r.state),
    [null, null, null],
  );
});

// Fixtures the autoPan tests and the no-mutation test read; the fold's own
// tests are in wb-geometry.test.mjs.
const PAN_VIEW = { left: 100, top: 50, right: 1100, bottom: 850 };
const BAND = 48;
const STEP = 24;
const EXT = { width: 2000, height: 1500 };

// ---- autoPan: the loop around panNudge ---------------------------------------
// One loop serves the window drag and the fence drag. Frames are queued by a
// stub `requestAnimationFrame` and run by hand, so the test sees each tick.
function withFrames(body) {
  const frames = new Map();
  let next = 0;
  const saved = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  globalThis.requestAnimationFrame = (cb) => {
    next += 1;
    frames.set(next, cb);
    return next;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  const runFrame = () => {
    const [id, cb] = frames.entries().next().value || [];
    if (!cb) return false;
    frames.delete(id);
    cb();
    return true;
  };
  try {
    body(frames, runFrame);
  } finally {
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = saved;
  }
}

function panHarness() {
  const ws = { scrollLeft: 0, scrollTop: 0, getBoundingClientRect: () => PAN_VIEW };
  const api = load({}, { getElementById: (id) => (id === "workspace" ? ws : null) });
  const node = { isConnected: true };
  const placed = [];
  const pan = api.autoPan(node, (p) => placed.push(p));
  return { ws, node, placed, pan };
}

test("autoPan: a pointer at the edge scrolls the plane and places again each frame", () => {
  withFrames((frames, runFrame) => {
    const { ws, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    assert.ok(runFrame());
    assert.ok(runFrame());
    assert.equal(ws.scrollLeft, 48);
    assert.equal(placed.length, 2);
    assert.equal(frames.size, 1, "the loop keeps running while the pointer is in the band");
  });
});

test("autoPan: stop ends the loop, so a released button pans nothing more", () => {
  withFrames((frames, runFrame) => {
    const { ws, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    pan.stop();
    assert.equal(frames.size, 0);
    assert.equal(runFrame(), false);
    assert.equal(ws.scrollLeft, 0);
  });
});

test("autoPan: a node that left the page ends the loop without placing it", () => {
  withFrames((frames, runFrame) => {
    const { ws, node, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    node.isConnected = false;
    assert.ok(runFrame());
    assert.equal(frames.size, 0);
    assert.equal(ws.scrollLeft, 0);
    assert.equal(placed.length, 0);
  });
});

test("autoPan: a pointer away from the edges starts no loop", () => {
  withFrames((frames) => {
    const { pan } = panHarness();
    pan.follow({ x: 600, y: 400 });
    assert.equal(frames.size, 0);
  });
});

// ---- peerFold: the heartbeat and the peer-lost transition (issue #347) -------
// The window is passed in, so these run in microseconds rather than six real
// seconds — which is the whole reason the rule is a pure fold and not a timer.
const WINDOW_MS = 6000;
const SEEN = 100000;

test("peerFold: silence past the window loses the peer, exactly once", () => {
  const WB = load();
  const lost = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS + 1 }, WINDOW_MS);
  assert.deepStrictEqual(lost.effects, [{ type: "peer-lost" }]);
  assert.equal(lost.state.lost, true);
  // The effect turns into a `window.close()`, so a SECOND tick must be silent.
  const again = WB.peerFold(lost.state, { type: "tick", at: SEEN + WINDOW_MS + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(again.effects, []);
  assert.equal(again.state.lost, true);
});

// THE NEGATIVE CONTROL for the whole rule: inverting the comparison, or dropping
// the beat's `seen` update, makes this row red while the one above stays green.
test("peerFold: a beat inside the window does not expire — an F5 costs no popup", () => {
  const WB = load();
  const beaten = WB.peerFold({ seen: SEEN, lost: false }, { type: "beat", at: SEEN + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(beaten.effects, []);
  assert.equal(beaten.state.seen, SEEN + 5000);
  const tick = WB.peerFold(beaten.state, { type: "tick", at: SEEN + 6001 }, WINDOW_MS);
  assert.deepStrictEqual(tick.effects, [], "1001 ms of silence is well inside a 6000 ms window");
  assert.equal(tick.state.lost, false);
});

test("peerFold: the boundary is strict — a tick exactly at the window is still alive", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: a peer never heard from does not expire — the boot-adoption grace", () => {
  const WB = load();
  const out = WB.peerFold({ seen: null, lost: false }, { type: "tick", at: SEEN + 999999 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: loss is terminal — a beat after the loss does not resurrect the peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: true }, { type: "beat", at: SEEN + 1 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, true);
  assert.equal(out.state.seen, SEEN, "a lost peer's clock stops with it");
});

test("peerFold: an announced departure loses the peer without waiting the window out", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "gone" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, [{ type: "peer-lost" }]);
  assert.equal(out.state.lost, true);
});

test("peerFold: an unknown event is inert, so a stray channel message cannot lose a peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "origin-ping" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.state, { seen: SEEN, lost: false });
  const undef = WB.peerFold(undefined, { type: "tick", at: SEEN }, WINDOW_MS);
  assert.deepStrictEqual(undef.effects, []);
  assert.deepStrictEqual(undef.state, { seen: null, lost: false });
});

test("the fence cap is a number the shell can state, and the plane is at it from the 12th fence", async () => {
  assert.equal(load().FENCE_MAX, 12);
  // A desk with `n` fences, landed through the real GET.
  const plane = async (n) => {
    const fences = Array.from({ length: n }, (_, i) => ({
      id: `f${i}`,
      name: `Fence ${i + 1}`,
      rect: { left: i * 10, top: 0, width: 5, height: 5 },
      locked: false,
      ts: i,
    }));
    const realFetch = globalThis.fetch;
    globalThis.fetch = async () => ({ ok: true, json: async () => ({ windows: [], fences }) });
    try {
      const wb = load({}, {}, { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } });
      await wb.whenDeskLoaded();
      assert.equal(wb.fenceRecords().length, n, "the desk landed");
      return wb;
    } finally {
      globalThis.fetch = realFetch;
    }
  };
  assert.equal((await plane(0)).atFenceCap(), false, "an empty plane");
  assert.equal((await plane(11)).atFenceCap(), false, "one below the cap");
  const full = await plane(12);
  assert.equal(full.atFenceCap(), true, "at the cap");
  assert.equal(full.createFence(), false, "a fence past the cap is refused");
});

// --- resumeDecision: coming back from a suspend --------------------------
// A tablet's tab is frozen with its sockets still reporting OPEN, and the link
// is torn down without a close frame, so the exponential backoff never arms.
// `stale` is the caller's liveness verdict (the shell's presence heartbeat).
// The decision table itself is pinned once, in wb-daemon.test.mjs.

test("resumeAll and setStaleProbe are exported like the rest of the module's seam", () => {
  const c = load();
  assert.equal(typeof c.resumeAll, "function");
  assert.equal(typeof c.setStaleProbe, "function");
  // With no window open there is nothing to resume, and it must not throw:
  // `online` fires in a document that has painted no console at all.
  assert.equal(c.resumeAll(true), 0);
  // The probe seam is what keeps the shell's heartbeat verdict out of this
  // module. Setting a non-function clears it rather than poisoning the path.
  c.setStaleProbe(() => true);
  c.setStaleProbe(null);
  assert.equal(c.resumeAll(false), 0);
});

test("the font range holds xterm's default, so an unset preference changes nothing", () => {
  const c = load();
  assert.ok(c.FONT_MIN < c.FONT_DEFAULT && c.FONT_DEFAULT < c.FONT_MAX);
  // With no view store (the popup, and this harness) the size is the default.
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

test("a touch surface with no stored preference starts at the same size", () => {
  const coarse = (q) => ({ matches: q.includes("coarse"), addEventListener() {} });
  const c = load({ matchMedia: coarse });
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

// --- restoreRect: a window nobody can measure is read from its inline rect ----
// `restoreDesk` runs on load whatever tab is showing, and the Consoles tab is
// `display:none` under any other — so a restored window measured 0×0 at 0,0
// there, and the `persistWin` at the end of its spawn stored the zeros over the
// record's real box. The next load rendered the zeros as the CSS floor (240×150
// at the origin) and stored THAT: the console had "moved to the corner"
// (2026-09-09). The inline rect is what `buildChrome` just wrote from the record,
// and it is the honest box while nothing can be measured.
function fakeWin({ maximized = false, classes = [], offsets, inline }) {
  return {
    classList: { contains: (c) => (c === "maximized" && maximized) || classes.includes(c) },
    offsetLeft: offsets.left,
    offsetTop: offsets.top,
    offsetWidth: offsets.width,
    offsetHeight: offsets.height,
    style: inline || {},
  };
}
const REAL = { left: 59, top: 2569, width: 1122, height: 634 };
const INLINE = { left: "59px", top: "2569px", width: "1122px", height: "634px" };
const HIDDEN = { left: 0, top: 0, width: 0, height: 0 };

test("restoreRect reads a measurable window from the DOM", () => {
  const { restoreRect } = load();
  // The inline rect is stale on purpose: after a drag the DOM is the truth.
  const win = fakeWin({ offsets: REAL, inline: { left: "1px", top: "1px", width: "1px", height: "1px" } });
  assert.deepEqual(restoreRect(win), REAL);
});

test("restoreRect keeps a hidden window's box instead of storing zeros", () => {
  const { restoreRect } = load();
  const win = fakeWin({ offsets: HIDDEN, inline: INLINE });
  assert.deepEqual(restoreRect(win), REAL);
});

test("restoreRect on a hidden window honours a real 0 in the inline rect", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    offsets: HIDDEN,
    inline: { left: "0px", top: "0px", width: "560px", height: "340px" },
  });
  assert.deepEqual(restoreRect(win), { left: 0, top: 0, width: 560, height: 340 });
});

test("restoreRect on a maximized window still reads the pre-maximize inline rect", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    maximized: true,
    offsets: { left: 0, top: 0, width: 1440, height: 900 },
    inline: INLINE,
  });
  assert.deepEqual(restoreRect(win), REAL);
});

// --- columns: only the leftmost is the maximized console the desk records ----
// `wb-console.ts` never loads `wb-columns.ts` (the popup boots without it), so
// the harness runs the REAL fold beside it, as `app.ts` does in the browser.
function loadColumns() {
  return WBColumns;
}

// A column right of the leftmost is not `.maximized` (ADR-0051 §5), yet its
// painted box is view state: reading it would write the column onto the desk.
test("restoreRect on a column reads the inline rect, not the painted column box", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    classes: ["column"],
    offsets: { left: 960, top: 0, width: 960, height: 1000 },
    inline: INLINE,
  });
  assert.deepEqual(restoreRect(win), REAL);
});

// --- the desk changes (ADR-0050 amendment 2026-10-04, changes, not the desk) --
// A page sends only the fields each act changed. These tests load the module
// over a fake daemon: the GET serves `served`, and the sink records every
// body it is handed and answers `ok` with no desk.

async function deskPage(served = {}, extras = {}, docExtras = {}) {
  const realFetch = globalThis.fetch;
  const gets = [];
  globalThis.fetch = async (url) => {
    gets.push(String(url));
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [], ...served }) };
  };
  const sent = [];
  const answers = extras.answers || [];
  const wb = load({ ...extras.window }, { getElementById: () => null, ...docExtras }, {
    deskSink: {
      put(body) {
        sent.push(JSON.parse(body));
        return Promise.resolve(answers.shift() || { kind: "ok", reply: null });
      },
      putSync(body) {
        sent.push({ closing: true, ...JSON.parse(body) });
      },
    },
  });
  await wb.whenDeskLoaded();
  return { wb, sent, gets, restore: () => (globalThis.fetch = realFetch) };
}
const settle = (ms = 400) => new Promise((r) => setTimeout(r, ms));
const changesOf = (sent) => sent.flatMap((b) => b.changes);

// The selected checkout per project (ADR-0063 §4) is the fourth record type.
test("a checkout pick and a clear are two changes, and the clear names the tree it clears", async () => {
  const page = await deskPage({ checkouts: { "o/r": "wt" } });
  try {
    const { wb, sent } = page;
    assert.equal(wb.checkoutOf("o/r"), "wt");
    assert.deepEqual(wb.checkouts(), { "o/r": "wt" });
    // A copy, never the view itself.
    wb.checkouts()["o/r"] = "tampered";
    assert.equal(wb.checkoutOf("o/r"), "wt");

    wb.setCheckout("o/r", null);
    assert.equal(wb.checkoutOf("o/r"), null);
    wb.setCheckout("o/s", null); // nothing selected there: nothing to say
    wb.setCheckout("o/r", "wt-b");
    assert.equal(wb.checkoutOf("o/r"), "wt-b");
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "checkout-clear", repo: "o/r", ifName: "wt" },
      { op: "checkout", repo: "o/r", name: "wt-b" },
    ]);
  } finally {
    page.restore();
  }
});

test("a desk from an older daemon has no checkouts and reads as none", async () => {
  const page = await deskPage({ checkouts: undefined });
  try {
    assert.equal(page.wb.checkoutOf("o/r"), null);
    assert.deepEqual(page.wb.checkouts(), {});
  } finally {
    page.restore();
  }
});

// The measured loss of 2026-10-04 came from a page that wrote what it had not
// changed. A flush reads nothing first and carries no record it was not told.
test("a flush sends only this page's changes: no read before it, no whole desk", async () => {
  const theirs = { id: "theirs", repo: "o/r", agent: "console", kind: "console", rect: { left: 1, top: 1, width: 300, height: 200 }, sessionId: 4, consoleName: "r #1" };
  const page = await deskPage({ rev: 3, generation: 7, windows: [theirs] });
  try {
    page.wb.setCheckout("o/r", "wt");
    await settle();
    assert.equal(page.gets.length, 1, `only the load read the desk: ${page.gets}`);
    assert.equal(page.sent.length, 1);
    assert.deepEqual(Object.keys(page.sent[0]).sort(), ["changes", "generation", "seq"]);
    assert.equal(page.sent[0].generation, 7);
    assert.ok(!JSON.stringify(page.sent).includes("theirs"), JSON.stringify(page.sent));
  } finally {
    page.restore();
  }
});

const RECT = { left: 80, top: 120, width: 240, height: 180 };
const card = (id, path, extra = {}) => ({ id, repo: "o/r", path, rect: RECT, ...extra });

test("saveNotes sends a create, the fields that changed, and a remove", async () => {
  const page = await deskPage({ notes: [card("n1", "a.note"), card("n2", "b.note")] });
  try {
    const { wb, sent } = page;
    // A move stamps nothing: a `ts` handed back is not a field.
    wb.saveNotes(wb.notes().map((n) => (n.id === "n1" ? { ...n, rect: { ...RECT, left: 500 }, ts: 99 } : n)));
    wb.saveNotes(wb.notes().filter((n) => n.id !== "n2"));
    wb.saveNotes(wb.notes().concat([card("n3", "")]));
    wb.saveNotes(wb.notes().map((n) => (n.id === "n3" ? { ...n, path: "c.note" } : n)));
    wb.saveNotes(wb.notes());
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "note", id: "n1", fields: { rect: { ...RECT, left: 500 } } },
      { op: "remove", type: "note", id: "n2" },
      { op: "create", type: "note", record: card("n3", "") },
      { op: "set", type: "note", id: "n3", fields: { file: { repo: "o/r", path: "c.note", checkout: null } } },
    ]);
    // A copy: mutating what was handed out must not reach the view.
    const kept = wb.notes();
    kept.pop();
    assert.equal(wb.notes().length, 2);
  } finally {
    page.restore();
  }
});

test("a fence rename sends its name, and a fence remove sends the remove", async () => {
  const fence = { id: "f1", name: "Fence 1", rect: { left: 0, top: 0, width: 400, height: 300 } };
  const page = await deskPage({ fences: [fence] });
  try {
    page.wb.renameFence("f1", "backend");
    page.wb.removeFence("f1");
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "fence", id: "f1", fields: { name: "backend" } },
      { op: "remove", type: "fence", id: "f1" },
    ]);
  } finally {
    page.restore();
  }
});

// A window the module writes without a gesture: a fake with the fields the
// writes read.
function deskWin(id, extra = {}) {
  return {
    _deskId: id,
    _deskRepo: "o/r",
    _deskAgent: "console",
    _deskKind: "console",
    _deskDaemonId: null,
    _deskEnvironment: null,
    _deskCheckout: null,
    _deskLocked: false,
    _deskConsoleName: "r #1",
    _deskUnrecorded: false,
    isConnected: true,
    classList: { contains: () => false },
    offsetLeft: 30,
    offsetTop: 40,
    offsetWidth: 640,
    offsetHeight: 420,
    style: {},
    ...extra,
  };
}
const SAVED = { id: "w-1", repo: "o/r", agent: "console", kind: "console", rect: { left: 100, top: 100, width: 600, height: 400 }, max: false, sessionId: 7, checkout: "wt-a", consoleName: "r #1" };

// The DOM that `applyColumns` paints: a stage whose windows match the few
// selectors it queries, and a workspace with no scroll. The stage measures 0
// wide, so the fence refresh after a write returns early.
function columnDom(wins) {
  const match = (w, sel) =>
    sel.split(",").some((one) =>
      one.trim().split(".").filter(Boolean).every((c) => w.classList.contains(c)),
    );
  const all = (sel) => wins.filter((w) => match(w, sel));
  const one = (sel) => all(sel)[0] ?? null;
  const stageEl = { offsetWidth: 0, offsetHeight: 0, style: {}, querySelectorAll: all, querySelector: one };
  const workspaceEl = {
    scrollLeft: 0,
    scrollTop: 0,
    classList: { toggle() {} },
    querySelectorAll: all,
    querySelector: one,
  };
  return { stage: stageEl, workspace: workspaceEl };
}
function columnWin(id, maximized = false) {
  const classes = new Set(maximized ? ["session-window", "maximized"] : ["session-window"]);
  return deskWin(id, {
    classList: {
      contains: (c) => classes.has(c),
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c), on),
    },
    style: { setProperty() {}, removeProperty() {} },
  });
}
const colRecord = (id, max) => ({ ...SAVED, id, max, sessionId: null, checkout: null });
const byId = (changes) => [...changes].sort((x, y) => x.id.localeCompare(y.id));

// ADR-0051 §5: the console that becomes first in the columns is written to
// the desk as an ordinary maximize, and the one that stops being first is
// written as not maximized. `WBColumns.fromStored` keeps the stored grid only
// when the desk records its first console as maximized.
test("applyColumns writes the moved maximize to the desk when asked to persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
  }
});

// The old maximized console can stay in the grid, not first: a fence opened as
// columns whose leftmost member is another console. No `unmax` names it.
test("applyColumns writes the unmaximize of a console that stays a column, not first", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    page.wb.applyColumns(C.painted([["w-b"], ["w-a"]], 2), { cap: 2, unmax: null, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
  }
});

test("markRelaunch marks the id, and a take-down that throws clears the mark", () => {
  const { markRelaunch, isRelaunching } = load();
  assert.equal(isRelaunching("w-a"), false);
  let seen = null;
  markRelaunch("w-a", () => (seen = isRelaunching("w-a")));
  assert.equal(seen, true, "marked before the take-down");
  assert.equal(isRelaunching("w-a"), true, "still marked until the respawn");
  assert.equal(isRelaunching("w-b"), false);
  assert.throws(() => markRelaunch("w-b", () => { throw new Error("gone"); }), /gone/);
  assert.equal(isRelaunching("w-b"), false, "a take-down that throws clears the mark");
});

// A relaunch takes the window off the stage and spawns a new one under the
// same id; the shell's columns wait for an id marked in between. These paths
// need a real DOM, so the wiring is pinned on the source.
test("a restart and a placeholder's Launch mark the console as relaunching", () => {
  assert.match(SRC, /markRelaunch\(carry\.id, discard\);/);
  assert.match(SRC, /markRelaunch\(carry\.id, \(\) => drop\(true\)\);/);
  assert.match(SRC, /\} finally \{\s*relaunching\.delete\(carry\?\.id\);/);
});

// The torn-off fence window's grid is never stored (ADR-0051 §8, amended
// 2026-10-05), so its one call never asks to write the maximize.
test("the torn-off fence window paints its columns without persist", () => {
  // The page and its page script module.
  const html =
    readFileSync(join(UI, "detached-fence.html"), "utf8") + readFileSync(join(UI, "wb-detached-fence.ts"), "utf8");
  const calls = html.match(/WBConsole\.applyColumns\([^;]*;/g) || [];
  assert.equal(calls.length, 1, calls.join("\n"));
  assert.ok(!/\bpersist\b/.test(html), "the torn-off fence window names persist");
});

// The torn-off fence window paints its own grid and never stores it
// (ADR-0051 §8, amended 2026-10-05), so its maximize is not written.
test("applyColumns writes nothing to the desk without persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax });
    await settle();
    assert.ok(b.classList.contains("maximized"), "the page still shows the move");
    assert.deepEqual(page.sent, []);
  } finally {
    page.restore();
  }
});

// Seven of the eleven old window writes were not about the rect, and each
// wrote the rect anyway. A reconnect writes the session, only when it moved.
test("a reconnect writes the session only when it differs, and never the rect", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const win = deskWin("w-1", { _term: { sessionId: 7 }, _deskCheckout: "wt-a" });
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(page.sent, [], "the same session writes nothing");
    win._term.sessionId = 8;
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "window", id: "w-1", fields: { session: { sessionId: 8, daemonId: null, environment: null } } },
    ]);
  } finally {
    page.restore();
  }
});

test("a birth writes only what it changed; an adopted console waits for the operator's first act", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const { wb, sent } = page;
    // A relaunch in the primary tree: the record said `wt-a`.
    wb.recordBirth(deskWin("w-1"), SAVED);
    // An adopted console: its record is another page's to write.
    const adopted = deskWin("w-adopted");
    wb.recordBirth(adopted, { unrecorded: true });
    // A new console: its record is created.
    wb.recordBirth(deskWin("w-new"), undefined);
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "window", id: "w-1", fields: { checkout: null } },
      {
        op: "create",
        type: "window",
        record: {
          id: "w-new", repo: "o/r", agent: "console", kind: "console",
          rect: { left: 30, top: 40, width: 640, height: 420 }, max: false, sessionId: null,
          daemonId: null, environment: null, checkout: null, locked: false, consoleName: "r #1",
        },
      },
    ]);
    assert.equal(adopted._deskUnrecorded, true);
    // The operator's first act on it: the record, then the act, in one batch.
    sent.length = 0;
    wb.setWin(adopted, { locked: true });
    await settle();
    assert.equal(sent.length, 1, "one batch");
    assert.deepEqual(
      sent[0].changes.map((c) => [c.op, c.type, c.record?.id ?? c.id]),
      [["create", "window", "w-adopted"], ["set", "window", "w-adopted"]],
    );
    assert.equal(adopted._deskUnrecorded, false);
  } finally {
    page.restore();
  }
});

// A failure the daemon may still accept keeps the batch, resent with its
// `seq`; a 400 drops it; a restore reloads the page.
test("a flush keeps a batch the network lost, drops one the daemon calls malformed, and reloads on a restore", async () => {
  let reloads = 0;
  const page = await deskPage(
    {},
    {
      answers: [{ kind: "network" }, { kind: "ok", reply: null }, { kind: "refused", status: 400, reply: { error: "x" } }, { kind: "refused", status: 409, reply: { state: "restored" } }],
      window: { location: { reload: () => reloads++ } },
    },
  );
  try {
    const { wb, sent } = page;
    wb.setCheckout("o/r", "a");
    await settle(1600); // the first retry waits one second
    assert.equal(sent.length, 2);
    assert.deepEqual(sent[1], sent[0], "the resend is the same batch, with the same seq");
    wb.setCheckout("o/r", "b");
    await settle();
    assert.equal(sent[2].seq, sent[0].seq + 1);
    wb.setCheckout("o/r", "c");
    await settle();
    assert.deepEqual(changesOf([sent[3]]), [{ op: "checkout", repo: "o/r", name: "c" }], "the dropped change is gone");
    assert.equal(reloads, 1, "a restore reloads the page");
    wb.setCheckout("o/r", "d");
    await settle();
    assert.equal(sent.length, 4, "a page that reloads sends nothing more");
  } finally {
    page.restore();
  }
});

// The folds and the geometry are pure: none mutates what it is given.
test("the pure folds mutate none of their arguments", () => {
  const WB = load();
  const detach = (event) => [
    `detachFold on ${event.type}`,
    [["f-a", "f-b"], event],
    (reg, e) => WB.detachFold(reg, e),
    // A NEW array comes back, never the input.
    (out, [reg]) => assert.notEqual(out.registry, reg, `detachFold on ${event.type} returns a new array`),
  ];
  const peer = (event) => [
    `peerFold on ${event.type}`,
    [{ seen: SEEN, lost: false }, event, WINDOW_MS],
    (...args) => WB.peerFold(...args),
  ];
  // [case, arguments, call, extra check on the result]
  const rows = [
    [
      "anchorIntoView",
      [{ left: 600, top: 400, width: 200, height: 100 }, { width: 1000, height: 700 }, EXT],
      (...args) => WB.anchorIntoView(...args),
    ],
    [
      "panNudge",
      [{ x: 1076, y: 400 }, { left: 100, top: 50, right: 1100, bottom: 850 }, BAND, STEP],
      (...args) => WB.panNudge(...args),
    ],
    [
      "bringIntoView",
      [
        { left: 1200, top: 400, width: 200, height: 100 },
        { width: 1000, height: 700 },
        { width: 2000, height: 1500 },
      ],
      (...args) => WB.bringIntoView(...args),
    ],
    [
      "fenceCycle",
      [
        [
          { id: "a", rect: { left: 0, top: 0, width: 400, height: 300 } },
          { id: "b", rect: { left: 500, top: 0, width: 400, height: 300 } },
        ],
        "a",
        1,
      ],
      (...args) => WB.fenceCycle(...args),
    ],
    detach({ type: "detach", fenceId: "f-new" }),
    detach({ type: "detach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-nope" }),
    detach({ type: "focus", fenceId: "f-a" }),
    detach({ type: "wat", fenceId: "f-a" }),
    peer({ type: "beat", at: SEEN + 10 }),
    peer({ type: "tick", at: SEEN + WINDOW_MS + 1 }),
    peer({ type: "gone" }),
    ["dragBegins", [{ x: 1, y: 2 }, { x: 30, y: 40 }, 10], (...args) => WB.dragBegins(...args)],
    [
      "isDoubleTap",
      [{ t: 0, x: 1, y: 2 }, { t: 100, x: 3, y: 4 }],
      (...args) => WB.isDoubleTap(...args),
    ],
  ];
  for (const [name, args, call, check] of rows) {
    const before = structuredClone(args);
    const out = call(...args);
    assert.deepEqual(args, before, `${name} mutated its arguments`);
    if (check) check(out, args);
  }
});

// ADR-0070 D4: a desk the daemon cannot read is a failure with a reason, and
// this page never uploads over it.
test("an unreadable desk is a failure, and no flush PUTs over it", async () => {
  const calls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    return {
      ok: false,
      status: 409,
      json: async () => ({ state: "unreadable", error: "parsing desk layout C:/x/desk.toml" }),
    };
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "the file is damaged");
    c.setCheckout("o/r", "wt-a");
    await new Promise((r) => setTimeout(r, 400));
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    calls.filter((c) => c.method === "PUT"),
    [],
    "no PUT over a desk the daemon cannot read",
  );
});

// A desk that becomes unreadable after a good load: the daemon refuses the
// PUT, the page shows the failure and sends nothing more.
test("a PUT the daemon refuses as unreadable shows the failure and stops the sending", async () => {
  const calls = [];
  let unreadable = false;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    if (unreadable) {
      return { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "parsing desk layout" }) };
    }
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [] }) };
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "", "the first read was good");
    unreadable = true;
    calls.length = 0;
    c.setCheckout("o/r", "wt-a");
    await new Promise((r) => setTimeout(r, 400));
    assert.equal(c.deskFailure(), "the file is damaged");
    c.setCheckout("o/r", "wt-b");
    await new Promise((r) => setTimeout(r, 400));
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(
    calls.filter((c) => c.method === "PUT").length,
    1,
    "one PUT, refused, and nothing after it",
  );
});

// Another tab may start the new desk first: the daemon then answers 409
// "readable", and this tab reads the desk like any other.
test("start a new desk treats a desk another tab already started as done", async () => {
  let started = false;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    if (init.method === "POST") {
      started = true;
      return { ok: false, status: 409, json: async () => ({ state: "readable" }) };
    }
    if (!started) {
      return { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "parsing desk layout" }) };
    }
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [] }) };
  };
  try {
    // The empty new desk is restored, which looks for the stage.
    const c = load({}, { getElementById: () => null });
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "the file is damaged");
    await c.startNewDesk();
    assert.equal(c.deskFailure(), "", "the desk is readable now");
  } finally {
    globalThis.fetch = realFetch;
  }
});

// NEGATIVE CONTROL: any other refusal is a failure that names the status.
test("start a new desk fails with the status on any other refusal", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) =>
    init.method === "POST"
      ? { ok: false, status: 500, json: async () => ({}) }
      : { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "x" }) };
  try {
    const c = load();
    await c.whenDeskLoaded();
    await assert.rejects(c.startNewDesk(), /the daemon answered 500/);
  } finally {
    globalThis.fetch = realFetch;
  }
});

// NEGATIVE CONTROL: a transport failure is not a broken desk, so it offers no
// new desk.
test("a desk read that fails in transport sets no desk failure", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error("offline");
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "");
  } finally {
    globalThis.fetch = realFetch;
  }
});

// A full desk refuses a NEW console instead of cutting one in silence.
test("atDeskCap is true once the desk holds the cap", async () => {
  const realFetch = globalThis.fetch;
  const desk = (n) => ({
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
  try {
    globalThis.fetch = async () => ({ ok: true, json: async () => desk(29) });
    const wb = load({}, {}, { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } });
    await wb.whenDeskLoaded();
    assert.equal(wb.DESK_MAX, 30);
    assert.equal(wb.atDeskCap(), false, "29 records leave room for one more");
    globalThis.fetch = async () => ({ ok: true, json: async () => desk(30) });
    await wb.reloadDesk();
    assert.equal(wb.atDeskCap(), true, "30 records are a full desk");
  } finally {
    globalThis.fetch = realFetch;
  }
});
