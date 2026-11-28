// Unit tests for assets/ui/wb-files.ts, the Alpine component of the FILES tree
// of the open project, and for assets/ui/wb-file-paths.ts, the path rules it
// shares with app.ts and the move dialog. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";
import { classify, fileTabId, isProtectedDir, newEntryTitle, parentRel, tabIcon, underProtectedDir } from "../assets/ui/wb-file-paths.ts";
import { page } from "./wb-files-fixture.mjs";

// ---- the path rules (wb-file-paths.ts) ---------------------------------------

test("the path rules: the viewer of a name, the parent of a rel path, a tab id, a create title", () => {
  const rows = [
    ["idea.note", "note"],
    ["README.md", "markdown"],
    ["notes.MARKDOWN", "markdown"],
    ["logo.PNG", "image"],
    ["archive.zip", "archive"],
    ["backup.tar.gz", "archive"],
    ["main.rs", "code"],
    ["Makefile", "code"],
    ["erd.mmd", "mermaid"],
    ["FLOW.MMD", "mermaid"],
    ["schema.dbml", "dbml"],
    ["report.html", "html"],
    ["OLD.HTM", "html"],
  ];
  for (const [name, want] of rows) assert.equal(classify(name), want, name);
  assert.deepEqual([parentRel("a/b/c.txt"), parentRel("top.txt")], ["a/b", ""]);
  // The primary's id has no checkout; a worktree's names it.
  assert.equal(fileTabId("o/r", "a.md", null), "file:o/r:a.md");
  assert.equal(fileTabId("o/r", "a.md", "wt-a"), "file:o/r@wt-a:a.md");
  assert.equal(newEntryTitle("file", ""), "New file in the project root");
  assert.equal(newEntryTitle("folder", "src"), "New folder in src");
});

test("the tab icon of each file kind", () => {
  // [file kind, icon of its canvas tab]
  const rows = [
    ["markdown", "bi bi-file-earmark-text"],
    ["image", "bi bi-file-earmark-image"],
    ["mermaid", "bi bi-diagram-3"],
    ["dbml", "bi bi-table"],
    ["html", "bi bi-filetype-html"],
    ["code", "bi bi-file-earmark-code"],
  ];
  for (const [kind, want] of rows) assert.equal(tabIcon(kind), want, kind);
});

test("the tree icon of a diagram file", () => {
  const s = loadComponent("wbFiles").scope;
  assert.equal(s.fileIcon("flow.mmd"), "bi bi-diagram-3");
  assert.equal(s.fileIcon("schema.dbml"), "bi bi-table");
  // Control: a file with no icon of its own gets the plain one.
  assert.equal(s.fileIcon("flow.txt"), "bi bi-file-earmark");
});

test("the protected directories: .git and .ralphy in any case, and the note in the notes directory", () => {
  assert.deepEqual([".git", ".GIT", ".ralphy", "src"].map((d) => isProtectedDir(d)), [true, true, true, false]);
  const rows = [
    [".ralphy/notes/idea.note", false],
    ["src/.git/config", true],
    [".ralphy/notes/sub/idea.note", true],
    [".ralphy/./notes/idea.note", false],
    ["src/app.js", false],
  ];
  for (const [rel, want] of rows) assert.equal(underProtectedDir(rel), want, rel);
});

// ---- the component and its markup ---------------------------------------------

