// Unit tests for assets/ui/wb-console-title.ts — the console name, the title
// and the worktree switcher (`wb-console-checkout.ts`, built inside the
// title). `createTitle` is driven with fake `deps` and a fake document, with
// no console and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createTitle } from "../assets/ui/wb-console-title.ts";
import { checkoutsStore } from "../assets/ui/wb-checkouts-store.ts";

// An element with the few DOM members the title uses. Listeners are kept so a
// test can fire them.
function fakeElement(tag = "div") {
  const listeners = {};
  const el = {
    tagName: tag,
    className: "",
    style: {},
    title: "",
    innerHTML: "",
    textContent: "",
    value: "",
    children: [],
    parent: null,
    isConnected: true,
    offsetWidth: 80,
    listeners,
    classList: { contains: () => false },
    setAttribute() {},
    append(...kids) {
      for (const k of kids) if (k && typeof k === "object") k.parent = el;
      el.children.push(...kids);
    },
    remove() {
      if (el.parent) el.parent.children = el.parent.children.filter((k) => k !== el);
      el.isConnected = false;
    },
    replaceWith(other) {
      const at = el.parent.children.indexOf(el);
      el.parent.children[at] = other;
      other.parent = el.parent;
      el.isConnected = false;
    },
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    fire(type, ev = {}) {
      for (const fn of [...(listeners[type] || [])]) fn({ stopPropagation() {}, ...ev });
    },
    focus() {},
    select() {},
    contains: (node) => node === el,
    querySelector: () => null,
    getBoundingClientRect: () => ({ left: 10, top: 10, bottom: 30, width: 80, height: 20 }),
  };
  return el;
}

// The page's document: elements, and listeners a test fires by type.
function fakeDocument() {
  const listeners = {};
  return {
    body: fakeElement("body"),
    getElementById: () => null,
    createElement: (tag) => fakeElement(tag),
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    removeEventListener(type, fn) {
      listeners[type] = (listeners[type] || []).filter((f) => f !== fn);
    },
  };
}

// Every member of `TitleDeps`; `setWin` records its calls.
function fakeDeps(more = {}) {
  const writes = [];
  const deps = {
    window: {},
    document: fakeDocument(),
    OPTS: {},
    wins: new Set(),
    pageWindows: () => [],
    desk: () => [],
    lastSessions: () => [],
    askConfirm: async () => true,
    projectNameOf: (ref) => ref.split("/").pop(),
    projectTitleOf: (ref) => ref,
    setWin: (win, fields) => writes.push({ win, fields }),
    ...more,
  };
  return { deps, writes };
}

// A console window as the console builds it: an agent window that can
// relaunch, in `repo`.
function fakeWindow(more = {}) {
  return {
    ...fakeElement("div"),
    _deskId: "w1",
    _deskRepo: "paulocorcino/vibeforge",
    _deskAgent: "claude",
    _deskKind: "agent",
    _deskConsoleName: "vibeforge #1",
    _relaunchIn() {},
    ...more,
  };
}

test("takenNames reads the desk at each call and leaves out the console being renamed", () => {
  let desk = [{ id: "w1", consoleName: "vibeforge #1" }];
  const onStage = { _deskId: "w9", _deskConsoleName: "shell #1" };
  const { deps } = fakeDeps({
    desk: () => desk,
    pageWindows: () => [onStage],
  });
  const title = createTitle(deps);
  // The console reassigns its desk mirror after the title is built.
  desk = [
    { id: "w1", consoleName: "vibeforge #1" },
    { id: "w2", consoleName: "vibeforge #2" },
  ];
  assert.deepEqual(title.takenNames("w1"), ["vibeforge #2", "shell #1"]);
});

test("a rename to an empty name takes the console prefix and the first free number", () => {
  const { deps, writes } = fakeDeps({
    desk: () => [
      { id: "w1", consoleName: "vibeforge #1" },
      { id: "w2", consoleName: "vibeforge #2" },
      { id: "w3", consoleName: "vibeforge #3" },
    ],
  });
  const title = createTitle(deps);
  // Its own name is free to take again.
  const win = fakeWindow({ _deskId: "w3", _deskConsoleName: "vibeforge #3" });
  const bar = fakeElement("span");
  const span = fakeElement("span");
  bar.append(span);
  title.startRename(win, span);
  const input = bar.children[0];
  assert.equal(input.className, "session-name-input");
  input.value = "   ";
  input.fire("keydown", { key: "Enter" });
  assert.equal(win._deskConsoleName, "vibeforge #3");
  assert.deepEqual(writes.at(-1).fields, { consoleName: "vibeforge #3" });
});

