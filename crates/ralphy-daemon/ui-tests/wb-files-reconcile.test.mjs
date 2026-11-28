// Unit tests for the reconcile of one tree level in assets/ui/wb-files.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// ---- the reconcile of one level (`_reconcileOnce`) -----------------------------
// A stand-in for the Wunderbaum tree, as far as a reconcile reads it. `disk`
// maps a rel path to the listing a read of that level answers; a folder
// opened with no children reads its level from it. A teardown clamps the
// scroll offset to 0, as the browser does for a shorter list.
function fakeTree(disk) {
  const tree = { element: { scrollTop: 0 }, active: null, filtering: false, log: [] };
  const relOf = (n) => (n.parent ? [relOf(n.parent), n.title].filter(Boolean).join("/") : "");
  const node = (title, parent, spec = {}) => ({
    title,
    parent,
    tree,
    data: { folder: Boolean(spec.folder) },
    lazy: Boolean(spec.folder),
    classes: spec.classes || "",
    expanded: false,
    children: null,
    hasClass(name) {
      return this.classes === name;
    },
    isLoading: () => false,
    visit(fn) {
      for (const c of this.children || []) {
        fn(c);
        c.visit(fn);
      }
    },
    removeChildren() {
      this.visit((c) => (c.tree = null));
      this.children = null;
      tree.element.scrollTop = 0;
    },
    load(source) {
      this.children = source.map((s) => node(s.title, this, s));
    },
    async setExpanded(flag) {
      if (flag && this.children == null) await this.load(disk[relOf(this)] || []);
      this.expanded = flag;
      tree.log.push(`expand ${relOf(this)}`);
    },
    setActive() {
      tree.active = this;
    },
  });
  tree.root = node("root", null, { folder: true });
  tree.root.expanded = true;
  tree.findFirst = (match) => {
    let hit;
    tree.root.visit((n) => (hit ||= match(n) ? n : undefined));
    return hit;
  };
  tree.getActiveNode = () => tree.active;
  tree.isFilterActive = () => tree.filtering;
  tree.updateFilter = () => tree.log.push("filter");
  tree.updatePendingModifications = () => tree.log.push("pending");
  tree.relOf = relOf;
  // Opens the folders `rels` lists, shallowest first, as the operator did.
  tree.open = async (...rels) => {
    await tree.root.load(disk[""]);
    for (const r of rels) await tree.findFirst((n) => relOf(n) === r).setExpanded(true);
  };
  return tree;
}

// The files over `tree`, with the reads stubbed: `fetchTreeLevel` answers
// from `disk`, and the cache calls are recorded in `calls`.
function reconcileFiles(tree, disk) {
  const s = loadComponent("wbFiles").scope;
  const calls = [];
  s.setTree(tree);
  s.fetchTreeLevel = (rel) => Promise.resolve(disk[rel]);
  s.revalidateBelow = (node, rel) => calls.push(["revalidateBelow", rel]);
  s.forgetValidatedBelow = (rel) => calls.push(["forgetValidatedBelow", rel]);
  return { s, calls };
}

const titles = (node) => (node.children || []).map((n) => n.title);

test("a reconcile on a node a reload unregistered works on the live node of its rel", async () => {
  const disk = { "": [{ title: "a", folder: true }, { title: "x.txt" }], a: [{ title: "b.txt" }] };
  const tree = fakeTree(disk);
  await tree.open();
  const { s } = reconcileFiles(tree, disk);
  const dead = {
    tree: null,
    removeChildren() {
      throw new Error("a teardown on a dead node throws inside Wunderbaum");
    },
  };
  disk[""] = [{ title: "a", folder: true }, { title: "y.txt" }];
  await s._reconcileOnce(dead, "");
  assert.deepEqual(titles(tree.root), ["a", "y.txt"]);
  // A level that no longer exists, and one still loading, are left alone.
  await s._reconcileOnce(dead, "gone");
  const a = tree.findFirst((n) => n.title === "a");
  a.isLoading = () => true;
  await s._reconcileOnce(dead, "a");
  assert.equal(a.children, null, "a teardown under an in-flight load doubles the children");
});

test("a level that already shows the listing is not torn down", async () => {
  const disk = { "": [{ title: "a", folder: true }, { title: "x.txt" }] };
  const tree = fakeTree(disk);
  await tree.open();
  const { s, calls } = reconcileFiles(tree, disk);
  const rows = tree.root.children;
  await s._reconcileOnce(tree.root, "");
  assert.equal(tree.root.children, rows);
  assert.deepEqual(calls, []);
  // An unchanged level with a .gitignore still re-reads the levels below it.
  disk[""] = [{ title: "a", folder: true }, { title: ".gitignore" }];
  await tree.open();
  const kept = tree.root.children;
  await s._reconcileOnce(tree.root, "");
  assert.equal(tree.root.children, kept);
  assert.deepEqual(calls, [["revalidateBelow", ""]]);
  // A changed level with a .gitignore is re-listed, and forgets what it validated below.
  calls.length = 0;
  disk[""] = [{ title: "a", folder: true }, { title: "b", folder: true }, { title: ".gitignore" }];
  await s._reconcileOnce(tree.root, "");
  assert.deepEqual(titles(tree.root), ["a", "b", ".gitignore"]);
  assert.deepEqual(calls, [["forgetValidatedBelow", ""]]);
});

