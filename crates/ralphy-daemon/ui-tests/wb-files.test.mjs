// Unit tests for assets/ui/wb-files.ts, the Alpine component of the FILES tree
// of the open project, and for assets/ui/wb-file-paths.ts, the path rules it
// shares with app.ts and the move dialog. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";
import {
  classify,
  fileTabId,
  isProtectedDir,
  newEntryTitle,
  parentRel,
  underProtectedDir,
} from "../assets/ui/wb-file-paths.ts";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// One page with the shell and the files, and a window that delivers the
// events it is sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds the component.
function page() {
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
  const files = loadComponent("wbFiles", { from: loaded, magics: { $nextTick: (fn) => fn(), $refs: {} } });
  files.scope.init();
  const fire = (type, detail) => loaded.window.dispatchEvent({ type, detail });
  return { shell: loaded.state, files: files.scope, window: loaded.window, fire, events, listeners };
}

// ---- the path rules (wb-file-paths.ts) ---------------------------------------

test("the path rules: the viewer of a name, the parent of a rel path, a tab id, a create title", () => {
  const rows = [
    ["idea.note", "note"],
    ["README.md", "markdown"],
    ["notes.MARKDOWN", "markdown"],
    ["logo.PNG", "image"],
    ["archive.zip", "binary"],
    ["main.rs", "code"],
    ["Makefile", "code"],
  ];
  for (const [name, want] of rows) assert.equal(classify(name), want, name);
  assert.deepEqual([parentRel("a/b/c.txt"), parentRel("top.txt")], ["a/b", ""]);
  // The primary's id has no checkout; a worktree's names it.
  assert.equal(fileTabId("o/r", "a.md", null), "file:o/r:a.md");
  assert.equal(fileTabId("o/r", "a.md", "wt-a"), "file:o/r@wt-a:a.md");
  assert.equal(newEntryTitle("file", ""), "New file in the project root");
  assert.equal(newEntryTitle("folder", "src"), "New folder in src");
});

test("the protected directories: .git and .ralphy in any case, and the note in the notes directory", () => {
  assert.deepEqual([".git", ".GIT", ".ralphy", "src"].map(isProtectedDir), [true, true, true, false]);
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

test("every name the markup of wbFiles reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbFiles");
  const markup = componentMarkup(HTML, "wbFiles");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component; `x-ref` names an element.
    if (attr === "x-data" || attr === "x-ref") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$")) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 20, `the markup was read: ${read} names`);
});

