// Unit tests for assets/ui/wb-console-title.ts — the console name, the title
// and the worktree switcher. `createTitle` is driven with fake `deps` and a
// fake document, with no console and no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createTitle } from "../assets/ui/wb-console-title.ts";

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
    stage: () => null,
    desk: () => [],
    lastSessions: () => [],
    agentStateTitle: (state) => state,
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
    document: { ...fakeDocument(), getElementById: () => ({}) },
    stage: () => ({ querySelectorAll: () => [onStage] }),
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

test("the checkout button opens a menu with the worktrees of the console's repo", () => {
  const { deps } = fakeDeps();
  const title = createTitle(deps);
  const win = fakeWindow({ _deskCheckout: "feat-a" });
  const bar = fakeElement("span");
  title.ingestWorktrees("paulocorcino/vibeforge", {
    worktrees: [
      { name: "feat-a", branch: "feat-a", dirty: true },
      { name: "feat-b", branch: "feat-b" },
    ],
  });
  title.ingestWorktrees("someone/else", { worktrees: [{ name: "other", branch: "other" }] });
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
