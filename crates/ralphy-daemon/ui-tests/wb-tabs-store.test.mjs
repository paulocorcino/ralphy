// The Alpine store `tabs` (wb-tabs-store.ts): the canvas tabs, the slot
// beside the active tab, the split and the tab menu. Built per page by the
// harness, as `main.ts` registers it; `window.Alpine.nextTick` runs its
// callback at once unless a test makes it a sink.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

// The viewer pin (#406): a tab is pinned to the checkout it was opened in, the
// pin is part of its identity, and an explicit `null` is the primary — not
// "whatever is selected now". Driven with no DOM and the viewer mount skipped
// (`Alpine.nextTick` swallowed): the tab records are the fold under test.
test("openTab pins the tab to the selected checkout and keys the tab by it", () => {
  const { state, window } = loadShell();
  window.WBView = { patch() {}, read: () => null };
  window.Alpine.nextTick = () => {};
  state.$store.projects.setOpen("owner/repo");
  state.$store.checkouts.checkouts = { "owner/repo": "wt-a" };

  state.$store.tabs.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "x" });
  const pinned = state.$store.tabs.tabs.find((t) => t.path === "README.md");
  assert.equal(pinned.checkout, "wt-a", "the default pin is the project's selection");
  assert.equal(pinned.id, "file:owner/repo@wt-a:README.md");

  // The same rel under the PRIMARY is another tab, not the worktree's one.
  state.$store.tabs.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "y", checkout: null });
  const primary = state.$store.tabs.tabs.filter((t) => t.path === "README.md");
  assert.equal(primary.length, 2, "two trees, two tabs");
  assert.equal(primary[1].checkout, null, "an explicit null is the primary even while wt-a is selected");
  assert.equal(primary[1].id, "file:owner/repo:README.md", "the primary's id is the pre-#406 spelling");

  // Selection moves on; the pins do not.
  state.$store.checkouts.checkouts = {};
  state.$store.tabs.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "z", checkout: "wt-a" });
  assert.equal(state.$store.tabs.tabs.filter((t) => t.path === "README.md").length, 2, "the pinned tab is reused, not duplicated");
  assert.equal(state.$store.tabs.active, "file:owner/repo@wt-a:README.md", "re-opening activates the pinned tab");
});

test("persistView stores the pin and restoreView hands it back explicitly", () => {
  const { state, window } = loadShell();
  let stored = null;
  window.WBView = { patch: (v) => (stored = v), read: () => stored };
  window.Alpine.nextTick = () => {};
  state.$store.projects.setOpen("owner/repo");
  state.$store.checkouts.checkouts = { "owner/repo": "wt-a" };
  state.$store.tabs.openTab({ project: "owner/repo", path: "a.txt", title: "a.txt", ftype: "code", content: "a" });
  state.$store.tabs.openTab({ project: "owner/repo", path: "b.txt", title: "b.txt", ftype: "code", content: "b", checkout: null });
  assert.deepEqual(
    stored.tabs.map((t) => [t.path, t.checkout]),
    [["a.txt", "wt-a"], ["b.txt", null]],
  );

  // A fresh shell whose selection is now the PRIMARY restores the pins as
  // stored — the worktree tab must not be re-pinned to the primary.
  const fresh = loadShell();
  fresh.window.WBView = { patch() {}, read: () => stored };
  fresh.window.Alpine.nextTick = () => {};
  fresh.state.$store.checkouts.checkouts = {};
  fresh.state.$store.tabs.restoreView();
  assert.deepEqual(
    fresh.state.$store.tabs.tabs.filter((t) => t.id.startsWith("file:")).map((t) => [t.id, t.checkout]),
    [["file:owner/repo@wt-a:a.txt", "wt-a"], ["file:owner/repo:b.txt", null]],
  );
});

