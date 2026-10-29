// Unit tests for assets/ui/wb-connection-store.ts, the Alpine store
// `connection`: the session, the presence socket and its heartbeat, the page
// build and the write locks. A store test builds it without Alpine (ADR-0073
// D3, D6), as `main.ts` builds it for `Alpine.store`. A page test builds the
// page with `loadShell`, whose window hears what it is sent, and runs the
// `init()` of the store `projects` and of `shell()`, so the events the store
// sends reach their listeners as on the page.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, loadShell } from "./harness.mjs";
import { connectionStore } from "../assets/ui/wb-connection-store.ts";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";

const VISIBLE = { document: { visibilityState: "visible", hasFocus: () => true } };

// The store alone, on a window stub that hears what it is sent.
function storeOnly() {
  const events = [];
  globalThis.window = {
    Alpine: { store: () => undefined },
    addEventListener() {},
    dispatchEvent: (e) => (events.push(e.type), true),
  };
  globalThis.document = { visibilityState: "visible", querySelector: () => null, addEventListener() {} };
  const store = connectionStore({ messages: { flash() {} }, detached: { dirty: () => false, close() {} } });
  return { store, events };
}

// A page whose window hears what it is sent, after the `init()` of the stores
// `projects` and `release`, and of `shell()`. Their own reads at load and their clocks do
// not run here.
function connectedPage(opts = {}) {
  const listeners = {};
  const loaded = loadShell({
    ...opts,
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
      ...opts.window,
    },
  });
  const { state } = loaded;
  const projects = state.$store.projects;
  const reads = {};
  for (const name of ["loadRepos", "loadAgents"]) {
    reads[name] = projects[name];
    projects[name] = () => {};
  }
  const release = state.$store.release;
  const loadRelease = release.loadRelease;
  release.loadRelease = () => {};
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = () => 0;
  try {
    projects.init();
    release.init();
    state.init();
  } finally {
    globalThis.setInterval = realSetInterval;
    Object.assign(projects, reads);
    release.loadRelease = loadRelease;
  }
  return { ...loaded, store: state.$store.connection };
}

// Every URL the code under test fetched, answered with an empty 200. A bare
// `fetch` in app.ts resolves to `globalThis.fetch`, not the harness window's.
// `until`, when given, names a URL the read must reach: the spy waits for it
// (up to 2 s) instead of a fixed number of event-loop turns, because a read
// scheduled with `setTimeout` can land after any number of turns.
async function withFetchSpy(fn, until = null) {
  const urls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    urls.push(String(url));
    return { ok: true, status: 200, json: async () => [] };
  };
  try {
    await fn(urls);
    // Let the reads chained behind the first await land.
    for (let i = 0; i < 10; i++) await new Promise((r) => setImmediate(r));
    const deadline = Date.now() + 2000;
    while (until && !urls.includes(until) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
  } finally {
    globalThis.fetch = realFetch;
  }
  return urls;
}

test("fmtUptime steps down through the units and never renders a negative", () => {
  const { store: s } = storeOnly();
  assert.equal(s.fmtUptime(0), "0s");
  assert.equal(s.fmtUptime(45), "45s");
  assert.equal(s.fmtUptime(60), "1m");
  assert.equal(s.fmtUptime(3600), "1h 0m");
  assert.equal(s.fmtUptime(3661), "1h 1m");
  assert.equal(s.fmtUptime(86400), "1d 0h");
  assert.equal(s.fmtUptime(90061), "1d 1h");
  // Two coarse units only: a day-old daemon never shows minutes.
  assert.equal(s.fmtUptime(90000 + 59), "1d 1h");
  // NEGATIVE CONTROL: a clock that ran backwards, and a missing reading, must
  // both render as zero rather than as "-1s" or "NaNs".
  assert.equal(s.fmtUptime(-5), "0s");
  assert.equal(s.fmtUptime(null), "0s");
  assert.equal(s.fmtUptime(undefined), "0s");
});