test("wbFiles lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbFiles");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the files, and a create re-lists before it reveals", async () => {
  const { files, fire, shell } = page();
  const trees = [];
  files.destroyTree = () => trees.push("destroy");
  files.mountTree = () => {
    trees.push("mount");
    files._treeCheckout = shell.$store.checkouts.checkoutOf(files.$store.projects.openSlug);
  };
  const writes = [];
  const realDaemon = globalThis.WBDaemon;
  globalThis.WBDaemon = {
    ...realDaemon,
    write: (verb, body) => (writes.push([verb, body]), Promise.resolve({})),
    withCheckout: (body) => body,
  };
  let replays = 0;
  files._treeSub = { replay: () => replays++ };
  const relists = [];
  let settle;
  files.onTreeDirty = (rel) => {
    relists.push(`dirty ${rel}`);
    return new Promise((r) => { settle = r; });
  };
  files.revealRel = (rel) => relists.push(`reveal ${rel}`);
  const peer = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
  try {
    files.$store.projects.setOpen(`${peer}/o/r`);
    fire("workbench:project-changed", { slug: `${peer}/o/r`, previous: null });
    assert.deepEqual(trees, ["destroy", "mount"], "the open project's tree is mounted");
    files._filesPeerDown = `${peer}/o/r`;
    files.openPeerDown = () => false;
    fire("workbench:fleet-read");
    assert.equal(files._filesPeerDown, null, "the fleet read says the peer is back");
    fire("workbench:peer-woken", { daemon: peer });
    assert.equal(trees.length, 4, "the woken peer's tree is mounted again");
    shell.$store.checkouts.checkouts = { [`${peer}/o/r`]: "wt-a" };
    fire("workbench:checkout-changed");
    assert.equal(trees.length, 6, "the tree of the new checkout is mounted");
    assert.equal(files._treeCheckout, "wt-a");
    fire("workbench:panels-reread");
    assert.equal(replays, 1, "the socket sends what the tree missed");
    fire("workbench:file-search-open");
    assert.equal(files.fileSearch.open, true);
    fire("workbench:move-confirmed", { from: "a.txt", to: "dst/a.txt" });
    await new Promise((r) => { setImmediate(r); });
    assert.deepEqual(writes, [["file.rename", { repo: `${peer}/o/r`, path: "a.txt", to: "dst/a.txt" }]]);
    settle?.();
    await new Promise((r) => { setImmediate(r); });
    relists.length = 0;
    fire("workbench:tree-dirty", { rel: "src", reveal: "src/new.txt" });
    assert.deepEqual(relists, ["dirty src"], "the reveal waits for the level");
    settle();
    await new Promise((r) => { setImmediate(r); });
    assert.deepEqual(relists, ["dirty src", "reveal src/new.txt"]);
  } finally {
    globalThis.WBDaemon = realDaemon;
  }
});

test("a tree is remounted when its checkout or its woken peer is the open project's, and only then", () => {
  const { shell, files } = page();
  const calls = [];
  files.destroyTree = () => calls.push("destroy");
  files.mountTree = () => calls.push("mount");
  const peer = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
  files.$store.projects.setOpen(`${peer}/o/r`);
  files._treeCheckout = null;
  files.filesFollowCheckout();
  assert.deepEqual(calls, [], "the tree is the primary's, and so is the selection");
  shell.$store.checkouts.checkouts = { [`${peer}/o/r`]: "wt-a" };
  files.filesFollowCheckout();
  assert.deepEqual(calls, ["destroy", "mount"]);
  files.filesFollowWake("01OTHERPEER000000000000000");
  assert.equal(calls.length, 2, "another peer woke");
  files.filesFollowWake(peer);
  assert.deepEqual(calls, ["destroy", "mount", "destroy", "mount"]);
});

test("a head.dirty push sends workbench:head-moved once for the open project", async () => {
  const { files, events } = page();
  files.HEAD_SETTLE_MS = 0;
  files.$store.projects.setOpen("o/r");
  files.onHeadMoved();
  files.onHeadMoved();
  await new Promise((r) => { setTimeout(r, 5); });
  assert.deepEqual(
    events.filter((e) => e.type === "workbench:head-moved").map((e) => e.detail),
    [{ ref: "o/r" }],
  );
});

test("the Move item asks the move dialog with the full rel path", () => {
  const { shell, files, events } = page();
  let items = null;
  shell.$store.tabs.renderMenu = (_x, _y, list) => (items = list);
  const root = { title: "root", parent: null };
  const src = { title: "src", parent: root, data: { folder: true }, children: [] };
  const file = { title: "a.txt", parent: src, data: {}, children: null };
  files.showMenu(0, 0, file);
  items.find((i) => i.label === "Move to…").run();
  assert.deepEqual(
    events.filter((e) => e.type === "workbench:move-open").map((e) => e.detail),
    [{ from: "src/a.txt" }],
  );
});

