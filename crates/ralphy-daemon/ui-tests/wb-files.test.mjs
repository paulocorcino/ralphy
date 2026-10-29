// Unit tests for assets/ui/wb-files.ts, the Alpine component of the FILES tree
// of the open project, and for assets/ui/wb-file-paths.ts, the path rules it
// shares with app.ts and the move dialog. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { assembledPage, bindingNames, loadComponent, loadShell, partMarkup, withoutComments } from "./harness.mjs";
import {
  classify,
  fileTabId,
  isProtectedDir,
  newEntryTitle,
  parentRel,
  tabIcon,
  underProtectedDir, fullPath } from "../assets/ui/wb-file-paths.ts";

// The part that holds the files pane's markup.
const PART = "wb-files.part.html";

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
    ["erd.mmd", "mermaid"],
    ["FLOW.MMD", "mermaid"],
    ["schema.dbml", "dbml"],
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
  const markup = partMarkup(PART);
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component; `x-ref` names an element.
    if (attr === "x-data" || attr === "x-ref") continue;
    const raw = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
    for (const n of bindingNames(raw)) {
      // `u` is the row of the upload card's `x-for`.
      if (n.startsWith("$") || n === "u") continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 20, `the markup was read: ${read} names`);
});

test("nothing outside the files element names their state", () => {
  const outside = withoutComments(assembledPage("index.html", { skip: PART }));
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
  await new Promise((r) => setTimeout(r, 5));
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
  shell.$store.tabs.renderMenu = (_x, _y, list) => {
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
// `shell` is the Alpine store `projects`: it holds the fleet, which the files
// read through `fleetGroups`.
function peerFilesShell(state) {
  const loaded = loadShell();
  const files = loadComponent("wbFiles", { from: loaded });
  const s = files.scope;
  s.$store.projects.setProjects([]);
  loaded.state.$store.projects.fleetPeers = [{ daemon_id: FILES_PEER, name: "corcino-mac", environment: "macOS 15", tunnel: true, state, diagnosis: `diagnosis of ${state}` }];
  s.$store.projects.setOpen(FILES_REF);
  s.treeMem();
  return { state: s, shell: loaded.state.$store.projects, window: loaded.window, document: loaded.document };
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
    data: { folder: !!spec.folder },
    lazy: !!spec.folder,
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
    async load(source) {
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
  s._tree = tree;
  s.fetchTreeLevel = async (rel) => disk[rel];
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
  s.fetchTreeLevel = async (rel) => {
    s._revealSeq += 1;
    s._revealedRel = "y.txt";
    return disk[rel];
  };
  await s._reconcileOnce(tree.root, "");
  assert.equal(tree.relOf(tree.active), "y.txt");
  assert.equal(tree.element.scrollTop, 0);
});

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
