// Unit tests for the row items, keys and nudges of assets/ui/wb-files.ts.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, partMarkup } from "./harness.mjs";
import { fullPath } from "../assets/ui/wb-file-paths.ts";
import { page, peerFilesShell, mountedTreeOptions } from "./wb-files-fixture.mjs";

test("a full path takes the separator of the root it is joined onto", () => {
  const rows = [
    ["C:\\Dev\\r", "src/a.txt", "C:\\Dev\\r\\src\\a.txt"],
    // A drive root keeps one separator, not two.
    ["C:\\", "a.txt", "C:\\a.txt"],
    ["\\\\host\\share\\r", "src/a.txt", "\\\\host\\share\\r\\src\\a.txt"],
    ["/home/me/r", "src/a.txt", "/home/me/r/src/a.txt"],
    ["/home/me/r/", "a.txt", "/home/me/r/a.txt"],
    // The root itself: no separator is added after it.
    ["C:\\Dev\\r", "", "C:\\Dev\\r"],
    ["/home/me/r", "", "/home/me/r"],
  ];
  for (const [root, rel, want] of rows) assert.equal(fullPath(root, rel), want, `${root} + ${rel}`);
});

test("the Properties item addresses the row, and empty space addresses the project root", () => {
  const { shell, files, events } = page();
  let items = null;
  shell.$store.tabs.renderMenu = (_x, _y, list) => (items = list);
  const root = { title: "root", parent: null };
  const src = { title: "src", parent: root, data: { folder: true }, children: [] };
  const file = { title: "a.txt", parent: src, data: {}, children: null };
  const asked = (node) => {
    files.showMenu(0, 0, node);
    items.find((i) => i.label === "Properties").run();
    return events.filter((e) => e.type === "workbench:properties-open").at(-1).detail;
  };
  assert.deepEqual(asked(file), { path: "src/a.txt", isFolder: false });
  assert.deepEqual(asked(src), { path: "src", isFolder: true });
  // Empty space has no node: the path is empty, which the dialog reads as the root.
  assert.deepEqual(asked(null), { path: "", isFolder: false });
});

test("the Properties item is offered where Move is withheld", () => {
  const { shell, files } = page();
  let items = null;
  shell.$store.tabs.renderMenu = (_x, _y, list) => (items = list);
  const inGit = { title: "config", parent: { title: ".git", parent: { title: "root", parent: null } }, data: {}, children: null };
  files.showMenu(0, 0, inGit);
  const labels = items.map((i) => i.label);
  assert.equal(labels.includes("Properties"), true);
  assert.equal(labels.some((l) => l?.startsWith("Move")), false, "the control: Move is withheld here");
});

test("Alt+Enter on a row asks for its properties and keeps the tree from opening it", () => {
  const { state, window, document } = peerFilesShell("reachable");
  const sent = [];
  window.dispatchEvent = (e) => (sent.push(e), true);
  const options = mountedTreeOptions(state, window, document);
  state.relPath = (n) => n.rel;
  const file = { rel: "src/a.txt", data: {}, children: null };
  const folder = { rel: "src", data: { folder: true }, children: [] };
  const key = (node, init) => options.keydown({ node, event: { key: "Enter", altKey: false, ...init } });
  const asked = () => sent.filter((e) => e.type === "workbench:properties-open").map((e) => e.detail);

  assert.equal(key(file, { altKey: true }), false, "the tree does not open the file");
  assert.equal(key(folder, { altKey: true }), false);
  assert.deepEqual(asked(), [
    { path: "src/a.txt", isFolder: false },
    { path: "src", isFolder: true },
  ]);
});

// A selected row of a tree, as the selection rules read it.
function selectedRow() {
  const n = { selected: true };
  n.setSelected = (flag) => (n.selected = flag);
  return n;
}

test("a plain click on a row and Escape drop the selection; Ctrl+click and other keys keep it", () => {
  const { state, window, document } = peerFilesShell("reachable");
  const options = mountedTreeOptions(state, window, document);
  let rows = [];
  const tree = { getSelectedNodes: () => rows.filter((n) => n.selected) };
  const fresh = () => (rows = [selectedRow(), selectedRow()]);
  const kept = () => tree.getSelectedNodes().length;

  fresh();
  options.click({ tree, node: rows[0], event: { ctrlKey: true } });
  assert.equal(kept(), 2, "Ctrl+click is the tree's own toggle");
  options.click({ tree, node: rows[0], event: { ctrlKey: false } });
  assert.equal(kept(), 0);

  fresh();
  assert.equal(options.keydown({ tree, node: rows[0], event: { key: "ArrowDown" } }), undefined);
  assert.equal(kept(), 2);
  assert.equal(options.keydown({ tree, node: rows[0], event: { key: "Escape" } }), false);
  assert.equal(kept(), 0);
  assert.equal(options.keydown({ tree, node: rows[0], event: { key: "Escape" } }), undefined, "nothing selected: the tree keeps Escape");
});