// The binding cannot see ticks STOP; the clock that `init` starts must.
test("presenceStale turns true on the clock once the heartbeat is older than the limit", async () => {
  const { store } = storeOnly();
  const ticks = [];
  const realSetInterval = globalThis.setInterval;
  const realFetch = globalThis.fetch;
  globalThis.setInterval = (fn, ms) => (ticks.push({ fn, ms }), 0);
  globalThis.fetch = async () => ({ ok: false, status: 404, json: async () => null });
  try {
    store.init();
  } finally {
    globalThis.setInterval = realSetInterval;
    globalThis.fetch = realFetch;
  }
  const clock = ticks.find((t) => t.ms === 1000);
  assert.ok(clock, "init starts the one-second presence clock");
  // The resume on a lost heartbeat has its own test below.
  store.resumeSockets = () => {};
  store._lastHeartbeat = Date.now();
  clock.fn();
  assert.equal(store.presenceStale, false, "a fresh heartbeat is not stale");
  store._lastHeartbeat = Date.now() - 7000;
  clock.fn();
  assert.equal(store.presenceStale, true, "three missed heartbeats are stale");
});

// #751: a desktop that slept with the tab in front fires no page event. The
// clock resumes the page once, when the heartbeat stops while it is visible.
test("a heartbeat that stops while the page is visible resumes the sockets and the consoles once", () => {
  const { store } = storeOnly();
  const ticks = [];
  const realSetInterval = globalThis.setInterval;
  const realFetch = globalThis.fetch;
  globalThis.setInterval = (fn, ms) => (ticks.push({ fn, ms }), 0);
  globalThis.fetch = async () => ({ ok: false, status: 404, json: async () => null });
  try {
    store.init();
  } finally {
    globalThis.setInterval = realSetInterval;
    globalThis.fetch = realFetch;
  }
  const clock = ticks.find((t) => t.ms === 1000);
  const calls = [];
  store.resumeSockets = (stale) => calls.push(["sockets", stale]);
  window.WBConsole = { resumeAll: (stale) => calls.push(["consoles", stale]) };

  clock.fn();
  assert.deepEqual(calls, [], "a page that never had a heartbeat lost nothing");
  store._lastHeartbeat = Date.now();
  clock.fn();
  store._lastHeartbeat = Date.now() - 7000;
  clock.fn();
  assert.deepEqual(calls, [["sockets", true], ["consoles", true]]);
  clock.fn();
  assert.equal(calls.length, 2, "one resume per loss, not one per tick");

  store._lastHeartbeat = Date.now();
  clock.fn();
  document.visibilityState = "hidden";
  store._lastHeartbeat = Date.now() - 7000;
  clock.fn();
  assert.equal(calls.length, 2, "a hidden page resumes on its return instead");
});

test("returning to the tab reads the release view again", () => {
  const calls = [];
  const { state, store, window } = connectedPage(VISIBLE);
  window.addEventListener("workbench:board-refresh", (e) => calls.push(`board:${e.detail.trigger}`));
  state.rereadDesk = () => calls.push("rereadDesk");
  state.$store.release.loadRelease = () => calls.push("loadRelease");
  state.$store.projects.loadRepos = () => calls.push("loadRepos");
  store.resumeSockets = () => calls.push("resumeSockets");
  state.$store.runs.hydrateRuns = () => {};
  store.onTabVisible();
  // Each read happens once; their order is not what the tab depends on.
  // `loadRepos` reads the sessions, the fleet, the change set and the branch.
  assert.deepEqual(calls.toSorted(), [
    "board:visible",
    "loadRelease",
    "loadRepos",
    "rereadDesk",
    "resumeSockets",
  ]);
});

test("resumeSockets resumes the file tree socket with the others", () => {
  const resumed = [];
  const { state } = loadShell({
    window: { dispatchEvent: (e) => resumed.push([e.type, e.detail.stale]) },
  });
  const store = state.$store.connection;
  state.$store.runs._runsSub = { resume: (stale) => resumed.push(["_runsSub", stale]) };
  store._presenceSub = { resume: (stale) => resumed.push(["_presenceSub", stale]) };
  store.resumeSockets(true);
  // The file tree's socket is the files' (wb-files.test.mjs hears the event),
  // and the changes socket is the Changes view's (wb-changes-panel.test.mjs).
  assert.deepEqual(resumed, [
    ["_runsSub", true],
    ["_presenceSub", true],
    ["workbench:sockets-resume", true],
  ]);
});

