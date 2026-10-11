// Unit tests for assets/ui/wb-runs-store.ts, the Alpine store `runs`: the runs
// of each project, their read state, the selected run and the runs socket. It
// is built here without Alpine (ADR-0073 D3, D6), as `main.ts` builds it for
// `Alpine.store`, on a page stub that gives it the `projects` store, the
// daemon door and a window that hears what it is sent.
import { test } from "node:test";
import assert from "node:assert/strict";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";
import { runsStore } from "../assets/ui/wb-runs-store.ts";

// `observe` answers `runs.list`. The open project is `o/r`.
function page(observe = async () => null) {
  const projects = projectsStore();
  projects.setOpen("o/r");
  const listeners = {};
  const events = [];
  const subs = [];
  globalThis.window = {
    Alpine: { store: (name) => (name === "projects" ? projects : undefined), nextTick: (fn) => fn() },
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
    dispatchEvent: (e) => {
      events.push(e.type);
      for (const fn of listeners[e.type] || []) fn(e);
      return true;
    },
    WBDaemon: {
      observe,
      subscribeRuns: (repo, onDirty) => {
        const sub = { repo, onDirty, closed: false, close: () => (sub.closed = true) };
        subs.push(sub);
        return sub;
      },
    },
  };
  const store = runsStore({ messages: null });
  return { store, projects, events, subs, window: globalThis.window };
}

// Answers each call with the next reply in `replies`.
const scripted = (replies) => async () => replies.shift() ?? null;

test("a failed runs read after a good one keeps the runs, marked not current", async () => {
  const { store } = page(scripted([{ status: "ok", runs: [] }, { status: "error", reason: "the run store is locked" }]));
  await store.hydrateRuns();
  store.runsByProject["o/r"] = [{ runid: "r1" }];
  await store.hydrateRuns();
  assert.deepEqual(store.runsByProject["o/r"], [{ runid: "r1" }]);
  assert.equal(store.runsRead["o/r"].current, false);
  assert.match(store.runsError, /Not current: the run store is locked/);
});

test("a failed first runs read shows the words for a daemon code, never the code", async () => {
  const { store } = page(scripted([{ status: "error", reason: "unknown repo" }]));
  await store.hydrateRuns();
  assert.deepEqual(store.runsByProject["o/r"], []);
  assert.equal(store.runsError, "Could not read the runs: the project is not in the list.");
});

test("an unreadable run is counted, and its id is never shown", async () => {
  const { store } = page(scripted([{ status: "ok", runs: [], unreadable: [{ runid: "01JX4ABCDEF", reason: "malformed" }] }]));
  await store.hydrateRuns();
  assert.equal(store.runsError, "Could not read 1 saved run. The file is damaged or from another version of Ralphy.");
});

// Two reads can be in flight; the daemon may answer the older one last.
test("a runs reply that arrives after a newer read is dropped", async () => {
  const answers = [];
  const { store } = page(() => new Promise((resolve) => answers.push(resolve)));
  const older = store.hydrateRuns();
  const newer = store.hydrateRuns();
  answers[1]({ status: "ok", runs: [{ runid: "new" }] });
  await newer;
  answers[0]({ status: "ok", runs: [{ runid: "old" }] });
  await older;
  assert.deepEqual(store.projectRuns().map((r) => r.runid), ["new"]);
  assert.equal(store.currentRun().runid, "new");
});

test("a kept runs read is told on the window, and a failed one is not", async () => {
  const { store, events } = page(scripted([{ status: "ok", runs: [{ runid: "a" }] }, { status: "error", reason: "x" }]));
  await store.hydrateRuns();
  assert.deepEqual(events, ["workbench:runs-read"]);
  await store.hydrateRuns();
  assert.deepEqual(events, ["workbench:runs-read"], "a failed read tells nothing");
});

test("the selected run stays while it is listed, and setCurrentRun is its writer", async () => {
  const runs = [{ runid: "a" }, { runid: "b" }];
  const { store } = page(async () => ({ status: "ok", runs }));
  await store.hydrateRuns();
  assert.equal(store.currentRun().runid, "a");
  store.setCurrentRun("b");
  await store.hydrateRuns();
  assert.equal(store.currentRun().runid, "b");
  // A run that left the list falls back to the first.
  runs.pop();
  await store.hydrateRuns();
  assert.equal(store.currentRunId, "a");
});

test("the runs follow the open project: the socket, the selection and a read", () => {
  const { store, projects, subs, window } = page(async () => ({ status: "ok", runs: [] }));
  store.init();
  let reads = 0;
  store.hydrateRuns = async () => reads++;
  store.runsByProject["o/b"] = [{ runid: "run-2" }];
  store.setCurrentRun("run-1");
  projects.setOpen("o/b");
  window.dispatchEvent(new CustomEvent("workbench:project-changed", { detail: { slug: "o/b", previous: "o/r" } }));
  assert.deepEqual(subs.map((s) => [s.repo, s.closed]), [["o/b", false]]);
  assert.equal(store.currentRunId, "run-2");
  assert.equal(reads, 1);
  // Closing the project closes its socket and reads nothing.
  projects.setOpen(null);
  window.dispatchEvent(new CustomEvent("workbench:project-changed", { detail: { slug: null, previous: "o/b" } }));
  assert.deepEqual(subs.map((s) => [s.repo, s.closed]), [["o/b", true]]);
  assert.equal(store._runsSub, null);
  assert.equal(store.currentRunId, null);
  assert.equal(reads, 1);
});

test("a push on the runs socket is told on the window as workbench:runs-dirty", () => {
  const { store, subs, events } = page();
  store.mountRunsSub();
  subs[0].onDirty();
  assert.deepEqual(events, ["workbench:runs-dirty"]);
});

// The Runs panel reads the plan; the store writes it on the run (one writer).
test("a failed plan read keeps the last good text and flags it; a good read clears the flag", () => {
  const { store } = page();
  const run = { runid: "r1", planMd: "", planReadFailed: false };
  store.setRunPlan(run, "## Steps");
  assert.deepEqual([run.planMd, run.planReadFailed], ["## Steps", false]);
  store.setRunPlan(run, null);
  assert.deepEqual([run.planMd, run.planReadFailed], ["## Steps", true], "the last good text stays");
  store.setRunPlan(run, "");
  assert.deepEqual([run.planMd, run.planReadFailed], ["", false], "an empty plan is a good read");
});
