// Characterization tests for assets/ui/app.ts — the `shell()` component.
//
// These exist to be written BEFORE app.ts is split, not after. A split of the
// read-only folds out of a 5,000-line component is a refactor only if something
// states what those folds do today; without that it is a rewrite with a
// reassuring diff. Every assertion here was derived by running the fold, not by
// reading it — where the two disagreed, the run won and the surprise is noted.
//
// Scope on purpose: folds that COMPUTE. `shell()` also owns fetch paths, Alpine
// lifecycle and DOM work, and the harness deliberately provides an empty
// document and a throwing `fetch` so that a fold reaching for either fails
// loudly here rather than being quietly half-covered.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadComponent, loadShell, UI } from "./harness.mjs";

// NEGATIVE CONTROL for the whole file: a duplicate key in the state literal is
// silently legal in sloppy mode and the LAST one wins, which is how `app.ts`
// carried two incompatible `changesError` declarations. `Object.keys()` cannot
// see the loser, so this reads the source.
test("the state literal declares no key twice", () => {
  const text = readFileSync(join(UI, "app.ts"), "utf8");
  const body = text.slice(text.indexOf("function shell()"), text.indexOf("export function wire("));
  // BOTH spellings, because the literal uses both and the collision that
  // prompted this test could just as easily be two methods. Matching only
  // `name:` saw 119 of the 393 entries and was blind to all 271 method
  // shorthands — a gate covering 30% of the thing it guards.
  const keys = new Map();
  const count = (key) => keys.set(key, (keys.get(key) || 0) + 1);
  for (const m of body.matchAll(/^ {4}([A-Za-z_$][\w$]*):/gm)) count(m[1]);
  // A control keyword at method indent (`    if (…)`) is not a key. Without
  // this the scan reports `if` as declared three times.
  const KEYWORD = new Set(["if", "for", "while", "switch", "catch", "return", "do", "else"]);
  for (const m of body.matchAll(/^ {4}(?:async )?([A-Za-z_$][\w$]*)\s*\(/gm)) {
    if (!KEYWORD.has(m[1])) count(m[1]);
  }
  const dupes = [...keys].filter(([, n]) => n > 1).map(([k]) => k);
  // NEGATIVE CONTROL: the scan must actually reach the literal. A regex that
  // matched nothing would report no duplicates forever.
  assert.ok(
    keys.size > 25,
    `the key scan found only ${keys.size} entries — it is not reading the state literal`,
  );
  assert.deepEqual(
    dupes,
    [],
    "a duplicate key is not an error in sloppy mode — the last declaration " +
      "silently wins and every write against the first shape becomes a no-op",
  );
});

test("rowOpen compares by composite ref, not by slug", () => {
  // Two peers can host the same slug (ADR-0052 §5); only the ref tells them
  // apart, and the open-row highlight is where conflating them shows.
  const own = loadShell().state;
  const a = { slug: "owner/repo", daemon: false };
  own.$store.projects.setOpen(own.$store.projects.repoRef(a));
  assert.equal(own.$store.projects.rowOpen(a), true);
  assert.equal(own.$store.projects.rowOpen({ slug: "owner/other", daemon: false }), false);
});

test("filteredProjects keeps the open project whatever the query", () => {
  const own = loadShell().state;
  const a = { slug: "owner/alpha", branch: "main", path: "C:\\src\\alpha" };
  const b = { slug: "owner/beta", branch: "main", path: "C:\\src\\beta" };
  own.$store.projects.setProjects([a, b]);
  own.$store.projects.setOpen(own.$store.projects.repoRef(a));

  // The open project's files sit under the open row; a query that matches
  // nothing must not drop that row.
  own.projectQuery = "zzz-matches-nothing";
  assert.deepEqual(own.filteredProjects(), [a]);

  // A query that matches only the sibling keeps both: the sibling because it
  // matches, the open row because it is open.
  own.projectQuery = "beta";
  assert.deepEqual(own.filteredProjects(), [a, b]);

  // NEGATIVE CONTROL: the pin is the open row, not a change to matching — with
  // nothing open the same query filters everything out.
  own.$store.projects.setOpen(null);
  own.projectQuery = "zzz-matches-nothing";
  assert.deepEqual(own.filteredProjects(), []);
});

// A shell whose `toggle` side effects are recorders: each method `toggle` and
// the `workbench:project-changed` listeners reach is replaced, so the test
// sees the calls they make. The window hears the events it is sent, and
// `init()` registers the listeners, as on the page.
function toggleShell() {
  const listeners = {};
  const events = [];
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        events.push(e);
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const { state, window } = loaded;
  const calls = [];
  const record = (name) => (...args) => {
    calls.push([name, ...args]);
  };
  for (const name of ["loadRepos", "wakePeerFor", "loadAgents"]) state.$store.projects[name] = record(name);
  state.$store.checkouts.ensureWorktreeListing = record("ensureWorktreeListing");
  const runs = state.$store.runs;
  for (const name of ["destroyRunsSub", "mountRunsSub", "hydrateRuns"]) runs[name] = record(name);
  runs.projectRuns = () => [];
  runs.currentRun = () => null;
  state.$nextTick = (fn) => fn();
  runs.init();
  // `init()` starts its clocks on the global timers; here they never tick.
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = () => 0;
  try {
    state.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  calls.length = 0;
  return { state, window, loaded, calls, events, named: (name) => calls.filter((c) => c[0] === name) };
}

test("opening a row asks to wake its peer, and closing it does not", () => {
  const { state, named } = toggleShell();
  state.toggle("peer:wsl/owner/repo");
  assert.deepEqual(named("wakePeerFor"), [["wakePeerFor", "peer:wsl/owner/repo"]]);
  // CONTROL: the same row again closes it, and a closing row wakes nothing.
  state.toggle("peer:wsl/owner/repo");
  assert.equal(state.$store.projects.openSlug, null);
  assert.equal(named("wakePeerFor").length, 1);
});

test("opening a project row asks the Spend tab to read again", () => {
  const { state, events } = toggleShell();
  state.toggle("owner/repo");
  assert.equal(events.filter((e) => e.type === "workbench:spend-refresh").length, 1);
});

test("a row on a host that cannot answer stays closed, and still wakes it", () => {
  const { state, named, events } = toggleShell();
  const peer = "01KY0000000000000000000000";
  const ref = `${peer}/owner/repo`;
  let groups = [{ daemon: peer, local: false, state: "unreachable" }];
  state.$store.projects.fleetGroups = () => groups;
  const opened = () => events.filter((e) => e.type === "workbench:project-changed").length;
  state.toggle(ref);
  assert.equal(state.$store.projects.openSlug, null);
  assert.equal(opened(), 0);
  assert.deepEqual(named("wakePeerFor"), [["wakePeerFor", ref]]);
  // CONTROL: once the host answers, the same click opens the row.
  groups = [{ daemon: peer, local: false, state: "reachable" }];
  state.toggle(ref);
  assert.equal(state.$store.projects.openSlug, ref);
  assert.equal(opened(), 1);
});

// `toggle` changes the store and sends `workbench:project-changed` on the
// window; the files, the board and the git part each listen to it (ADR-0073
// amendment of 2026-10-08, decision 4).
test("toggle sends workbench:project-changed with the project now open and the one that was", () => {
  const { state, events } = toggleShell();
  state.toggle("owner/a");
  state.toggle("owner/b");
  state.toggle("owner/b");
  assert.deepEqual(
    events.filter((e) => e.type === "workbench:project-changed").map((e) => e.detail),
    [
      { slug: "owner/a", previous: null },
      { slug: "owner/b", previous: "owner/a" },
      { slug: null, previous: "owner/b" },
    ],
  );
});

test("toggle writes no field of the git part", () => {
  // The default page: the window hears nothing, so no listener runs.
  const { state } = loadShell();
  for (const name of ["wakePeerFor", "loadAgents"]) state.$store.projects[name] = () => {};
  state.$store.checkouts.ensureWorktreeListing = () => {};
  state.$store.checkouts.branchRefused("a branch refusal");
  state.$store.runs.setCurrentRun("run-1");
  state.toggle("owner/b");
  assert.equal(state.$store.projects.openSlug, "owner/b");
  assert.equal(state.$store.checkouts.branchError, "a branch refusal");
  assert.equal(state.$store.runs.currentRunId, "run-1");
});

// Each part resets only its own state, so the order of the listeners is not
// a contract: each part reacts, in its own order.
test("the tree, the board and git each react to the project toggle opens", () => {
  const { state, loaded, calls } = toggleShell();
  const files = loadComponent("wbFiles", { from: loaded, magics: { $nextTick: (fn) => fn() } }).scope;
  files.destroyTree = () => calls.push(["destroyTree"]);
  files.mountTree = () => calls.push(["mountTree"]);
  files.init();
  const board = loadComponent("wbBoard", { from: loaded, magics: { $nextTick: (fn) => fn() } }).scope;
  board.loadBoard = () => calls.push(["loadBoard"]);
  // The board's `init()` starts its backstop on the global timer; here it never ticks.
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = () => 0;
  try {
    board.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  const changes = loadComponent("wbChanges", { from: loaded, magics: { $nextTick: (fn) => fn() } }).scope;
  for (const name of ["destroyChangesSub", "mountChangesSub", "loadChanges", "loadSync"]) {
    changes[name] = () => calls.push([name]);
  }
  globalThis.setInterval = () => 0;
  try {
    changes.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  state.kanbanOpen = true;
  state.toggle("owner/repo");
  const seen = calls.map((c) => c[0]);
  const parts = {
    tree: ["destroyTree", "mountTree"],
    runs: ["destroyRunsSub", "mountRunsSub", "hydrateRuns"],
    board: ["loadBoard"],
    git: ["destroyChangesSub", "mountChangesSub", "loadChanges", "loadSync"],
  };
  for (const [part, names] of Object.entries(parts)) {
    assert.deepEqual(seen.filter((n) => names.includes(n)), names, part);
  }
});

test("removing the open project closes it and sends workbench:project-changed", async () => {
  const { state, events, window } = toggleShell();
  window.WBDaemon.observe = async () => ({ status: "ok" });
  state.$store.modals.askConfirm = async () => true;
  const a = { slug: "owner/a" };
  const b = { slug: "owner/b" };
  state.$store.projects.setProjects([a, b]);
  state.$store.projects.setOpen("owner/a");
  const changed = () => events.filter((e) => e.type === "workbench:project-changed");
  // CONTROL: a project that is not open goes without the event.
  await state.removeProject(b);
  assert.equal(changed().length, 0);
  await state.removeProject(a);
  assert.equal(state.$store.projects.openSlug, null);
  assert.deepEqual(changed().map((e) => [e.type, e.detail]), [
    ["workbench:project-changed", { slug: null, previous: "owner/a" }],
  ]);
});

test("the create action asks for the name through the prompt dialog", async () => {
  const listeners = [];
  const { window } = loadShell({
    document: {
      addEventListener: (type, fn) => type === "workbench:action" && listeners.push(fn),
    },
  });
  const written = [];
  window.WBDaemon = { write: (verb, payload) => written.push({ verb, payload }) };
  const asked = [];
  window.Alpine.store("modals").askPrompt = async (opts) => {
    asked.push(opts);
    return null;
  };
  window.getShell = () => ({});
  const prompted = [];
  window.prompt = (msg) => {
    prompted.push(msg);
    return null;
  };
  assert.ok(listeners.length > 0, "app.ts subscribes to workbench:action at load");
  await Promise.all(
    listeners.map((fn) => fn({ detail: { action: "create", project: "owner/repo", path: "src", kind: "file" } })),
  );
  assert.deepEqual(
    asked.map((o) => o.title),
    ["New file in src"],
    "the name is asked once, through askPrompt",
  );
  assert.deepEqual(prompted, [], "the browser's prompt is only the no-shell fallback");
  // A cancelled prompt creates nothing.
  assert.deepEqual(written, []);
});

// --- ADR-0070 D2: a shown fact is read again on named events only ----------

// The login gate (wb-login.ts) sends `workbench:logged-in` once the daemon
// accepted a login: the shell reads the desk and restores the open tabs (the
// store `tabs`), and both were
// refused while gated (#327, #339).
test("a login the daemon accepted reads the desk again and restores the view", async () => {
  const listeners = {};
  const { state, window } = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
    },
  });
  const calls = [];
  window.WBConsole.afterLogin = () => (calls.push("afterLogin"), Promise.resolve());
  state.$store.checkouts.adoptDeskCheckouts = () => calls.push("adoptDeskCheckouts");
  state.$store.tabs.restoreView = () => calls.push("restoreView");
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = () => 0;
  try {
    state.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  window.dispatchEvent(new CustomEvent("workbench:logged-in"));
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(calls, ["afterLogin", "restoreView", "adoptDeskCheckouts"]);
});

// --- ADR-0070 D3: a failed read keeps the last good value, marked not current

// A shell whose `WBDaemon.observe` answers each call with the next reply.
function observedShell(replies) {
  const { state, window } = loadShell();
  window.WBDaemon.observe = async () => replies.shift() ?? null;
  state.$store.projects.setOpen("o/r");
  state.$store.modals.flash = () => {};
  return state;
}

// --- the shell and the files (wb-files.ts) talk through events ---------------

// A shell whose window records every event it is sent.
function eventShell(opts = {}) {
  const events = [];
  const loaded = loadShell({ ...opts, window: { dispatchEvent: (e) => (events.push(e), true) } });
  const sent = (type) => events.filter((e) => e.type === type).map((e) => e.detail ?? null);
  return { ...loaded, events, sent };
}

test("a create through the write seam asks the files to re-list the folder and reveal the entry", async () => {
  const listeners = [];
  const { window, sent } = eventShell({
    document: { addEventListener: (type, fn) => type === "workbench:action" && listeners.push(fn) },
  });
  window.WBDaemon = { write: async () => ({ status: "ok" }), withCheckout: (p) => p };
  const realDaemon = globalThis.WBDaemon;
  globalThis.WBDaemon = window.WBDaemon;
  window.getShell = () => ({});
  window.Alpine.store("tabs").openTab = () => {};
  window.Alpine.store("modals").askPrompt = async () => "x.txt";
  window.Alpine.store("modals").flash = () => {};
  try {
    for (const fn of listeners) await fn({ detail: { action: "create", project: "o/r", path: "src", kind: "file" } });
    await new Promise((r) => setImmediate(r));
  } finally {
    globalThis.WBDaemon = realDaemon;
  }
  assert.deepEqual(sent("workbench:tree-dirty"), [{ rel: "src", reveal: "src/x.txt" }]);
});

// --- second review of #511 ---------------------------------------------------

// A shell whose window hears what it is sent, after `init()` as on the page.
function runsEventShell({ visibilityState = "visible" } = {}) {
  const listeners = {};
  const loaded = loadShell({
    document: { visibilityState, hasFocus: () => true },
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
    },
  });
  const { state } = loaded;
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = () => 0;
  try {
    state.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  return loaded;
}

// The Alpine store `runs` tells the page; the shell decides what each fact
// means for it.
test("a runs push on a visible tab reads the runs and nudges the board; a hidden tab does neither", () => {
  for (const visibilityState of ["visible", "hidden"]) {
    const { state, window } = runsEventShell({ visibilityState });
    const calls = [];
    state.$store.runs.hydrateRuns = () => calls.push("hydrateRuns");
    window.addEventListener("workbench:board-refresh", (e) => calls.push(`board:${e.detail.trigger}`));
    window.dispatchEvent(new CustomEvent("workbench:runs-dirty"));
    assert.deepEqual(calls, visibilityState === "visible" ? ["hydrateRuns", "board:runs"] : [], visibilityState);
  }
});

test("a reopened socket refreshes an open board, as the tab becoming visible does", () => {
  const { window } = loadShell();
  const ask = (trigger) =>
    window.WBKanban.shouldRefresh({ trigger, sinceMs: 60 * 60 * 1000, boardOpen: true, docVisible: true });
  assert.equal(ask("reopen"), true);
  assert.equal(ask("reopen"), ask("visible"));
});

test("Remove project names a remoteless repo by its folder, not its path- slug", async () => {
  const { state } = loadShell();
  const asked = [];
  state.$store.modals.askConfirm = async (o) => {
    asked.push(o.message);
    return false;
  };
  await state.removeProject({ slug: "path-8ee0b8b587ea7891", name: "widget", path: "/home/me/widget/" });
  await state.removeProject({ slug: "owner/repo", path: "/home/me/elsewhere" });
  assert.deepEqual(asked, [
    "Remove “widget” from Ralphy? Files on disk are kept.",
    "Remove “owner/repo” from Ralphy? Files on disk are kept.",
  ]);
});

test("projectLabel and projectTitle never print a path- key or a daemon id", () => {
  const { state } = loadShell();
  state.$store.projects.setProjects([
    { slug: "path-8ee0b8b587ea7891", name: "widget", path: "C:/Dev/widget" },
    { key: "01KYPEER/path-1234", slug: "path-1234", name: "gadget", path: "/home/me/gadget", daemon: "01KYPEER", env: "WSL: Ubuntu" },
    { slug: "owner/repo", name: "owner/repo", path: "C:/Dev/repo" },
  ]);
  const ref = (p) => state.$store.projects.repoRef(p);
  const [local, peer, forge] = state.$store.projects.projects;
  assert.equal(state.$store.projects.projectLabel(ref(local)), "widget");
  assert.equal(state.$store.projects.projectTitle(ref(local)), "C:/Dev/widget");
  assert.equal(state.$store.projects.projectLabel(ref(peer)), "gadget · WSL: Ubuntu");
  assert.equal(state.$store.projects.projectTitle(ref(peer)), "/home/me/gadget · WSL: Ubuntu");
  assert.equal(state.$store.projects.projectLabel(ref(forge)), "owner/repo");
  assert.equal(state.$store.projects.projectTitle(ref(forge)), "owner/repo");
});

test("a read failure shows the words for a daemon code, never the code", async () => {
  const state = observedShell([{ status: "error", reason: "unknown repo" }]);
  await state.$store.runs.hydrateRuns();
  assert.equal(state.$store.runs.runsError, "Could not read the runs: the project is not in the list.");
});

test("peer states show as words, and the dot says what it means", () => {
  const { state } = loadShell();
  assert.equal(state.peerStateWord("version-mismatch"), "version mismatch");
  assert.equal(state.peerStateWord("asleep"), "asleep");
  assert.equal(state.dotTitle("offline"), "The folder cannot be reached");
  assert.equal(state.dotTitle("idle"), "No console is open");
});

// --- the write actions of the seam: delete, rename, worktree-created --------

// A shell whose `workbench:action` listeners can be fired, with a daemon that
// answers every write with `reply` and records it.
function writeSeam(reply) {
  const listeners = [];
  const { window, sent } = eventShell({
    document: { addEventListener: (type, fn) => type === "workbench:action" && listeners.push(fn) },
  });
  const written = [];
  window.WBDaemon = {
    write: async (verb, payload) => (written.push({ verb, payload }), reply),
    withCheckout: (p) => p,
  };
  const flashed = [];
  window.Alpine.store("modals").flash = (m) => flashed.push(m);
  const answer = { yes: true };
  window.Alpine.store("modals").askConfirm = async () => answer.yes;
  const act = async (detail) => {
    const realDaemon = globalThis.WBDaemon;
    globalThis.WBDaemon = window.WBDaemon;
    try {
      for (const fn of listeners) await fn({ detail });
      await new Promise((r) => setImmediate(r));
    } finally {
      globalThis.WBDaemon = realDaemon;
    }
  };
  return { window, sent, written, flashed, answer, act };
}

test("a delete that the daemon answers 'not found' flashes the refusal and re-lists the parent", async () => {
  const { sent, written, flashed, act } = writeSeam({ status: "error", message: "not found" });
  await act({ action: "delete", project: "o/r", path: "src/a/gone.txt", title: "gone.txt" });
  assert.deepEqual(written.map((w) => [w.verb, w.payload.path]), [["file.delete", "src/a/gone.txt"]]);
  assert.equal(flashed.length, 1);
  assert.match(flashed[0], /Could not delete/);
  assert.deepEqual(sent("workbench:tree-dirty"), [{ rel: "src/a" }]);
});

test("a delete that works says so and re-lists nothing; one that is refused for another reason re-lists nothing", async () => {
  let seam = writeSeam({ status: "ok" });
  await seam.act({ action: "delete", project: "o/r", path: "src/a.txt", title: "a.txt" });
  assert.deepEqual(seam.flashed, ["a.txt deleted."]);
  assert.deepEqual(seam.sent("workbench:tree-dirty"), []);

  seam = writeSeam({ status: "error", message: "permission denied" });
  await seam.act({ action: "delete", project: "o/r", path: "src/a.txt" });
  assert.match(seam.flashed[0], /Could not delete/);
  assert.deepEqual(seam.sent("workbench:tree-dirty"), []);
});

test("a delete the operator declines writes nothing", async () => {
  const { answer, written, flashed, act } = writeSeam({ status: "ok" });
  answer.yes = false;
  await act({ action: "delete", project: "o/r", path: "src/a.txt" });
  assert.deepEqual(written, []);
  assert.deepEqual(flashed, []);
});

test("a rename writes the full paths and flashes only a refusal", async () => {
  let seam = writeSeam({ status: "ok" });
  await seam.act({ action: "rename", project: "o/r", from: "src/a.txt", to: "src/b.txt" });
  assert.deepEqual(seam.written.map((w) => [w.verb, w.payload.path, w.payload.to]), [["file.rename", "src/a.txt", "src/b.txt"]]);
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(seam.flashed, []);

  seam = writeSeam({ status: "error", message: "already exists" });
  await seam.act({ action: "rename", project: "o/r", from: "src/a.txt", to: "src/b.txt" });
  await new Promise((r) => setImmediate(r));
  assert.equal(seam.flashed.length, 1);
  assert.match(seam.flashed[0], /Could not rename/);
});

test("a worktree-created action re-reads the worktree list and flashes its message as one line", async () => {
  const { window, flashed, act } = writeSeam({ status: "ok" });
  const reads = [];
  window.Alpine.store("checkouts").ensureWorktreeListing = (repo, force) => reads.push([repo, force]);
  await act({ action: "worktree-created", project: "o/r", message: "Created.\nPrepared." });
  assert.deepEqual(reads, [["o/r", true]]);
  assert.deepEqual(flashed, ["Created. Prepared."]);
  await act({ action: "worktree-created", project: "o/r" });
  assert.equal(flashed.length, 1, "no message, no flash");
  assert.equal(reads.length, 2);
});