// The page's store `checkouts`, built as `main.ts` builds it, on a window whose
// daemon answers `worktree.list` with the worktrees of `repos[ref]`. The console is told by
// the store through `WBConsole.ingestWorktrees`, so `title` is wired there.
function pageWithStore(repos) {
  const store = checkoutsStore({ messages: { flash() {} } });
  const window = {
    Alpine: { store: (name) => (name === "checkouts" ? store : undefined) },
    WBDaemon: {
      observe: async (verb, { repo }) =>
        verb === "worktree.list" ? { status: "ok", checkouts: { worktrees: repos[repo] } } : null,
    },
    addEventListener() {},
    dispatchEvent: () => true,
  };
  globalThis.window = window;
  const { deps } = fakeDeps({ window });
  const title = createTitle(deps);
  window.WBConsole = { ingestWorktrees: title.ingestWorktrees };
  return { store, title };
}

// The names of the rows of the menu the checkout button opens.
function switcherRows(title, win) {
  const bar = fakeElement("span");
  title.renderTitle(win, bar, { checkout: "primary" });
  bar.children.find((k) => k.className === "session-checkout").fire("click");
  const menu = win.children.find((k) => k.className === "session-checkout-menu");
  return menu.children
    .map((r) => r.children.find((k) => k.className === "session-checkout-name")?.textContent)
    .filter(Boolean);
}

test("a worktree being deleted is not offered in the console switcher", async () => {
  const { store, title } = pageWithStore({
    "paulocorcino/vibeforge": [
      { name: "feat-a", branch: "feat-a" },
      { name: "feat-b", branch: "feat-b" },
    ],
  });
  const win = fakeWindow();
  // The delete of feat-a is in flight when an agent window of this repo opens.
  store.hideWorktree("paulocorcino/vibeforge", "feat-a");
  title.ensureListing("paulocorcino/vibeforge");
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.deepEqual(switcherRows(title, win), ["primary", "feat-b"]);
});

test("the checkout button opens a menu with the worktrees of the console's repo", async () => {
  const { store, title } = pageWithStore({
    "paulocorcino/vibeforge": [
      { name: "feat-a", branch: "feat-a", dirty: true },
      { name: "feat-b", branch: "feat-b" },
    ],
    "someone/else": [{ name: "other", branch: "other" }],
  });
  const win = fakeWindow({ _deskCheckout: "feat-a" });
  const bar = fakeElement("span");
  // The store holds the listing of each repo; this console reads its own.
  await store.ensureWorktreeListing("paulocorcino/vibeforge");
  await store.ensureWorktreeListing("someone/else");
  title.renderTitle(win, bar, { checkout: "feat-a" });
  const button = bar.children.find((k) => k.className === "session-checkout");
  assert.ok(button, "an agent window that can relaunch has a checkout button");
  button.fire("click");
  const menu = win.children.find((k) => k.className === "session-checkout-menu");
  assert.ok(menu, "the menu lands on the console window");
  const rows = menu.children.filter((k) => k.className.startsWith("session-checkout-item"));
  const names = rows.map((r) => r.children.find((k) => k.className === "session-checkout-name")?.textContent);
  assert.deepEqual(names, ["primary", "feat-a", "feat-b", undefined]);
  assert.equal(rows[1].className, "session-checkout-item current");
  assert.equal(rows[3].className, "session-checkout-item create");
});

test("the store's notice draws again the titles of the repo whose listing changed", () => {
  const wins = new Set();
  const bars = {};
  for (const [id, repo] of [["w1", "paulocorcino/vibeforge"], ["w2", "someone/else"]]) {
    const bar = fakeElement("span");
    const win = fakeWindow({ _deskId: id, _deskRepo: repo, _title: bar, _presentation: { checkout: "primary" } });
    wins.add(win);
    bars[id] = bar;
  }
  const { deps } = fakeDeps({ wins });
  const title = createTitle(deps);
  title.ingestWorktrees("paulocorcino/vibeforge");
  assert.ok(bars.w1.children.length > 0, "the window of the repo is drawn");
  assert.equal(bars.w2.children.length, 0, "the window of another repo is not");
});

test("tell me when it is done sets the mark", () => {
  // The desk says what is on: the daemon clears the mark when it fires.
  let desk = [{ id: "w1" }];
  const { deps, writes } = fakeDeps({ desk: () => desk });
  const title = createTitle(deps);
  const win = fakeWindow();
  const bar = fakeElement("span");
  title.renderTitle(win, bar, { checkout: "primary" });
  const bell = bar.children.find((k) => k.className?.includes("session-done-mark"));
  assert.ok(bell, "an agent console has the item");
  bell.fire("click");
  assert.deepEqual(writes.at(-1), { win, fields: { doneMark: true } });
  // With the mark on, the same click turns it off.
  desk = [{ id: "w1", doneMark: true }];
  bell.fire("click");
  assert.deepEqual(writes.at(-1).fields, { doneMark: false });
  // A plain shell has no hooks to tell it, and the popup writes nothing.
  for (const [what, deps2] of [
    ["a shell", fakeWindow({ _deskKind: "console" })],
    ["the popup", fakeWindow()],
  ]) {
    const { deps: d } = fakeDeps({ OPTS: what === "the popup" ? { canLaunch: false } : {} });
    const strip = fakeElement("span");
    createTitle(d).renderTitle(deps2, strip, { checkout: "primary" });
    assert.ok(!strip.children.some((k) => k.className?.includes("session-done-mark")), what);
  }
});