test("every other key, and Alt+Enter with no row, is left to the tree", () => {
  const { state, window, document } = peerFilesShell("reachable");
  const sent = [];
  window.dispatchEvent = (e) => (sent.push(e), true);
  const options = mountedTreeOptions(state, window, document);
  state.relPath = (n) => n.rel;
  const file = { rel: "a.txt", data: {}, children: null };

  assert.equal(options.keydown({ node: file, event: { key: "Enter", altKey: false } }), undefined, "Enter alone opens the file");
  assert.equal(options.keydown({ node: file, event: { key: "ArrowDown", altKey: true } }), undefined);
  assert.equal(options.keydown({ node: undefined, event: { key: "Enter", altKey: true } }), undefined);
  assert.deepEqual(sent.filter((e) => e.type === "workbench:properties-open"), []);
});

// `duplicateNode` over a folder that holds `names`; the copies it asks for.
async function duplicate(rel, names) {
  const { scope: s } = loadComponent("wbFiles", { magics: { $refs: {}, $nextTick: (f) => f() } });
  const real = globalThis.WBDaemon;
  const writes = [];
  globalThis.WBDaemon = {
    observe: () => Promise.resolve({ status: "ok", entries: names.map((name) => ({ name })) }),
    write: (verb, payload) => {
      writes.push([verb, payload.path, payload.to]);
      return Promise.resolve({ status: "ok" });
    },
    withCheckout: (payload) => ({ ...payload }),
  };
  s.$store.projects.setOpen("o/r");
  s.relPath = () => rel;
  s.onTreeDirty = async () => {};
  s.revealRel = () => Promise.resolve(null);
  try {
    await s.duplicateNode({});
  } finally {
    globalThis.WBDaemon = real;
  }
  return writes;
}

test("a duplicate lands beside its file under the first copy name the folder does not hold", async () => {
  assert.deepEqual(await duplicate("docs/a.txt", ["a.txt", "a copy.txt"]), [["file.copy", "docs/a.txt", "docs/a copy 2.txt"]]);
  assert.deepEqual(await duplicate("plan.md", ["plan.md"]), [["file.copy", "plan.md", "plan copy.md"]]);
  // A leading dot is the whole name of a dotfile, not an extension.
  assert.deepEqual(await duplicate("cfg/.env", [".env"]), [["file.copy", "cfg/.env", "cfg/.env copy"]]);
});

// The open tabs of the folder a nudge names read their bytes again.
test("a nudge re-reads an image tab through the image read, and a text tab through file.read", async () => {
  const { scope: s, shell } = loadComponent("wbFiles", { magics: { $refs: {}, $nextTick: (f) => f() } });
  const real = { daemon: globalThis.WBDaemon, viewer: globalThis.WBViewer };
  const reads = [];
  const changed = [];
  globalThis.WBDaemon = {
    observe: (verb, payload) => {
      reads.push([verb, payload.path]);
      return Promise.resolve({ status: "ok", content: "text" });
    },
    readImage: (project, path) => {
      reads.push(["image", path]);
      return Promise.resolve("data:image/png;base64,AA==");
    },
    withCheckout: (payload) => ({ ...payload }),
  };
  globalThis.WBViewer = { encodingOf: () => null, externalChange: (id, content) => changed.push([id, content]) };
  shell.$store.projects.setOpen("o/r");
  shell.$store.tabs.tabs.push(
    { id: "i", kind: "image", project: "o/r", path: "img/a.png", checkout: null },
    { id: "t", kind: "code", project: "o/r", path: "img/b.txt", checkout: null },
    { id: "o", kind: "code", project: "o/r", path: "doc/c.txt", checkout: null },
  );
  try {
    await s.refreshOpenViewers("img");
  } finally {
    globalThis.WBDaemon = real.daemon;
    globalThis.WBViewer = real.viewer;
  }
  assert.deepEqual(reads, [["image", "img/a.png"], ["file.read", "img/b.txt"]]);
  assert.deepEqual(changed, [["i", "data:image/png;base64,AA=="], ["t", "text"]]);
});

// pin: the search button shows the title with its keys —
// the binding is only in the markup, and node runs no Alpine over it.
test("the files search button names its keys from the list of commands", () => {
  assert.equal(loadComponent("wbFiles").scope.searchFilesTitle, "Search files (Ctrl+Shift+F)");
  assert.match(partMarkup("wb-files.part.html"), /:title="searchFilesTitle"/u);
});

test("an upload can be cancelled until it is done", () => {
  const { files } = page();
  for (const state of ["sending", "waiting", "failed"]) assert.equal(files.uploadCancellable({ state }), true, state);
  assert.equal(files.uploadCancellable({ state: "done" }), false);
});

test("the stale-list note shows only with rows on screen and no peer down", () => {
  const { files } = page();
  assert.equal(files.treeStaleShown(), false);
  files.treeStale = "The list shown is the last one read.";
  assert.equal(files.treeStaleShown(), true);
  files.treeError = "the daemon answered 500";
  assert.equal(files.treeStaleShown(), false);
});