test("nothing outside the files element names their state", () => {
  const start = HTML.indexOf('x-data="wbFiles"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["fileSearch", "treeStale", "treeError", "treeNotLive", "treeLoading", "openPeerDown", "createHere", "toggleFileSearch"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the files, and a create re-lists before it reveals", async () => {
  const { files, fire } = page();
  const calls = [];
  files.filesFollowProject = () => calls.push("project");
  files.filesFollowFleet = () => calls.push("fleet");
  files.filesFollowWake = (daemon) => calls.push(`wake ${daemon}`);
  files.filesFollowCheckout = () => calls.push("checkout");
  files.openFileSearch = () => calls.push("search");
  files.performMove = (from, to) => calls.push(`move ${from} ${to}`);
  files._treeSub = { resume: (stale) => calls.push(`resume ${stale}`), replay: () => calls.push("replay") };
  let settle;
  files.onTreeDirty = (rel) => {
    calls.push(`dirty ${rel}`);
    return new Promise((r) => (settle = r));
  };
  files.revealRel = (rel) => calls.push(`reveal ${rel}`);
  fire("workbench:project-changed", { slug: "o/r", previous: null });
  fire("workbench:fleet-read");
  fire("workbench:peer-woken", { daemon: "01PEER" });
  fire("workbench:checkout-changed");
  fire("workbench:sockets-resume", { stale: true });
  fire("workbench:panels-reread");
  fire("workbench:file-search-open");
  fire("workbench:move-confirmed", { from: "a.txt", to: "dst/a.txt" });
  fire("workbench:tree-dirty", { rel: "src", reveal: "src/new.txt" });
  assert.equal(calls.at(-1), "dirty src", "the reveal waits for the level");
  settle();
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(calls, [
    "project",
    "fleet",
    "wake 01PEER",
    "checkout",
    "resume true",
    "replay",
    "search",
    "move a.txt dst/a.txt",
    "dirty src",
    "reveal src/new.txt",
  ]);
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
  shell.checkouts = { [`${peer}/o/r`]: "wt-a" };
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
  await new Promise((r) => setTimeout(r, 5));
  assert.deepEqual(
    events.filter((e) => e.type === "workbench:head-moved").map((e) => e.detail),
    [{ ref: "o/r" }],
  );
});

test("the Move item asks the move dialog with the full rel path", () => {
  const { shell, files, events } = page();
  let items = null;
  shell.renderMenu = (_x, _y, list) => (items = list);
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
    [...own._treeCache.keys()].map((k) => k.split("\n").at(-1)).sort();

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

test("the Move item is withheld inside .git and .ralphy, except for a note in the notes directory", () => {
  // The context menu is where the UI's mirror of the daemon's carve-out
  // (`fswrite::is_note_in_notes_dir`) decides what to offer: exactly
  // `.ralphy/notes/<name>.note`, nothing deeper or differently named.
  const { scope: own, shell } = loadComponent("wbFiles");
  let items = null;
  shell.renderMenu = (_x, _y, list) => {
    items = list;
  };
  const node = (rel) =>
    rel.split("/").reduce((parent, title) => ({ title, parent, data: {}, children: null }), {
      title: "root",
    });
  const offersMove = (rel) => {
    own.showMenu(0, 0, node(rel));
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

test("emitCreate sends the directory the create lands in", () => {
  const { scope: state, window } = loadComponent("wbFiles");
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push({ action, ...detail }) };
  state.$store.projects.setOpen("owner/repo");
  const root = { title: "root", parent: null };
  const src = { title: "src", parent: root, data: { folder: true } };
  const file = { title: "main.rs", parent: src, data: {} };
  state.emitCreate(file, "file");
  state.emitCreate(src, "folder");
  state.emitCreate(null, "file");
  assert.deepEqual(
    emitted.map((e) => [e.action, e.project, e.path, e.kind]),
    [
      ["create", "owner/repo", "src", "file"],
      ["create", "owner/repo", "src", "folder"],
      ["create", "owner/repo", "", "file"],
    ],
  );
});

// ---- the FILES search (ADR-0036 amendment 2026-09-15) --------------------------
// The component's half, driven with `WBDaemon.observe` stubbed and no tree
// mounted — the folds that decide what is sent and which reply is believed run
// without Wunderbaum.
async function withSearchShell(run, reply = { status: "ok", hits: [], truncated: false }) {
  const { scope: state, window } = loadComponent("wbFiles", { magics: { $refs: {}, $nextTick: (f) => f() } });
  const calls = [];
  let answer = reply;
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      return typeof answer === "function" ? answer() : answer;
    },
    // The real door's rule (wb-daemon.test.mjs pins it): the key only for a
    // real name, so a no-selection payload is the pre-#406 one.
    withCheckout: (payload, checkout) =>
      checkout ? { ...payload, checkout: String(checkout) } : { ...payload },
  };
  state.$store.projects.setOpen("owner/repo");
  return await run(state, calls, (a) => (answer = a));
}

test("fileSearchNow sends the mode's verb with the trimmed query", async () => {
  await withSearchShell(async (s, calls) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "  plan ";
    await s.fileSearchNow();
    assert.deepEqual(calls.at(-1), { verb: "tree.find", payload: { repo: "owner/repo", query: "plan" } });
    await s.setFileSearchMode("content");
    assert.deepEqual(calls.at(-1), { verb: "tree.grep", payload: { repo: "owner/repo", query: "plan" } });
    // NEGATIVE CONTROL: under the floor nothing is sent and the note is clear.
    s.fileSearch.query = "p";
    await s.fileSearchNow();
    assert.equal(calls.length, 2);
    assert.equal(s.fileSearch.note, "");
  });
});

test("a reply that is not the newest is dropped, never painted", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    let release;
    answer(() => new Promise((r) => (release = r)));
    s.fileSearch.query = "old";
    const slow = s.fileSearchNow();
    // A newer search lands first.
    answer({ status: "ok", hits: [{ path: "new.md" }], truncated: false });
    s.fileSearch.query = "new";
    await s.fileSearchNow();
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    // …then the slow one resolves with its stale hits: ignored.
    release({ status: "ok", hits: [{ path: "old.md" }], truncated: false });
    await slow;
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    assert.equal(calls.length, 2);
  });
});

test("the gutter says what the tree cannot: the cap, a miss, a refusal", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    // [case, the daemon's reply, the note under the search box]
    const rows = [
      [
        "the cap",
        { status: "ok", hits: [{ path: "a" }], truncated: true },
        "First 200 matches. Narrow the search to see more.",
      ],
      ["a miss", { status: "ok", hits: [], truncated: false }, "No matches"],
      [
        "a refusal",
        { status: "error", reason: "unknown verb" },
        "Could not search: the daemon does not know that command.",
      ],
    ];
    for (const [name, reply, note] of rows) {
      answer(reply);
      await s.fileSearchNow();
      assert.equal(s.fileSearch.note, note, name);
    }
  });
});

