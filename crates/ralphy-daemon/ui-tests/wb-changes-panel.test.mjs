// Unit tests for assets/ui/wb-changes-panel.ts, the Alpine component of the
// Changes view: the change set and the branch of each project, their reads,
// the remote acts, the rows, the commit box and the refusal line. The
// component is built with `loadComponent`, so every test here also fails when
// it reads a shell() name it does not list in `uses`, or assigns a shell()
// field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// `init()` starts the backstop on the global timer; here it never ticks.
// Returns the intervals it asked for.
function initQuietly(scope) {
  const asked = [];
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = (fn, ms) => (asked.push({ fn, ms }), 0);
  try {
    scope.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  return asked;
}

// One page with the shell and the Changes view, and a window that delivers
// the events it is sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds the component.
function page(opts = {}) {
  const listeners = {};
  const loaded = loadShell({
    ...opts,
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const changes = loadComponent("wbChanges", { from: loaded, magics: { $nextTick: (fn) => fn() } });
  const timers = initQuietly(changes.scope);
  const fire = (type, detail) => loaded.window.dispatchEvent({ type, detail });
  return { loaded, shell: loaded.state, changes: changes.scope, window: loaded.window, fire, timers };
}

// A page whose `WBDaemon.observe` answers each call with the next reply.
function observedPage(replies) {
  const p = page();
  p.window.WBDaemon.observe = async () => replies.shift() ?? null;
  p.shell.$store.projects.setOpen("o/r");
  p.shell.$store.modals.flash = () => {};
  return p;
}

const CHANGES_OK = {
  status: "ok",
  changes: { changes: [{ path: "a.txt", index: " ", worktree: "M" }] },
};

// ---- the component and its markup ---------------------------------------------

test("wbChanges lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbChanges");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbChanges reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbChanges");
  const markup = componentMarkup(HTML, "wbChanges");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variable of the two groups of rows, and a global.
  const loopVars = new Set(["c", "Boolean"]);
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component.
    if (attr === "x-data") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 40, `the markup was read: ${read} names`);
});