// --- the slot (ADR-0037 §3c) ----------------------------------------------
// The store reaches `WBViewer` as a bare global; the real one needs a DOM, so
// the fake records what the shell told it to paint and answers a wide canvas.
function slotShell(width = 1280, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  window.Alpine.nextTick = (fn) => fn();
  state.$store.projects.setOpen("o/r");
  const painted = [];
  const real = globalThis.WBViewer;
  globalThis.WBViewer = {
    setActive: (id, slot) => painted.push([id, slot ?? null]),
    width: () => width,
    open() {},
    close() {},
    jumpTo() {},
    find() {},
    repath() {},
  };
  const open = (path, ftype = "code") =>
    state.$store.tabs.openTab({ project: "o/r", path, title: path, ftype, content: "x", checkout: null });
  // The opens resolve their bytes on a microtask, so the fake stays until
  // those landed; restoring it under them would read `open` off `undefined`.
  const restore = async () => {
    await new Promise((r) => setImmediate(r));
    globalThis.WBViewer = real;
  };
  return { state, window, painted, open, stored: () => stored, restore };
}
const A = "file:o/r:a.js";
const B = "file:o/r:b.js";

test("pinning a tab paints it beside the active one; activating it focuses the right pane", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    assert.deepEqual(sh.painted.at(-1), [A, { id: B, mirror: false, focus: false, ratio: null }]);
    sh.state.$store.tabs.activate(B);
    assert.deepEqual(sh.painted.at(-1), [A, { id: B, mirror: false, focus: true, ratio: null }]);
    assert.deepEqual(sh.state.$store.tabs.slot, { kind: "pin", id: B }, "clicking the pinned tab does not unpin it");
    assert.equal(sh.state.$store.tabs.lastLeft, A);
  } finally {
    await sh.restore();
  }
});

test("a tab opened from the tree while pinned becomes the left the pin returns to", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    // `openTab` sets `active` without `activate`: the fold is where lastLeft
    // lives. Its paint comes after the bytes resolve (a microtask here).
    sh.open("c.js");
    await new Promise((r) => setImmediate(r));
    assert.deepEqual(sh.painted.at(-1), ["file:o/r:c.js", { id: B, mirror: false, focus: false, ratio: null }]);
    sh.state.$store.tabs.activate(B);
    assert.equal(sh.painted.at(-1)[0], "file:o/r:c.js", "the pane the operator was reading, not an earlier one");
  } finally {
    await sh.restore();
  }
});

test("closing the pinned tab while it is ACTIVE clears the slot and falls back to a neighbour", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    sh.state.$store.tabs.activate(B);
    sh.state.$store.tabs.closeTab(B);
    assert.equal(sh.state.$store.tabs.slot, null);
    assert.equal(sh.state.$store.tabs.lastLeft, A);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
  } finally {
    await sh.restore();
  }
});

test("a rename re-keys the pin and repaints so the viewer follows the new id", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    sh.state.$store.tabs.repathTabs("b.js", "c.js");
    assert.deepEqual(sh.state.$store.tabs.slot, { kind: "pin", id: "file:o/r:c.js" });
    assert.deepEqual(sh.painted.at(-1), [A, { id: "file:o/r:c.js", mirror: false, focus: false, ratio: null }]);
  } finally {
    await sh.restore();
  }
});

test("a folder move re-paths every tab under it and the active one, and closes a tab whose new path is open", async () => {
  const sh = slotShell();
  try {
    sh.open("dir/x.js");
    sh.open("dirt.js");
    sh.open("dir/y.js");
    sh.open("new/y.js");
    sh.state.$store.tabs.activate("file:o/r:dir/x.js");
    sh.state.$store.tabs.repathTabs("dir", "new");
    const ids = sh.state.$store.tabs.tabs.filter((t) => t.path).map((t) => t.id);
    assert.deepEqual(ids.sort(), ["file:o/r:dirt.js", "file:o/r:new/x.js", "file:o/r:new/y.js"]);
    assert.equal(sh.state.$store.tabs.active, "file:o/r:new/x.js");
    assert.equal(sh.state.$store.tabs.tabs.find((t) => t.id === "file:o/r:new/x.js").path, "new/x.js");
  } finally {
    await sh.restore();
  }
});