test("only a live CONTENT search lends its term to a tab opened from the tree", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "needle";
    answer({ status: "ok", hits: [{ path: "a.md", count: 2 }], truncated: false });
    await s.setFileSearchMode("content");
    assert.equal(s.fileSearchFindTerm(), "needle");
    // NEGATIVE CONTROLS: a name search, or a closed field, says nothing about
    // what is inside a file.
    await s.setFileSearchMode("name");
    assert.equal(s.fileSearchFindTerm(), null);
    await s.setFileSearchMode("content");
    s.fileSearch.open = false;
    assert.equal(s.fileSearchFindTerm(), null);
  });
});

test("closing the search forgets the query and the hits", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    answer({ status: "ok", hits: [{ path: "a" }], truncated: false });
    await s.fileSearchNow();
    await s.closeFileSearch();
    assert.equal(s.fileSearch.open, false);
    assert.equal(s.fileSearch.query, "");
    assert.deepEqual(s.fileSearch.hits, []);
    assert.equal(s.fileSearch.note, "");
    assert.equal(s.fileSearch.expandedBefore, null);
  });
});

// ---- FILES while the open project's peer is down -------------------------------

const FILES_PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const FILES_REF = `${FILES_PEER}/o/r`;

// The files with the peer project open and the peer in the given fleet state.
// `shell` holds the fleet, which the files read through `fleetGroups`.
function peerFilesShell(state) {
  const loaded = loadShell();
  const files = loadComponent("wbFiles", { from: loaded });
  const s = files.scope;
  s.$store.projects.setProjects([]);
  loaded.state.fleetPeers = [{ daemon_id: FILES_PEER, name: "corcino-mac", environment: "macOS 15", tunnel: true, state, diagnosis: `diagnosis of ${state}` }];
  s.$store.projects.setOpen(FILES_REF);
  s.treeMem();
  return { state: s, shell: loaded.state, window: loaded.window, document: loaded.document };
}

// The options `mountTree` gives Wunderbaum, from a stand-in that keeps them.
function mountedTreeOptions(s, window, document) {
  let options = null;
  // No `/ws/tree` subscription: this checks the tree's own options.
  window.WBDaemon = {};
  const realMar10 = globalThis.mar10;
  globalThis.mar10 = {
    Wunderbaum: function (o) {
      options = o;
    },
  };
  const host = { addEventListener() {} };
  document.querySelector = (sel) => (sel === ".files-pane .wb-host" ? host : null);
  s.$store.projects.setProjects([{ key: FILES_REF, slug: "o/r", daemon: FILES_PEER, tree: [] }]);
  s.useDaemonTree = () => true;
  s.loadTreeLevel = () => Promise.resolve([]);
  try {
    s.mountTree();
  } finally {
    globalThis.mar10 = realMar10;
  }
  return options;
}

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
  globalThis.mar10 = { Wunderbaum: Object.assign(function () {}, { getNode: () => null }) };
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
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(result, false, "a rethrow makes Wunderbaum draw an error row");
  assert.deepEqual(closed, [false]);
  assert.equal(fleetReads, 1);
  assert.ok(state.treeStale.endsWith("The list shown is the last one read."), state.treeStale);
});

test("while the peer is down, only a folder read before opens", () => {
  const { state, shell, window, document } = peerFilesShell("unreachable");
  const options = mountedTreeOptions(state, window, document);
  state.relPath = (n) => n.rel;
  state._treeCache.set(state.treeKey("read"), []);
  const folder = (rel) => ({ rel, children: null });
  assert.equal(options.beforeExpand({ flag: true, node: folder("never") }), false);
  assert.equal(options.beforeExpand({ flag: true, node: folder("read") }), undefined);
  assert.equal(options.beforeExpand({ flag: false, node: folder("never") }), undefined);
  shell.fleetPeers[0].state = "reachable";
  assert.equal(options.beforeExpand({ flag: true, node: folder("never") }), undefined);
});

test("FILES names the down peer from the fleet, and the read that calls it back re-reads the tree", () => {
  const { state, shell } = peerFilesShell("tunnel-silent");
  const down = state.openPeerDown();
  assert.equal(down?.daemon, FILES_PEER);
  assert.equal(state.peerDownText(down), "corcino-mac is not connected. The list shown is the last one read.");
  assert.equal(state.peerDownAction(down), "Try again");
  const reread = [];
  state.revalidateLevel = (rel) => reread.push(rel);
  state._tree = {};
  const folders = [
    { rel: "src", expanded: true },
    { rel: "docs", expanded: false },
  ];
  state.rawTree = () => ({ root: { visit: (fn) => folders.forEach(fn) } });
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
