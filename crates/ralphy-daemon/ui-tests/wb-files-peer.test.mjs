// Unit tests for assets/ui/wb-files.ts while the open project's peer is down.
import { test } from "node:test";
import assert from "node:assert/strict";
import { FILES_PEER, FILES_REF, peerFilesShell, mountedTreeOptions } from "./wb-files-fixture.mjs";

// ---- FILES while the open project's peer is down -------------------------------

function Wunderbaum() {}

// Mounts the tree of `state` over a stand-in Wunderbaum and returns the instance.
function mountStandIn(state, window, document) {
  window.WBDaemon = {};
  document.querySelector = (sel) => (sel === ".files-pane .wb-host" ? { addEventListener() {} } : null);
  state.$store.projects.setProjects([{ key: FILES_REF, slug: "o/r", daemon: FILES_PEER, tree: [] }]);
  state.useDaemonTree = () => true;
  state.loadTreeLevel = () => Promise.resolve([]);
  let made = null;
  const realMar10 = globalThis.mar10;
  globalThis.mar10 = {
    Wunderbaum: function () {
      made = { destroy() {} };
      return made;
    },
  };
  try {
    state.mountTree();
  } finally {
    globalThis.mar10 = realMar10;
  }
  return made;
}

// Alpine wraps every field of the data in its Proxy; a node reached through it
// is not the node Wunderbaum's painter compares by identity.
test("the files tree stays out of the reactive data", () => {
  const { state, data, window, document } = peerFilesShell("reachable");
  const handle = mountStandIn(state, window, document);
  assert.ok(handle, "the mount built a tree");
  assert.equal(Object.values(data).includes(handle), false);
});

test("the files tree accessor returns the mounted instance, then null", () => {
  const { state, window, document } = peerFilesShell("reachable");
  assert.equal(state.tree(), null, "no tree before the mount");
  const handle = mountStandIn(state, window, document);
  assert.equal(state.tree(), handle);
  state.destroyTree();
  assert.equal(state.tree(), null);
});

// The one files host outlives every mount: a project switch, a checkout
// change and a woken peer each mount the tree again on the same element.
test("a right-click on the tree opens the menu once, however often the tree was mounted", () => {
  const { state, window, document } = peerFilesShell("reachable");
  window.WBDaemon = {};
  const listeners = [];
  const host = { addEventListener: (type, fn) => type === "contextmenu" && listeners.push(fn) };
  document.querySelector = (sel) => (sel === ".files-pane .wb-host" ? host : null);
  state.$store.projects.setProjects([{ key: FILES_REF, slug: "o/r", daemon: FILES_PEER, tree: [] }]);
  state.useDaemonTree = () => true;
  state.loadTreeLevel = () => Promise.resolve([]);
  const menus = [];
  state.showMenu = (x, y, node) => menus.push([x, y, node]);
  const realMar10 = globalThis.mar10;
  globalThis.mar10 = { Wunderbaum: Object.assign(Wunderbaum, { getNode: () => null }) };
  const rightClick = () => listeners.forEach((fn) => fn({ clientX: 7, clientY: 9, preventDefault() {} }));
  try {
    state.mountTree();
    rightClick();
    assert.deepEqual(menus, [[7, 9, null]], "the first mount wires the menu");
    state.mountTree();
    state.mountTree();
    menus.length = 0;
    rightClick();
  } finally {
    globalThis.mar10 = realMar10;
  }
  assert.deepEqual(menus, [[7, 9, null]]);
});

test("a tree level that fails to load draws no error row, closes, and asks the fleet", async () => {
  const { state, shell, window, document } = peerFilesShell("reachable");
  const options = mountedTreeOptions(state, window, document);
  state.loadTreeLevel = () => Promise.reject(new Error("macOS 15 did not answer: os error 10054. Start its daemon."));
  let fleetReads = 0;
  shell.readFleetNow = () => {
    fleetReads += 1;
    return Promise.resolve();
  };
  const closed = [];
  const node = { title: "src", data: {}, lazy: true, parent: null, setExpanded: (flag) => closed.push(flag) };
  state.relPath = () => "src";
  const result = await options.lazyLoad({ node });
  await new Promise((r) => { setTimeout(r, 0); });
  assert.equal(result, false, "a rethrow makes Wunderbaum draw an error row");
  assert.deepEqual(closed, [false]);
  assert.equal(fleetReads, 1);
  assert.ok(state.treeStale.endsWith("The list shown is the last one read."), state.treeStale);
});

const folderNode = (rel) => ({ rel, children: null });

test("while the peer is down, only a folder read before opens", () => {
  const { state, shell, window, document } = peerFilesShell("unreachable");
  const options = mountedTreeOptions(state, window, document);
  state.relPath = (n) => n.rel;
  state._treeCache.set(state.treeKey("read"), []);
  assert.equal(options.beforeExpand({ flag: true, node: folderNode("never") }), false);
  assert.equal(options.beforeExpand({ flag: true, node: folderNode("read") }), undefined);
  assert.equal(options.beforeExpand({ flag: false, node: folderNode("never") }), undefined);
  shell.fleetPeers[0].state = "reachable";
  assert.equal(options.beforeExpand({ flag: true, node: folderNode("never") }), undefined);
});

test("FILES names the down peer from the fleet, and the read that calls it back re-reads the tree", () => {
  const { state, shell } = peerFilesShell("tunnel-silent");
  const down = state.openPeerDown();
  assert.equal(down?.daemon, FILES_PEER);
  assert.equal(state.peerDownText(down), "corcino-mac is not connected. The list shown is the last one read.");
  assert.equal(state.peerDownAction(down), "Try again");
  const reread = [];
  state.revalidateLevel = (rel) => reread.push(rel);
  const folders = [
    { rel: "src", expanded: true },
    { rel: "docs", expanded: false },
  ];
  state.tree = () => ({ root: { visit: (fn) => folders.forEach((f) => fn(f)) } });
  state.isFolder = () => true;
  state.relPath = (n) => n.rel;
  state.treeStale = "Could not refresh the file list.";
  state.filesFollowFleet();
  assert.deepEqual(reread, [], "the peer is still down");
  shell.fleetPeers[0].state = "reachable";
  state.filesFollowFleet();
  assert.deepEqual(reread, ["", "src"]);
  assert.equal(state.treeStale, "");
  state.filesFollowFleet();
  assert.deepEqual(reread, ["", "src"], "only the read that calls the peer back");
});