test("a move leaves a diff tab and a tab of another project on their path, and the last tab on the left follows its file", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.state.$store.tabs.tabs.push(
      { id: "diff:o/r:a.js", kind: "diff", title: "a.js", path: "a.js", project: "o/r", closable: true, checkout: null },
      { id: "file:x/y:a.js", kind: "code", title: "a.js", path: "a.js", project: "x/y", closable: true, checkout: null },
    );
    sh.state.$store.tabs.activate("consoles");
    sh.state.$store.tabs.lastLeft = A;
    sh.state.$store.tabs.repathTabs("a.js", "z.js");
    const paths = sh.state.$store.tabs.tabs.filter((t) => t.path).map((t) => [t.id, t.path]);
    assert.deepEqual(paths, [
      ["file:o/r:z.js", "z.js"],
      ["diff:o/r:a.js", "a.js"],
      ["file:x/y:a.js", "a.js"],
    ]);
    assert.equal(sh.state.$store.tabs.lastLeft, "file:o/r:z.js");
  } finally {
    await sh.restore();
  }
});

test("a mirror doubles the active code tab and steps aside for a markdown tab", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("R.md", "markdown");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.toggleMirror();
    assert.deepEqual(sh.painted.at(-1), [A, { id: A, mirror: true, focus: false, ratio: null }]);
    sh.state.$store.tabs.activate("file:o/r:R.md");
    assert.deepEqual(sh.painted.at(-1), ["file:o/r:R.md", null]);
    assert.deepEqual(sh.state.$store.tabs.slot, { kind: "mirror" }, "the mirror waits for the next code tab");
    sh.state.$store.tabs.toggleMirror();
    assert.equal(sh.state.$store.tabs.slot, null);
  } finally {
    await sh.restore();
  }
});

test("closing the pinned tab clears the slot; a paneless tab keeps it waiting", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    sh.state.$store.tabs.activate("consoles");
    assert.deepEqual(sh.painted.at(-1), [null, null]);
    assert.deepEqual(sh.state.$store.tabs.slot, { kind: "pin", id: B });
    sh.state.$store.tabs.activate(A);
    assert.equal(sh.painted.at(-1)[1].id, B);
    sh.state.$store.tabs.closeTab(B);
    assert.equal(sh.state.$store.tabs.slot, null);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
    assert.equal(sh.stored().split, null, "the store is told, explicitly, that there is no slot");
  } finally {
    await sh.restore();
  }
});

test("the slot and its ratio persist per client and come back after the tabs do", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    sh.state.$store.tabs.splitRatioChanged(0.6);
    sh.state.$store.tabs.persistView();
    assert.deepEqual(sh.stored().split, { kind: "pin", project: "o/r", path: "b.js", checkout: null, ratio: 0.6 });

    // A fresh shell over the same record, the way a reload is. `Alpine.nextTick` is
    // a sink here (the restore's reads would hit the harness's throwing
    // fetch), so the paint is asked for by hand once the restore has run.
    const fresh = slotShell(1280, sh.stored());
    try {
      fresh.window.Alpine.nextTick = () => {};
      fresh.state.$store.tabs.restoreView();
      assert.deepEqual(fresh.state.$store.tabs.slot, { kind: "pin", id: B });
      assert.equal(fresh.state.$store.tabs.splitRatio, 0.6);
      assert.equal(fresh.state.$store.tabs.active, A);
      fresh.state.$store.tabs.syncViewer();
      assert.deepEqual(fresh.painted.at(-1), [A, { id: B, mirror: false, focus: false, ratio: 0.6 }]);
    } finally {
      await fresh.restore();
    }
  } finally {
    await sh.restore();
  }
});