test("Delete on a selected row covers every selected row; on another row, that row alone", () => {
  const { shell, files, window } = page();
  let items = null;
  shell.$store.tabs.renderMenu = (_x, _y, list) => (items = list);
  files.$store.projects.setOpen("o/r");
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push([action, detail]) };
  const root = { title: "root", parent: null };
  const node = (title, selected, folder = false) => ({
    title, parent: root, data: { folder }, children: folder ? [] : null, isSelected: () => selected,
  });
  const a = node("a.txt", true);
  const src = node("src", true, true);
  const other = node("c.txt", false);
  const tree = { getSelectedNodes: (stop) => (assert.equal(stop, true), [a, src]) };
  for (const n of [a, src, other]) n.tree = tree;
  const deleteOn = (n) => {
    files.showMenu(0, 0, n);
    const row = items.find((i) => i.label?.startsWith("Delete"));
    row.run();
    return [row.label, emitted.at(-1)[1].entries.map((e) => [e.path, e.isFolder])];
  };
  assert.deepEqual(deleteOn(a), ["Delete 2 items", [["a.txt", false], ["src", true]]]);
  assert.deepEqual(deleteOn(other), ["Delete", [["c.txt", false]]]);
  assert.equal(emitted.at(-1)[1].project, "o/r");
});

test("the files part mounts the tree of the open project, and only unmounts when none is open", () => {
  const { scope: state } = loadComponent("wbFiles", { magics: { $nextTick: (fn) => fn() } });
  const calls = [];
  state.destroyTree = () => calls.push("destroyTree");
  state.mountTree = () => calls.push("mountTree");
  const named = (n) => calls.filter((c) => c === n);
  state.$store.projects.setOpen("owner/b");
  state.filesFollowProject();
  assert.deepEqual([named("destroyTree").length, named("mountTree").length], [1, 1]);
  state.$store.projects.setOpen(null);
  state.filesFollowProject();
  assert.deepEqual([named("destroyTree").length, named("mountTree").length], [2, 1]);
});

// ---- folds the tree relies on --------------------------------------------------

test("a fresh listing evicts the levels it contradicts — a reused folder name is not the old folder", () => {
  const own = loadComponent("wbFiles").scope;
  own.$store.projects.setOpen("me/vc-stress");
  own.treeMem();
  const seed = (rel, entries) => own._treeCache.set(own.treeKey(rel), entries);
  // The key is `<slug>\n<checkout>\n<rel>` (#406 added the middle segment);
  // the rel is its LAST segment.
  const cached = () =>
    [...own._treeCache.keys()].map((k) => k.split("\n").at(-1)).toSorted();

  // The tree as it stood: `ideias/` holding `dossie/`, itself holding a file.
  seed("", [{ name: "ideias", dir: true }, { name: "README.md", dir: false }]);
  seed("ideias", [{ name: "dossie", dir: true }]);
  seed("ideias/dossie", [{ name: "nota.md", dir: false }]);
  // A sibling that shares the PREFIX but not the path: `ideias_vbforge` must
  // survive a prune of `ideias`, or the fix trades one ghost for a blank folder.
  seed("ideias_vbforge", [{ name: "dossie", dir: true }]);
  own._treeValidated.add(own.treeKey("ideias"));

  // The rename lands: the root no longer lists `ideias`, so everything
  // remembered UNDER it is a statement about a directory that is gone.
  own.pruneTreeCache("", [
    { name: "ideias_vbforge", dir: true },
    { name: "README.md", dir: false },
  ]);
  assert.deepEqual(cached(), ["", "ideias_vbforge"]);
  // Evicted from BOTH: a key left in `_treeValidated` is the half that made the
  // ghost immortal — the level would be painted from a later cache entry and
  // never re-read.
  assert.equal(own._treeValidated.has(own.treeKey("ideias")), false);

  // A name that came back as a FILE is contradicted just as hard as one that
  // vanished — a file has no children to remember.
  seed("ideias", [{ name: "dossie", dir: true }]);
  own.pruneTreeCache("", [{ name: "ideias", dir: false }]);
  assert.equal(own._treeCache.has(own.treeKey("ideias")), false);

  // NEGATIVE CONTROL: a listing that still names its subdirectory evicts
  // nothing, including the level being replaced (its own key is the caller's to
  // overwrite, not this fold's to drop).
  seed("ideias", [{ name: "dossie", dir: true }]);
  seed("ideias/dossie", [{ name: "nota.md", dir: false }]);
  own.pruneTreeCache("ideias", [{ name: "dossie", dir: true }]);
  assert.ok(own._treeCache.has(own.treeKey("ideias")));
  assert.ok(own._treeCache.has(own.treeKey("ideias/dossie")));

  // Scoped by REPO, like every other key read: another project's `ideias/` is
  // not this listing's business.
  own._treeCache.set("other/repo\nideias", [{ name: "dossie", dir: true }]);
  own.pruneTreeCache("", [{ name: "README.md", dir: false }]);
  assert.ok(own._treeCache.has("other/repo\nideias"));
});