test("a presence frame reads nothing", async () => {
  const { store, window } = connectedPage();
  let beat = null;
  window.WBDaemon.subscribePresence = (onPresence) => {
    beat = onPresence;
    return { resume() {}, close() {} };
  };
  store.subscribePresence();
  const urls = await withFetchSpy(() => beat({ uptime_secs: 1 }));
  assert.deepEqual(urls, [], "the heartbeat is not a read trigger");
  assert.equal(store.uptimeText, "Running for 1s");
});

test("a hidden tab reads nothing on a push, and reads when it becomes visible", async () => {
  const { state, store, document } = connectedPage({ document: { visibilityState: "hidden", hasFocus: () => true } });
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  // The store schedules the settle read with Node's global `setTimeout`. Catch
  // the timers here and run them inside the spy, so a read that the timer
  // would make is seen on every run and not only when it fires early.
  const timers = [];
  const realSetTimeout = globalThis.setTimeout;
  const hidden = await withFetchSpy(() => {
    globalThis.setTimeout = (fn) => {
      timers.push(fn);
      return timers.length;
    };
    try {
      store.onPresencePush("sessions.dirty", {});
      store.onPresencePush("repos.dirty", {});
      store.onPresencePush("desk.dirty", { tab: "other" });
      store.onPresenceOpen(true);
    } finally {
      globalThis.setTimeout = realSetTimeout;
    }
    for (const run of timers) run();
  });
  assert.deepEqual(hidden, []);
  document.visibilityState = "visible";
  const visible = await withFetchSpy(() => store.onTabVisible());
  assert.ok(visible.includes("/api/sessions"), visible.join(", "));
  assert.ok(visible.includes("/api/repos"), visible.join(", "));
  assert.ok(visible.includes("/api/desk"), visible.join(", "));
});

test("peers.dirty and repos.dirty read the project list", async () => {
  for (const verb of ["peers.dirty", "repos.dirty"]) {
    const { state, store } = connectedPage(VISIBLE);
    state.checkColumnDesk = async () => {};
    const urls = await withFetchSpy(() => store.onPresencePush(verb, {}));
    assert.ok(urls.includes("/api/repos"), `${verb}: ${urls.join(", ")}`);
  }
});

test("desk.dirty from this tab is ignored, and from another tab reads the desk", () => {
  const { state, store, window } = connectedPage(VISIBLE);
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  let reads = 0;
  window.WBConsole.reloadDesk = () => {
    reads += 1;
    return Promise.resolve();
  };
  store.onPresencePush("desk.dirty", { tab: WBDeskSink.tabId() });
  assert.equal(reads, 0, "this tab's own write");
  store.onPresencePush("desk.dirty", { tab: "another-tab" });
  assert.equal(reads, 1);
});

test("a reopened presence socket reads sessions, projects and the desk; the first open reads nothing", async () => {
  const { state, store } = connectedPage(VISIBLE);
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  const first = await withFetchSpy(() => store.onPresenceOpen(false));
  assert.deepEqual(first, []);
  const again = await withFetchSpy(() => store.onPresenceOpen(true));
  for (const url of ["/api/sessions", "/api/repos", "/api/desk"]) {
    assert.ok(again.includes(url), `${url} in ${again.join(", ")}`);
  }
});

test("sessions.dirty on a visible tab reads the sessions", async () => {
  const { store } = connectedPage(VISIBLE);
  store.LIVE_SETTLE_MS = 0;
  const urls = await withFetchSpy(() => store.onPresencePush("sessions.dirty", {}), "/api/sessions");
  assert.ok(urls.includes("/api/sessions"), urls.join(", "));
});

