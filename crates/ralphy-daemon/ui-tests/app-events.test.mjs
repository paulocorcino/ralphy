// Tests of assets/ui/app.ts — the `shell()` component: the events it reads
// again on, and the events it trades with the files and the seam.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

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
  await new Promise((r) => { setImmediate(r); });
  assert.deepEqual(calls, ["afterLogin", "restoreView", "adoptDeskCheckouts"]);
});

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
  window.WBDaemon = { write: () => Promise.resolve({ status: "ok" }), withCheckout: (p) => p };
  const realDaemon = globalThis.WBDaemon;
  globalThis.WBDaemon = window.WBDaemon;
  window.getShell = () => ({});
  window.Alpine.store("tabs").openTab = () => {};
  window.Alpine.store("modals").askPrompt = () => Promise.resolve("x.txt");
  window.Alpine.store("modals").flash = () => {};
  try {
    for (const fn of listeners) await fn({ detail: { action: "create", project: "o/r", path: "src", kind: "file" } });
    await new Promise((r) => { setImmediate(r); });
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
  state.$store.modals.askConfirm = (o) => {
    asked.push(o.message);
    return Promise.resolve(false);
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
    write: (verb, payload) => Promise.resolve((written.push({ verb, payload }), reply)),
    withCheckout: (p) => p,
  };
  const flashed = [];
  window.Alpine.store("modals").flash = (m) => flashed.push(m);
  const answer = { yes: true };
  window.Alpine.store("modals").askConfirm = () => Promise.resolve(answer.yes);
  const act = async (detail) => {
    const realDaemon = globalThis.WBDaemon;
    globalThis.WBDaemon = window.WBDaemon;
    try {
      for (const fn of listeners) await fn({ detail });
      await new Promise((r) => { setImmediate(r); });
    } finally {
      globalThis.WBDaemon = realDaemon;
    }
  };
  return { window, sent, written, flashed, answer, act };
}

test("a delete that the daemon answers 'not found' flashes the refusal and re-lists the parent", async () => {
  const { sent, written, flashed, act } = writeSeam({ status: "error", message: "not found" });
  await act({ action: "delete", project: "o/r", entries: [{ path: "src/a/gone.txt", title: "gone.txt" }] });
  assert.deepEqual(written.map((w) => [w.verb, w.payload.path]), [["file.delete", "src/a/gone.txt"]]);
  assert.equal(flashed.length, 1);
  assert.match(flashed[0], /Could not delete/u);
  assert.deepEqual(sent("workbench:tree-dirty"), [{ rel: "src/a" }]);
});

test("a delete that works says so and re-lists nothing; one that is refused for another reason re-lists nothing", async () => {
  let seam = writeSeam({ status: "ok" });
  await seam.act({ action: "delete", project: "o/r", entries: [{ path: "src/a.txt", title: "a.txt" }] });
  assert.deepEqual(seam.flashed, ["a.txt deleted."]);
  assert.deepEqual(seam.sent("workbench:tree-dirty"), []);

  seam = writeSeam({ status: "error", message: "permission denied" });
  await seam.act({ action: "delete", project: "o/r", entries: [{ path: "src/a.txt" }] });
  assert.match(seam.flashed[0], /Could not delete/u);
  assert.deepEqual(seam.sent("workbench:tree-dirty"), []);
});

test("a delete the operator declines writes nothing", async () => {
  const { answer, written, flashed, act } = writeSeam({ status: "ok" });
  answer.yes = false;
  await act({ action: "delete", project: "o/r", entries: [{ path: "src/a.txt" }] });
  assert.deepEqual(written, []);
  assert.deepEqual(flashed, []);
});

test("a delete of several entries asks once, deletes each, and flashes one line", async () => {
  let asked = 0;
  let seam = writeSeam({ status: "ok" });
  seam.window.Alpine.store("modals").askConfirm = (opts) => (asked++, Promise.resolve(/3 items/u.test(opts.message)));
  const entries = [{ path: "a.txt", title: "a.txt" }, { path: "src", title: "src", isFolder: true }, { path: "b.md", title: "b.md" }];
  await seam.act({ action: "delete", project: "o/r", entries });
  assert.equal(asked, 1);
  assert.deepEqual(seam.written.map((w) => [w.verb, w.payload.path]), [["file.delete", "a.txt"], ["file.delete", "src"], ["file.delete", "b.md"]]);
  assert.deepEqual(seam.flashed, ["3 items deleted."]);

  seam = writeSeam({ status: "error", message: "permission denied" });
  await seam.act({ action: "delete", project: "o/r", entries });
  assert.equal(seam.written.length, 3, "a refusal does not stop the others");
  assert.equal(seam.flashed.length, 1);
  assert.match(seam.flashed[0], /^0 of 3 items deleted\. a\.txt: Could not delete/u);
});

test("a rename writes the full paths and flashes only a refusal", async () => {
  let seam = writeSeam({ status: "ok" });
  await seam.act({ action: "rename", project: "o/r", from: "src/a.txt", to: "src/b.txt" });
  assert.deepEqual(seam.written.map((w) => [w.verb, w.payload.path, w.payload.to]), [["file.rename", "src/a.txt", "src/b.txt"]]);
  await new Promise((r) => { setImmediate(r); });
  assert.deepEqual(seam.flashed, []);

  seam = writeSeam({ status: "error", message: "already exists" });
  await seam.act({ action: "rename", project: "o/r", from: "src/a.txt", to: "src/b.txt" });
  await new Promise((r) => { setImmediate(r); });
  assert.equal(seam.flashed.length, 1);
  assert.match(seam.flashed[0], /Could not rename/u);
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

test("a fleet group's header buttons name the wake and the update that run", () => {
  const { state } = loadShell();
  const g = { daemon: "01KY", state: "asleep" };
  assert.equal(state.wakeLabel(g), "asleep");
  assert.equal(state.updateLabel(g), "Update Ralphy");
  assert.deepEqual(state.updateClass(g), ["asleep", "fault", ""]);
  state.$store.projects.waking["01KY"] = true;
  state.$store.projects.updating["01KY"] = true;
  assert.equal(state.wakeLabel(g), "waking");
  assert.equal(state.updateLabel(g), "Updating Ralphy");
  assert.deepEqual(state.wakeClass(g).slice(2), ["waking"]);
  assert.deepEqual(state.updateClass(g), ["asleep", "fault", "waking"]);
});

test("a middle click closes a tab only when the tab can be closed", () => {
  const { state } = loadShell();
  const closed = [];
  state.$store.tabs.closeTab = (id) => closed.push(id);
  state.closeTabOnMiddleClick({ id: "a", closable: true }, { button: 0 });
  state.closeTabOnMiddleClick({ id: "b", closable: false }, { button: 1 });
  assert.deepEqual(closed, []);
  state.closeTabOnMiddleClick({ id: "c", closable: true }, { button: 1 });
  assert.deepEqual(closed, ["c"]);
});

test("a project row names its remote and tabs to its chip only when it is open", () => {
  const { state } = loadShell();
  const a = { slug: "owner/a", remote: "github", branch: "main" };
  assert.equal(state.remoteIcon(a), "bi bi-github");
  assert.equal(state.remoteTitle(a), "On GitHub");
  assert.equal(state.remoteTitle({ slug: "x", remote: "local" }), "Local only");
  assert.equal(state.chipTabindex(a), -1);
  state.$store.projects.setProjects([a]);
  state.$store.projects.setOpen(state.$store.projects.repoRef(a));
  assert.equal(state.chipTabindex(a), 0);
});