test("isFolder reads Wunderbaum's data bag, a lazy flag or a child list", () => {
  // Wunderbaum copies a source key it does not define into `node.data`, so the
  // listing's `folder: true` arrives there, and a lazy folder has no children
  // until it is read.
  const s = loadComponent("wbFiles").scope;
  const rows = [
    ["a collapsed folder from the listing", { data: { folder: true }, children: null }, true],
    ["a lazy folder not read yet", { data: {}, lazy: true, children: null }, true],
    ["an expanded folder", { data: {}, children: [] }, true],
    ["a file", { data: {}, children: null }, false],
    ["no node (empty tree space)", null, false],
  ];
  for (const [why, node, want] of rows) assert.equal(s.isFolder(node), want, why);
});

const treeNode = (rel) =>
  rel.split("/").reduce((parent, title) => ({ title, parent, data: {}, children: null }), {
    title: "root",
  });

test("the Move item is withheld inside .git and .ralphy, except for a note in the notes directory", () => {
  // The context menu is where the UI's mirror of the daemon's carve-out
  // (`fswrite::is_note_in_notes_dir`) decides what to offer: exactly
  // `.ralphy/notes/<name>.note`, nothing deeper or differently named.
  const { scope: own, shell } = loadComponent("wbFiles");
  let items = null;
  shell.$store.tabs.renderMenu = (_x, _y, list) => {
    items = list;
  };
  const offersMove = (rel) => {
    own.showMenu(0, 0, treeNode(rel));
    return items.some((i) => i.label?.startsWith("Move"));
  };
  const rows = [
    [".ralphy/notes/idea.note", true],
    ["src/app.js", true],
    [".ralphy/notes/sub/idea.note", false],
    [".ralphy/notes/.note", false],
    [".ralphy/notes/idea.md", false],
    [".ralphy/drafts/idea.note", false],
    [".git/notes/idea.note", false],
  ];
  for (const [rel, want] of rows) assert.equal(offersMove(rel), want, rel);
});

test("emitCreate sends the directory the create lands in, and only what the listener reads", () => {
  const { scope: state, window } = loadComponent("wbFiles");
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push([action, detail]) };
  state.$store.projects.setOpen("owner/repo");
  const root = { title: "root", parent: null };
  const src = { title: "src", parent: root, data: { folder: true } };
  const file = { title: "main.rs", parent: src, data: {} };
  state.emitCreate(file, "file");
  state.emitCreate(src, "folder");
  state.emitCreate(null, "file");
  assert.deepEqual(emitted, [
    ["create", { project: "owner/repo", path: "src", kind: "file" }],
    ["create", { project: "owner/repo", path: "src", kind: "folder" }],
    ["create", { project: "owner/repo", path: "", kind: "file" }],
  ]);
});