test("a tab that becomes visible reads the runs with the Runs panel closed, so an ended run unlocks the writes", async () => {
  const { state, store, window } = connectedPage(VISIBLE);
  state.checkColumnDesk = async () => {};
  window.WBDaemon.observe = async (verb) => (verb === "runs.list" ? { status: "ok", runs: [] } : null);
  state.$store.projects.setOpen("o/r");
  state.runsOpen = false;
  state.$store.runs.runsByProject["o/r"] = [{ runid: "r1" }];
  assert.equal(store.writeLocked(), true, "the run seen before the tab was hidden locks");
  await withFetchSpy(() => store.onTabVisible());
  assert.equal(store.writeLocked(), false, "the run ended while hidden: the writes unlock");
});

test("a reopened socket reads the board and the runs", () => {
  const calls = [];
  const { state, store, window } = connectedPage(VISIBLE);
  window.addEventListener("workbench:board-refresh", (e) => calls.push(`board:${e.detail.trigger}`));
  state.rereadDesk = () => calls.push("rereadDesk");
  state.$store.projects.loadRepos = () => calls.push("loadRepos");
  state.$store.runs.hydrateRuns = () => calls.push("hydrateRuns");
  state.$store.projects.setOpen("o/r");
  store.onPresenceOpen(true);
  assert.ok(calls.includes("board:reopen"), calls.join(", "));
  assert.ok(calls.includes("hydrateRuns"), calls.join(", "));
});

test("visible and login ask the open panels to read again", () => {
  // The Settings dialog hears the event and reads only when it is open
  // (wb-settings-dialog.test.mjs); here the store sends it each time.
  const events = [];
  const loaded = loadShell({
    ...VISIBLE,
    window: { dispatchEvent: (e) => events.push(e.type) },
  });
  const { state } = loaded;
  const store = state.$store.connection;
  for (const name of ["loadRepos", "loadAgents"]) state.$store.projects[name] = () => {};
  for (const name of ["resumeSockets", "loadIdentity"]) store[name] = () => {};
  state.$store.runs.hydrateRuns = () => {};
  const rereads = () => events.filter((t) => t === "workbench:panels-reread").length;
  store.onTabVisible();
  assert.equal(rereads(), 1, "the tab becoming visible asks the open panels");
  loadComponent("wbLogin", { from: loaded }).scope.rehydrateAfterAuth();
  assert.equal(rereads(), 2, "a login asks the open panels too");
});

test("the peer tick and a project-list push read no change set and no branch", async () => {
  const git = [];
  const { state, store, window } = connectedPage(VISIBLE);
  window.addEventListener("workbench:changes-read", (e) => git.push(e.detail.ref));
  state.checkColumnDesk = async () => {};
  state.$store.projects.setOpen("o/r");
  state.$store.projects.fleetPeers = [{ daemon_id: "d" }];
  await withFetchSpy(async () => {
    state.$store.projects.peerTick();
    store.onPresencePush("repos.dirty", {});
    store.onPresencePush("peers.dirty", {});
  });
  assert.deepEqual(git, []);
  // NEGATIVE CONTROL: the tab becoming visible does read them.
  await withFetchSpy(() => store.onTabVisible());
  assert.deepEqual(git, ["o/r"]);
});

// The session read: the store keeps `authed`, the mark and the policy, and the
// shell takes the restore of the view (the login gate's password flag:
// wb-login.test.mjs).
test("a session read keeps the policy, and restores the view only when authorized", async () => {
  for (const authed of [true, false]) {
    const { state, store } = connectedPage(VISIBLE);
    const restored = [];
    state.$store.tabs.restoreView = () => restored.push("restore");
    const realFetch = globalThis.fetch;
    globalThis.fetch = async () => ({
      ok: true,
      status: 200,
      json: async () => ({ authed, password: true, policy: "bearer", avatar: "🦊" }),
    });
    try {
      await store.probeSession();
    } finally {
      globalThis.fetch = realFetch;
    }
    assert.equal(store.authed, authed);
    assert.equal(store.identityAvatar, "🦊");
    assert.equal(store.security.policy, "bearer");
    assert.deepEqual(restored, authed ? ["restore"] : [], `authed ${authed}`);
  }
});