test("the folders open before a re-list are open after it", async () => {
  const disk = {
    "": [{ title: "a", folder: true }, { title: "c", folder: true }],
    a: [{ title: "b", folder: true }],
    "a/b": [{ title: "f.txt" }],
    c: [],
  };
  const tree = fakeTree(disk);
  await tree.open("a", "a/b", "c");
  const { s } = reconcileFiles(tree, disk);
  disk[""] = [{ title: "a", folder: true }, { title: "c", folder: true }, { title: "new.txt" }];
  tree.log.length = 0;
  await s._reconcileOnce(tree.root, "");
  assert.deepEqual(tree.log.filter((l) => l.startsWith("expand")), ["expand a", "expand c", "expand a/b"], "shallowest first");
  const open = [];
  tree.root.visit((n) => n.expanded && open.push(tree.relOf(n)));
  assert.deepEqual(open, ["a", "a/b", "c"]);
});

test("a re-list keeps the scroll offset and the selection, unless a reveal landed meanwhile", async () => {
  const disk = { "": [{ title: "x.txt" }, { title: "y.txt" }] };
  const tree = fakeTree(disk);
  await tree.open();
  const { s } = reconcileFiles(tree, disk);
  tree.findFirst((n) => n.title === "x.txt").setActive();
  tree.element.scrollTop = 120;
  tree.filtering = true;
  disk[""] = [{ title: "x.txt" }, { title: "y.txt" }, { title: "z.txt" }];
  await s._reconcileOnce(tree.root, "");
  assert.equal(tree.element.scrollTop, 120);
  assert.equal(tree.relOf(tree.active), "x.txt");
  assert.notEqual(tree.active.tree, null, "the selection is the new row");
  assert.deepEqual(tree.log, ["filter", "pending"], "the search marks are painted and the list has its height first");
  // A reveal during the read owns the selection and the scroll offset.
  disk[""] = [{ title: "x.txt" }, { title: "y.txt" }];
  tree.element.scrollTop = 120;
  s.fetchTreeLevel = (rel) => {
    s._revealSeq += 1;
    s._revealedRel = "y.txt";
    return Promise.resolve(disk[rel]);
  };
  await s._reconcileOnce(tree.root, "");
  assert.equal(tree.relOf(tree.active), "y.txt");
  assert.equal(tree.element.scrollTop, 0);
});

// A reveal brings a row on screen and selects it.
test("a reveal opens the folders above the row, shallowest first, and selects it; an unmounted folder reveals nothing", async () => {
  const disk = { "": [{ title: "a", folder: true }], a: [{ title: "b", folder: true }], "a/b": [{ title: "f.txt" }] };
  const tree = fakeTree(disk);
  await tree.open();
  const { s } = reconcileFiles(tree, disk);
  const node = await s.revealRel("a/b/f.txt");
  assert.deepEqual(tree.log, ["expand a", "expand a/b"]);
  assert.equal(tree.relOf(node), "a/b/f.txt");
  assert.equal(tree.active, node);
  assert.equal(await s.revealRel("z/f.txt"), null);
  assert.equal(tree.active, node, "the selection stays where it was");
});

// A reconcile asked for while one of the same level runs.
test("a reconcile asked for during a pass waits for the level, which is read once more", async () => {
  const disk = { "": [{ title: "x.txt" }] };
  const tree = fakeTree(disk);
  await tree.open();
  const { s } = reconcileFiles(tree, disk);
  const passes = [];
  let finish;
  s._reconcileOnce = (node, rel) => {
    passes.push(rel);
    return passes.length === 1 ? new Promise((resolve, reject) => { finish = { resolve, reject }; }) : Promise.resolve();
  };
  const first = s.reconcileLevel(tree.root, "");
  let settled = false;
  const second = s.reconcileLevel(tree.root, "").then(() => (settled = true));
  await Promise.resolve();
  assert.equal(settled, false, "the second caller waits for the pass in flight");
  finish.resolve();
  await Promise.all([first, second]);
  assert.deepEqual(passes, ["", ""], "the level is read once more, and only once");

  // A pass that throws still settles every caller that waits on it.
  passes.length = 0;
  const failing = s.reconcileLevel(tree.root, "").catch((e) => e.message);
  const waiting = s.reconcileLevel(tree.root, "").then(() => "settled");
  finish.reject(new Error("socket dropped"));
  assert.deepEqual(await Promise.all([failing, waiting]), ["socket dropped", "settled"]);
  assert.deepEqual(passes, [""], "a pass that threw is not run again");
});