test("a restore skips a stored tab with no project or path, and a stored active tab it does not carry falls back to Consoles", async () => {
  const seed = {
    tabs: [
      { project: "o/r", path: "a.js", title: "a.js", kind: "code", checkout: null },
      { project: "o/r", title: "no path", kind: "code" },
      { path: "b.js", title: "no project", kind: "code" },
      null,
    ],
    active: "file:o/r:gone.js",
    split: null,
  };
  const sh = slotShell(1280, seed);
  try {
    sh.window.Alpine.nextTick = () => {};
    sh.state.$store.tabs.restoreView();
    assert.deepEqual(
      sh.state.$store.tabs.tabs.filter((t) => t.id.startsWith("file:")).map((t) => t.id),
      [A],
    );
    assert.equal(sh.state.$store.tabs.active, "consoles");
  } finally {
    await sh.restore();
  }
});

test("under the width floor the shell paints single and keeps the slot for a wider canvas", async () => {
  const sh = slotShell(800);
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.$store.tabs.activate(A);
    sh.state.$store.tabs.pinTab(B);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
    assert.deepEqual(sh.state.$store.tabs.slot, { kind: "pin", id: B });
    assert.equal(sh.state.$store.tabs.splitAvailable(), false);
  } finally {
    await sh.restore();
  }
});

// Closing the ACTIVE tab brings its right neighbour to the front; with none on
// the right, the left one.
test("closing the active tab activates its neighbour, the right one first", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.open("c.js");
    sh.state.$store.tabs.activate(B);
    sh.state.$store.tabs.closeTab(B);
    assert.equal(sh.state.$store.tabs.active, "file:o/r:c.js", "the right neighbour");
    sh.state.$store.tabs.closeTab("file:o/r:c.js");
    assert.equal(sh.state.$store.tabs.active, A, "no right neighbour: the left one");
  } finally {
    await sh.restore();
  }
});

// The store's `init()` hears a component ask for a tab (the Spend tab).
test("a component's tab request adds the tab once and brings it to the front", () => {
  const listeners = {};
  const { state, window } = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
    },
  });
  const tabs = state.$store.tabs;
  tabs.init();
  const tab = { id: "spend", kind: "spend", title: "Spend", icon: "bi bi-cash-stack", closable: true };
  const activated = [];
  tabs.activate = (id) => activated.push(id);
  window.dispatchEvent({ type: "workbench:tab-open", detail: { tab } });
  window.dispatchEvent({ type: "workbench:tab-open", detail: { tab } });
  assert.equal(tabs.tabs.filter((t) => t.id === "spend").length, 1);
  assert.deepEqual(activated, ["spend", "spend"]);
});

// The context menu builds each item from elements: a label can name a file,
// and a file name such as `<img src=x>` must reach the menu as text.
test("renderMenu sets a label as text, never as markup", () => {
  const created = [];
  const element = (tag) => {
    const el = {
      tag,
      children: [],
      style: {},
      className: "",
      textContent: "",
      innerHTMLWrites: [],
      append(...kids) {
        this.children.push(...kids);
      },
      set innerHTML(v) {
        this.innerHTMLWrites.push(v);
      },
    };
    created.push(el);
    return el;
  };
  const menu = element("div");
  const { state } = loadShell({
    document: { getElementById: () => menu, createElement: element },
  });
  // `renderMenu` reads the BARE viewport globals a browser has; lend them.
  globalThis.innerWidth = 1440;
  globalThis.innerHeight = 900;
  try {
    state.$store.tabs.renderMenu(0, 0, [{ icon: "bi-x", label: "<img src=x>", run() {} }]);
  } finally {
    delete globalThis.innerWidth;
    delete globalThis.innerHeight;
  }
  const button = menu.children[0];
  assert.equal(button.tag, "button");
  const [icon, label] = button.children;
  assert.equal(icon.tag, "i");
  assert.equal(icon.className, "bi bi-x");
  assert.equal(label.tag, "span");
  assert.equal(label.textContent, "<img src=x>");
  assert.ok(!created.some((el) => el.tag === "img"), "an img element was created");
  for (const el of created) {
    for (const w of el.innerHTMLWrites) {
      assert.ok(!w.includes("<img"), `innerHTML took the label: ${w}`);
    }
  }
});