// Log off: the page hears `workbench:log-off`, and only the `session` policy
// has a login gate to drop to (#205).
test("log off drops to the login gate only under the session policy", async () => {
  for (const [policy, authed] of [
    ["session", false],
    ["localhost", true],
  ]) {
    const sent = [];
    const { state, document } = loadShell({ window: { dispatchEvent: (e) => (sent.push(e.type), true) } });
    const actions = [];
    document.dispatchEvent = (e) => (actions.push(e.detail.action), true);
    const store = state.$store.connection;
    store.securityChanged({ policy });
    await store.logOff();
    assert.equal(store.authed, authed, policy);
    assert.deepEqual(sent, ["workbench:log-off"], policy);
    assert.deepEqual(actions, ["logoff"], policy);
  }
});

// --- ADR-0070 D6: a tab on an older build than the daemon -------------------

function skewShell({ dirty = false, pageBuild = "A" } = {}) {
  const loaded = loadShell();
  const { state, window } = loaded;
  const store = state.$store.connection;
  let reloads = 0;
  window.location.reload = () => (reloads += 1);
  window.WBViewer.anyDirty = () => dirty;
  window.WBNotes.anyDirty = () => false;
  let beat = null;
  window.WBDaemon.subscribePresence = (onPresence) => {
    beat = onPresence;
    return { resume() {}, close() {} };
  };
  store.pageBuild = pageBuild;
  store.subscribePresence();
  return { store, loaded, beat: (p) => beat(p), reloads: () => reloads };
}

test("a build the page was not served with reloads a tab with no unsaved work", () => {
  const t = skewShell();
  t.beat({ uptime_secs: 1, build: "A" });
  assert.equal(t.reloads(), 0, "the same build reloads nothing");
  t.beat({ uptime_secs: 3, build: "B" });
  assert.equal(t.reloads(), 1);
});

test("with unsaved work the tab keeps the page, shows the notice and locks writes", () => {
  const t = skewShell({ dirty: true });
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 0);
  assert.equal(t.store.buildSkew, true);
  assert.equal(t.store.writeLocked(), true);
  assert.match(t.store.writeLockReason(), /older than Ralphy/);
});

test("a page with no build id never reloads for a build", () => {
  const t = skewShell({ pageBuild: "" });
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 0);
  assert.equal(t.store.buildSkew, false);
});

test("a commit draft does not hold the build reload", () => {
  const t = skewShell();
  loadComponent("wbChanges", { from: t.loaded }).scope.commitMsg = "wip: half a message";
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 1);
});

test("a hidden tab with no unsaved work holds the desk and does not reload", () => {
  const t = skewShell();
  t.loaded.document.visibilityState = "hidden";
  const holds = [];
  const realSetHold = WBDeskSink.setHold;
  WBDeskSink.setHold = (on) => holds.push(on);
  try {
    t.beat({ uptime_secs: 1, build: "B" });
  } finally {
    WBDeskSink.setHold = realSetHold;
  }
  assert.equal(t.reloads(), 0, "a hidden tab waits for the next heartbeat");
  assert.deepEqual(holds, [true], "its desk is held until it reloads");
  assert.equal(t.store.buildSkew, false, "no unsaved work: no notice");
});

test("the desk hold ends right before the build reload, so the saved work's desk changes go out", () => {
  const { state, window } = loadShell(VISIBLE);
  const store = state.$store.connection;
  const events = [];
  window.location.reload = () => events.push("reload");
  // The sink is one module per document: its `setHold` is replaced for this
  // test only.
  const realSetHold = WBDeskSink.setHold;
  WBDeskSink.setHold = (on) => events.push(`hold:${on}`);
  try {
    let dirty = true;
    window.WBViewer.anyDirty = () => dirty;
    window.WBNotes.anyDirty = () => false;
    let beat = null;
    window.WBDaemon.subscribePresence = (onPresence) => {
      beat = onPresence;
      return { resume() {}, close() {} };
    };
    store.pageBuild = "A";
    store.subscribePresence();
    beat({ uptime_secs: 1, build: "B" });
    assert.deepEqual(events, ["hold:true"], "unsaved work: held, no reload");
    dirty = false;
    beat({ uptime_secs: 3, build: "B" });
    assert.deepEqual(events, ["hold:true", "hold:false", "reload"]);
  } finally {
    WBDeskSink.setHold = realSetHold;
  }
});