test("nothing outside the Changes view element names its state", () => {
  const start = HTML.indexOf('x-data="wbChanges"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["changesError", "changesCount", "changesReadError", "changesStaged", "changesUnstaged", "syncByProject", "syncBusy", "commitMsg", "loadChanges", "loadSync", "stagePaths", "discardRow", "commitStaged", "syncPush", "pullBlocked", "rowActTitle"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the Changes view", () => {
  const { changes, fire } = page();
  const calls = [];
  changes.gitFollowProject = (slug) => calls.push(`follow:${slug}`);
  changes.gitFollowHead = (ref) => calls.push(`head:${ref}`);
  changes.loadChanges = (ref) => calls.push(`changes:${ref}`);
  changes.loadSync = (ref) => calls.push(`sync:${ref}`);
  changes.refreshChanges = () => calls.push("refresh");
  changes._changesSub = { resume: (stale) => calls.push(`resume:${stale}`) };
  fire("workbench:project-changed", { slug: "o/b", previous: "o/a" });
  fire("workbench:head-moved", { ref: "o/r" });
  fire("workbench:changes-read", { ref: "o/r" });
  fire("workbench:changes-refresh", null);
  fire("workbench:sockets-resume", { stale: true });
  assert.deepEqual(calls, ["follow:o/b", "head:o/r", "changes:o/r", "sync:o/r", "refresh", "resume:true"]);
});

// The shell asks the Changes view for a read, and never calls it.
test("opening the Changes view, the sidebar refresh and a branch act read the change set", async () => {
  const { loaded, shell, changes, window } = page({ document: { visibilityState: "visible" } });
  const reads = [];
  changes.loadChanges = (ref) => reads.push(`changes:${ref}`);
  changes.loadSync = (ref) => reads.push(`sync:${ref}`);
  shell.$store.projects.setOpen("o/r");
  shell.sideOpen = true;
  shell.showSideView("changes");
  assert.deepEqual(reads, ["changes:o/r", "sync:o/r"], "opening the view");
  reads.length = 0;
  window.WBDaemon.observe = async () => ({ status: "ok" });
  shell.$store.checkouts.ensureWorktreeListing = () => {};
  const dialog = loadComponent("wbBranchDialog", { from: loaded }).scope;
  await dialog._mutateBranch("branch.switch", "o/r", "side", () => {});
  assert.deepEqual(reads, ["changes:o/r", "sync:o/r"], "a branch act");
});

test("the project rows read the change count the Changes view holds", () => {
  const { shell, changes } = page();
  changes.changesCount = { "o/r": 3 };
  assert.deepEqual(shell.projectBadge("o/r"), { show: true, text: "3", title: "3 changed" });
  // CONTROL: a slug the view never read shows no badge.
  assert.equal(shell.projectBadge("o/x").show, false);
});

test("init starts the 50 s backstop, which reads only while the Changes view is on screen", () => {
  const { shell, changes, timers } = page({ document: { visibilityState: "visible" } });
  assert.deepEqual(timers.map((t) => t.ms), [50000]);
  const reads = [];
  changes.loadChanges = (ref) => reads.push(`changes:${ref}`);
  changes.loadSync = (ref) => reads.push(`sync:${ref}`);
  shell.$store.projects.setOpen("o/r");
  shell.sideOpen = true;
  shell.sideView = "projects";
  timers[0].fn();
  assert.deepEqual(reads, [], "the Projects view is on screen");
  shell.sideView = "changes";
  timers[0].fn();
  assert.deepEqual(reads, ["changes:o/r", "sync:o/r"]);
});

test("opening a row remounts the run-completion subscription", () => {
  const { shell, changes } = page();
  const calls = [];
  changes.destroyChangesSub = () => calls.push("destroyChangesSub");
  changes.mountChangesSub = () => calls.push("mountChangesSub");
  changes.loadChanges = () => {};
  changes.loadSync = () => {};
  for (const name of ["wakePeerFor", "loadAgents"]) shell[name] = () => {};
  shell.$store.checkouts.ensureWorktreeListing = () => {};
  shell.toggle("owner/repo");
  // The old socket closes before the new one opens, so a nudge for the
  // project that WAS open never reloads the new one.
  assert.deepEqual(calls, ["destroyChangesSub", "mountChangesSub"]);
});

test("the git part drops the refusal and the commit message of the project that was open", () => {
  const { shell, changes } = page();
  const calls = [];
  changes.destroyChangesSub = () => {};
  changes.mountChangesSub = () => {};
  changes.loadChanges = (ref) => calls.push(`changes:${ref}`);
  changes.loadSync = (ref) => calls.push(`sync:${ref}`);
  shell.$store.projects.setOpen("owner/b");
  Object.assign(changes, { changesError: "x", commitMsg: "unsent", commitMsgSlug: "owner/a" });
  changes.gitFollowProject("owner/b");
  assert.deepEqual([changes.changesError, changes.commitMsg, changes.commitMsgSlug], ["", "", "owner/b"]);
  assert.deepEqual(calls, ["changes:owner/b", "sync:owner/b"]);
  // CONTROL: a message written for the project now open stays.
  changes.commitMsg = "kept";
  changes.gitFollowProject("owner/b");
  assert.equal(changes.commitMsg, "kept");
});

test("a moved HEAD re-reads the change set and the sync row, and the listing under a selection", () => {
  const { shell, changes } = page();
  const calls = [];
  for (const name of ["loadChanges", "loadSync"]) changes[name] = (ref) => calls.push(`${name} ${ref}`);
  shell.$store.checkouts.ensureWorktreeListing = (ref, force) => calls.push(`listing ${ref} ${force}`);
  changes.gitFollowHead("o/r");
  assert.deepEqual(calls, ["loadChanges o/r", "loadSync o/r"]);
  shell.$store.checkouts.checkouts = { "o/r": "wt-a" };
  calls.length = 0;
  changes.gitFollowHead("o/r");
  assert.deepEqual(calls, ["loadChanges o/r", "loadSync o/r", "listing o/r true"]);
});

test("a new checkout's change set does not inherit the old tree's last read", () => {
  const { shell, changes } = page();
  changes.loadChanges = () => {};
  changes.loadSync = () => {};
  changes.changesRead["o/r"] = { value: true, goodAt: 5, error: "", current: true };
  changes.syncRead["o/r"] = { value: true, goodAt: 5, error: "", current: true };
  shell.$store.checkouts.setCheckout("o/r", "wt-a");
  assert.equal(changes.changesRead["o/r"], undefined);
  assert.equal(changes.syncRead["o/r"], undefined);
});

// ---- the acts of the panel ---------------------------------------------------------

// Every act the Changes panel dispatches, each answering with a refusal.
const CHANGE_ACTS = [
  ["syncFetch", (st) => st.syncFetch("o/r")],
  ["syncPull", (st) => st.syncPull("o/r")],
  ["syncPush", (st) => st.syncPush("o/r")],
  ["stagePaths", (st) => st.stagePaths("o/r", ["a.txt"])],
  ["unstagePaths", (st) => st.unstagePaths("o/r", ["a.txt"])],
  ["discardRow", (st) => st.discardRow("o/r", { path: "a.txt", status: "modified" })],
  [
    "commitStaged",
    (st) => {
      st.commitMsgSlug = "o/r";
      st.commitMsg = "a message";
      return st.commitStaged("o/r");
    },
  ],
];

test("a refused Changes act lands in the Changes panel and still flashes", async () => {
  for (const [act, run] of CHANGE_ACTS) {
    const { shell, changes: st, window } = page();
    const said = `The daemon refused ${act}.`;
    window.WBDaemon.observe = async () => ({ status: "error", message: said });
    st.loadChanges = () => {};
    st.loadSync = () => {};
    shell.$store.modals.askConfirm = async () => true;
    const flashed = [];
    shell.$store.modals.flash = (msg) => flashed.push(msg);
    await run(st);
    assert.equal(st.changesError, said, `${act}: the panel shows the refusal`);
    assert.deepEqual(flashed, [said], `${act}: the flash still carries it`);
  }
});

test("one remote act at a time: the busy slot refuses a second act and frees itself on every exit", async () => {
  const { shell, changes: st, window } = page();
  st.loadChanges = () => {};
  st.loadSync = () => {};
  shell.$store.modals.flash = () => {};
  const sent = [];
  const answers = [];
  window.WBDaemon.observe = (verb) => {
    sent.push(verb);
    return new Promise((resolve) => answers.push(resolve));
  };
  const acts = [st.syncFetch("o/r")];
  assert.equal(st.syncBusy, "fetch");
  acts.push(st.syncPull("o/r"), st.syncPush("o/r"));
  const whileBusy = [...sent];
  for (const answer of answers) answer({ status: "ok" });
  await Promise.all(acts);
  assert.deepEqual(whileBusy, ["sync.fetch"], "an act that finds the slot taken sends nothing");
  assert.equal(st.syncBusy, null, "an answered act frees the slot");

  window.WBDaemon.observe = async () => ({ status: "error", message: "The push was refused." });
  await st.syncPush("o/r");
  assert.equal(st.syncBusy, null, "a refused act frees the slot");

  window.WBDaemon.observe = async (verb) => {
    sent.push(verb);
    throw new Error("the socket closed");
  };
  await st.syncPull("o/r");
  assert.equal(st.syncBusy, null, "an act whose transport threw frees the slot");
  assert.deepEqual(sent, ["sync.fetch", "sync.pull"], "the freed slot takes the next act");
});

// ---- ADR-0070 D3: a shown fact that is not current -------------------------------

test("a failed change-set read after a good one keeps the groups, marked not current", async () => {
  const { changes: state } = observedPage([CHANGES_OK, { status: "error", message: "git exited 128" }]);
  await state.loadChanges("o/r");
  const before = state.changesCount["o/r"];
  assert.ok(before > 0, "the first read found a change");
  await state.loadChanges("o/r");
  assert.equal(state.changesCount["o/r"], before);
  assert.equal(state.changesRead["o/r"].current, false);
  assert.match(state.changesReadError["o/r"], /Not current: git exited 128/);
});

// ADR-0070 D3: a write acts on what the page shows, so it is locked while that
// is not current, and says why.
test("writes are locked while the change set is not current", async () => {
  const { shell, changes } = observedPage([CHANGES_OK, { status: "error", message: "git exited 128" }]);
  shell.$store.runs.runsByProject["o/r"] = [];
  await changes.loadChanges("o/r");
  assert.equal(shell.$store.connection.writeLocked(), false, "a good read locks nothing");
  await changes.loadChanges("o/r");
  assert.equal(shell.$store.connection.writeLocked(), true);
  assert.match(shell.$store.connection.writeLockReason(), /not current/);
});

// NEGATIVE CONTROL: two good reads lock nothing.
test("two good change-set reads lock no write", async () => {
  const { shell, changes } = observedPage([CHANGES_OK, CHANGES_OK]);
  shell.$store.runs.runsByProject["o/r"] = [];
  await changes.loadChanges("o/r");
  await changes.loadChanges("o/r");
  assert.equal(shell.$store.connection.writeLocked(), false);
});

test("a failed first change-set or branch read says why", async () => {
  const { changes } = observedPage([{ status: "error", message: "git exited 128" }]);
  await changes.loadChanges("o/r");
  assert.match(changes.changesReadError["o/r"], /Could not read the changes: git exited 128/);
  const { changes: sync } = observedPage([{ status: "error", message: "not a git repository" }]);
  await sync.loadSync("o/r");
  assert.match(sync.syncByProject["o/r"].note, /not a git repository/);
});

// ADR-0070 D3: each write action asks the lock itself, not only its button.
test("every change-set write is refused while the change set is not current", async () => {
  const { shell, changes: state, window } = page();
  const replies = [CHANGES_OK, { status: "error", message: "git exited 128" }];
  const verbs = [];
  window.WBDaemon.observe = async (verb) => {
    verbs.push(verb);
    return replies.shift() ?? { status: "ok" };
  };
  shell.$store.projects.setOpen("o/r");
  shell.$store.modals.flash = () => {};
  shell.$store.runs.runsByProject["o/r"] = [];
  await state.loadChanges("o/r");
  await state.loadChanges("o/r");
  assert.equal(shell.$store.connection.writeLocked(), true);
  verbs.length = 0;
  shell.$store.modals.askConfirm = async () => true;
  state.commitMsgSlug = "o/r";
  state.commitMsg = "msg";
  await state.stagePaths("o/r", ["a.txt"]);
  await state.unstagePaths("o/r", ["a.txt"]);
  await state.discardRow("o/r", { path: "a.txt", index: " ", worktree: "M" });
  await state.commitStaged("o/r");
  await state.syncFetch("o/r");
  await state.syncPull("o/r");
  await state.syncPush("o/r");
  assert.deepEqual(verbs, [], "no write reached the daemon");
});

// NEGATIVE CONTROL for the refusal above: with a current change set, each
// action does reach the daemon, so the refusal is the lock and not a missing
// argument.
test("every change-set write reaches the daemon while the change set is current", async () => {
  const acts = {
    stage: (s) => s.stagePaths("o/r", ["a.txt"]),
    unstage: (s) => s.unstagePaths("o/r", ["a.txt"]),
    discard: (s) => s.discardRow("o/r", { path: "a.txt", index: " ", worktree: "M" }),
    commit: (s) => s.commitStaged("o/r"),
    fetch: (s) => s.syncFetch("o/r"),
    pull: (s) => s.syncPull("o/r"),
    push: (s) => s.syncPush("o/r"),
  };
  for (const [name, act] of Object.entries(acts)) {
    const { shell, changes: state, window } = page();
    const replies = [CHANGES_OK];
    const verbs = [];
    window.WBDaemon.observe = async (verb) => {
      verbs.push(verb);
      return replies.shift() ?? { status: "ok" };
    };
    shell.$store.projects.setOpen("o/r");
    shell.$store.modals.flash = () => {};
    shell.$store.runs.runsByProject["o/r"] = [];
    await state.loadChanges("o/r");
    assert.equal(shell.$store.connection.writeLocked(), false, name);
    verbs.length = 0;
    shell.$store.modals.askConfirm = async () => true;
    state.commitMsgSlug = "o/r";
    state.commitMsg = "msg";
    await act(state);
    assert.ok(verbs.length > 0, `${name} reached the daemon`);
  }
});

test("a read failure shows the words for a daemon code, never the code", async () => {
  const { changes } = observedPage([
    { status: "error", message: "unknown repo" },
    { status: "error", message: "unknown checkout" },
  ]);
  await changes.loadChanges("o/r");
  assert.equal(changes.changesReadError["o/r"], "Could not read the changes: the project is not in the list");
  await changes.loadSync("o/r");
  assert.equal(changes.syncByProject["o/r"].note, "Could not read the branch: the worktree does not exist");
});
