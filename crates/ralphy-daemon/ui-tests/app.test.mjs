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
import { loadComponent, loadShell } from "./harness.mjs";

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
  window.WBDaemon.observe = () => Promise.resolve({ status: "ok" });
  state.$store.modals.askConfirm = () => Promise.resolve(true);
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
  window.Alpine.store("modals").askPrompt = (opts) => {
    asked.push(opts);
    return Promise.resolve(null);
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
