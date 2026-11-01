// Unit tests for assets/ui/wb-branch-dialog.ts, the Alpine component of the
// Switch branch modal and the checkout menu. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { assembledPage, bindingNames, loadComponent, loadShell, partMarkup, withoutComments } from "./harness.mjs";

// The part that holds the branch dialog's markup.
const PART = "wb-branch-dialog.part.html";

// ---- the component and its markup ---------------------------------------------

test("wbBranchDialog lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbBranchDialog");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbBranchDialog reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbBranchDialog");
  const markup = partMarkup(PART);
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variable of the branch rows.
  const loopVars = new Set(["b"]);
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component; `x-ref` names an element.
    if (attr === "x-data" || attr === "x-ref") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 15, `the markup was read: ${read} names`);
});

test("nothing outside the branch dialog element names its state", () => {
  const outside = withoutComments(assembledPage("index.html", { skip: PART }));
  for (const name of ["branchModal", "openBranchModal", "closeBranchModal", "branchList", "canCreateBranch", "branchEnter", "switchBranch", "createBranch", "removeWorktree", "worktreeRemoving", "openCheckoutChip", "_mutateBranch", "branchHeldBy"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

// One page with the shell, the files, the Changes view and the branch dialog,
// and a window that delivers the events it is sent, as the browser does.
function page() {
  const listeners = {};
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const dialog = loadComponent("wbBranchDialog", { from: loaded }).scope;
  const files = loadComponent("wbFiles", { from: loaded }).scope;
  const changes = loadComponent("wbChanges", { from: loaded }).scope;
  dialog.init();
  return { shell: loaded.state, dialog, files, changes };
}

test("the branch chip and the two checkout chips reach the branch dialog through events", () => {
  const { shell, dialog, files, changes } = page();
  const calls = [];
  dialog.openBranchModal = (p) => calls.push(["branch", p.slug]);
  dialog.openCheckoutChip = (p, anchor) => calls.push(["menu", p.slug, anchor]);
  const p = { slug: "o/r" };
  let stopped = 0;
  const click = { stopPropagation: () => stopped++ };
  // CONTROL: a collapsed row's chip is only the change count, and its click
  // falls through to the row.
  shell.branchChipClick(p, click);
  assert.deepEqual(calls, []);
  shell.$store.projects.setOpen("o/r");
  shell.branchChipClick(p, click);
  assert.equal(stopped, 1);
  files.checkoutChipClick(p, "files-anchor");
  changes.checkoutChipClick(p, "changes-anchor");
  assert.deepEqual(calls, [
    ["branch", "o/r"],
    ["menu", "o/r", "files-anchor"],
    ["menu", "o/r", "changes-anchor"],
  ]);
});

// ---- a branch another worktree holds (#622) ------------------------------------

// A picker open on `o/r` whose `branch.list` answers with `worktrees`; every
// verb the dialog sends is recorded in `verbs`.
async function heldPicker(replies = {}) {
  const { dialog } = page();
  const verbs = [];
  globalThis.WBDaemon = globalThis.window.WBDaemon; // `_mutateBranch` reads the bare global
  globalThis.window.WBDaemon.observe = async (verb, args) => {
    verbs.push([verb, args.name ?? null]);
    return replies[verb] ?? null;
  };
  dialog.branchModal = { slug: "o/r", filter: "", branches: [], current: "main", primaryBranch: "main", dirty: false, checkoutDirty: false, worktrees: {} };
  replies["branch.list"] = {
    status: "ok",
    branches: { current: "main", branches: ["main", "feat-a", "free"], worktrees: { main: "/repo", "feat-a": "/repo/wt-a" } },
  };
  await dialog.loadBranches("o/r");
  verbs.length = 0;
  return { dialog, verbs };
}

test("a branch held by another worktree is named by its path, never the current one", async () => {
  const { dialog } = await heldPicker();
  assert.equal(dialog.branchHeldBy("feat-a"), "/repo/wt-a");
  assert.equal(dialog.branchHeldBy("free"), "");
  // CONTROL: `main` is in the map, but this tree has it.
  assert.equal(dialog.branchHeldBy("main"), "");
});

test("a held branch is still listed, and switching to it sends nothing", async () => {
  const { dialog, verbs } = await heldPicker();
  assert.deepEqual(dialog.branchList(), ["main", "feat-a", "free"]);
  dialog.branchModal.filter = "feat";
  assert.deepEqual(dialog.branchList(), ["feat-a"]);
  dialog.switchBranch("feat-a");
  dialog.branchEnter();
  assert.deepEqual(verbs, []);
  // CONTROL: a free branch is sent.
  dialog.switchBranch("free");
  assert.deepEqual(verbs, [["branch.switch", "free"]]);
});

test("a refused switch shows git's words after the act", async () => {
  const { dialog } = await heldPicker({
    "branch.switch": {
      status: "error",
      message: "Error: cannot switch branch: error: Your local changes to the following files would be overwritten by checkout",
    },
  });
  dialog.switchBranch("free");
  await new Promise((r) => setTimeout(r, 0));
  const note = dialog.$store.checkouts.branchError;
  assert.match(note, /^Could not switch branch: error: Your local changes/);
});

// ---- deleting a worktree --------------------------------------------------------

// A dialog on `o/r` with worktrees `wt-a` and `wt-b`. `worktree.remove` waits
// until the test calls `settle(name, reply)`; `worktree.list` answers `listed`.
function deleter() {
  const { shell, dialog } = page();
  const store = dialog.$store.checkouts;
  const pending = {};
  const sent = [];
  const notices = [];
  const flashed = [];
  let listed = ["wt-a", "wt-b"];
  const listing = () => ({ primary: "/repo", worktrees: listed.map((name) => ({ name, branch: name })) });
  globalThis.window.WBDaemon.observe = async (verb, args) => {
    sent.push([verb, args.name ?? null]);
    if (verb === "worktree.list") return { status: "ok", checkouts: listing() };
    return new Promise((resolve) => (pending[args.name] = resolve));
  };
  globalThis.window.WBConsole.askNotice = (n) => notices.push(n.message);
  shell.$store.modals.askConfirm = async () => true;
  shell.$store.modals.flash = (msg) => flashed.push(msg);
  const shown = () => (store.worktreeListings["o/r"]?.worktrees || []).map((w) => w.name);
  const tick = () => new Promise((r) => setTimeout(r, 0));
  return {
    dialog,
    store,
    sent,
    notices,
    flashed,
    shown,
    tick,
    setListed: (names) => (listed = names),
    settle: async (name, reply) => (pending[name](reply), await tick(), await tick()),
  };
}

test("a deleted worktree leaves the list before the daemon answers, and another one can be deleted meanwhile", async () => {
  const d = deleter();
  await d.store.ensureWorktreeListing("o/r", true);
  assert.deepEqual(d.shown(), ["wt-a", "wt-b"]);
  const a = d.dialog.removeWorktree("o/r", { name: "wt-a" });
  await d.tick();
  assert.deepEqual(d.shown(), ["wt-b"]);
  const b = d.dialog.removeWorktree("o/r", { name: "wt-b" });
  await d.tick();
  assert.deepEqual(d.shown(), []);
  // CONTROL: the same worktree twice sends one delete.
  d.dialog.removeWorktree("o/r", { name: "wt-a" });
  await d.tick();
  assert.deepEqual(d.sent.filter(([v]) => v === "worktree.remove"), [["worktree.remove", "wt-a"], ["worktree.remove", "wt-b"]]);
  // wt-a is done first; the re-read must not bring back wt-b, still in flight.
  d.setListed(["wt-b"]);
  await d.settle("wt-a", { status: "ok" });
  assert.deepEqual(d.shown(), []);
  d.setListed([]);
  await d.settle("wt-b", { status: "ok" });
  await Promise.all([a, b]);
  assert.deepEqual(d.flashed, ["Worktree wt-a deleted", "Worktree wt-b deleted"]);
  assert.deepEqual(d.notices, []);
});

test("a refused delete brings the row back with a notice; a kept branch is a flash", async () => {
  const d = deleter();
  await d.store.ensureWorktreeListing("o/r", true);
  const a = d.dialog.removeWorktree("o/r", { name: "wt-a" });
  await d.tick();
  await d.settle("wt-a", { status: "error", message: "worktree 'wt-a' has uncommitted changes: commit or discard them first" });
  await a;
  assert.deepEqual(d.shown(), ["wt-a", "wt-b"]);
  assert.deepEqual(d.dialog.worktreeRemoving, {}, "a settled delete leaves no entry for its repo");
  assert.equal(d.notices.length, 1);
  assert.match(d.notices[0], /has uncommitted changes/);
  d.setListed(["wt-a"]);
  const b = d.dialog.removeWorktree("o/r", { name: "wt-b" });
  await d.tick();
  await d.settle("wt-b", { status: "error", message: "removed worktree 'wt-b'; branch 'wt-b' kept: it has commits not on main" });
  await b;
  assert.equal(d.notices.length, 1, "a worktree that is gone opens no notice");
  assert.equal(d.flashed.length, 1);
  assert.match(d.flashed[0], /branch 'wt-b' kept: it has commits not on main/);
});
