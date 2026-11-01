// Unit tests for the "new worktree" prompt of assets/ui/wb-console-checkout.ts:
// `createWorktreeFor`, reached from the `New worktree…` row of the checkout
// menu. The switcher is built with fake `deps`, a fake document and a fake
// `WBDaemon`, with no console and no browser. They are here, not in
// wb-console-title.test.mjs, so a change of the title and a change of this
// prompt never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createCheckoutSwitcher } from "../assets/ui/wb-console-checkout.ts";
import { checkoutsStore } from "../assets/ui/wb-checkouts-store.ts";

// An element with the few DOM members the menu and the prompt use. `all`
// records every element the document made, so a test finds the prompt's
// fields by class.
function makeDocument() {
  const all = [];
  const element = (tag) => {
    const listeners = {};
    const el = {
      tagName: tag,
      className: "",
      style: {},
      title: "",
      innerHTML: "",
      textContent: "",
      value: "",
      hidden: false,
      disabled: false,
      children: [],
      listeners,
      setAttribute() {},
      append(...kids) {
        el.children.push(...kids);
      },
      remove() {},
      addEventListener(type, fn) {
        (listeners[type] ||= []).push(fn);
      },
      fire(type) {
        for (const fn of listeners[type] || []) fn({ stopPropagation() {} });
      },
      focus() {},
      select() {},
      setSelectionRange() {},
      contains: () => false,
      getBoundingClientRect: () => ({ left: 0, top: 0, bottom: 10 }),
    };
    all.push(el);
    return el;
  };
  const document = {
    body: element("body"),
    createElement: element,
    addEventListener() {},
    removeEventListener() {},
  };
  return { document, all };
}

// The `New worktree…` row of the menu of a console in `repo`, clicked. The
// daemon answers the reads and writes of `script`, in order; a function in
// the script throws. Every call is recorded as `[verb, args]`.
function openPrompt({ branchList, adds }) {
  const { document, all } = makeDocument();
  const calls = [];
  const answers = { "branch.list": [branchList], "worktree.add": [...adds], "worktree.list": [{ status: "ok", checkouts: null }] };
  const daemon = {
    observe: async (verb, args) => {
      calls.push([verb, args]);
      const next = answers[verb].shift();
      if (next instanceof Error) throw next;
      return next;
    },
  };
  const emitted = [];
  const store = checkoutsStore({ messages: { flash() {} } });
  const window = {
    WBDaemon: daemon,
    Alpine: { store: () => store },
    addEventListener() {},
    dispatchEvent: () => true,
  };
  globalThis.window = window;
  // The shell's `worktree-created` action reads the listing again (wb-wire.ts).
  globalThis.WB = {
    emit: (type, detail) => {
      emitted.push([type, detail]);
      if (type === "worktree-created") void store.ensureWorktreeListing(detail.project, true);
    },
  };
  const relaunched = [];
  const writes = [];
  const switcher = createCheckoutSwitcher({
    window,
    document,
    OPTS: {},
    wins: new Set(),
    lastSessions: () => [],
    agentStateTitle: (state) => state,
    askConfirm: async () => true,
    setWin: (win, fields) => writes.push(fields),
    renderTitle() {},
  });
  const win = {
    ...document.createElement("div"),
    _deskRepo: "o/r",
    _deskCheckout: null,
    classList: { contains: () => false },
    _relaunchIn: (name) => relaunched.push(name),
  };
  switcher.openCheckoutMenu(win, document.createElement("button"));
  const menu = win.children.find((k) => k.className === "session-checkout-menu");
  menu.children.find((k) => k.className === "session-checkout-item create").fire("click");
  return { all, calls, emitted, relaunched, writes };
}

// Every task the prompt needs to build itself has run.
const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

// The fields of the latest prompt the document made.
function promptFields(all) {
  const scrims = all.filter((e) => e.className === "modal-scrim wb-confirm");
  const scrim = scrims.at(-1);
  const under = (el) => [el, ...el.children.flatMap(under)];
  const parts = under(scrim);
  const inputs = parts.filter((e) => e.className === "prompt-input");
  return {
    count: scrims.length,
    name: inputs[0],
    base: inputs[1],
    error: parts.find((e) => e.className === "prompt-error"),
    create: parts.find((e) => e.className === "btn accent"),
  };
}

const ok = { status: "ok", branches: { branches: ["main", "dev"], current: "dev" } };

test("a worktree.add the daemon never answers asks the prompt again, with the error, then moves the console", async () => {
  const t = openPrompt({ branchList: ok, adds: [new Error("socket closed"), { status: "ok", message: "made" }] });
  await settle();
  let prompt = promptFields(t.all);
  assert.equal(prompt.count, 1);
  assert.equal(prompt.error.hidden, true, "the first prompt has no error");
  prompt.name.value = "feat-x";
  prompt.create.fire("click");
  await settle();
  prompt = promptFields(t.all);
  assert.equal(prompt.count, 2, "the prompt is asked again");
  assert.equal(prompt.error.textContent, "Could not reach the daemon. Check whether the worktree was created.");
  assert.equal(prompt.error.hidden, false);
  assert.equal(prompt.name.value, "feat-x", "the name typed stays");
  assert.deepEqual(t.relaunched, [], "nothing moved yet");
  prompt.create.fire("click");
  await settle();
  assert.deepEqual(
    t.calls.filter(([verb]) => verb === "worktree.add"),
    [
      ["worktree.add", { repo: "o/r", name: "feat-x", base: "dev" }],
      ["worktree.add", { repo: "o/r", name: "feat-x", base: "dev" }],
    ],
  );
  assert.deepEqual(
    t.emitted.map(([type]) => type),
    ["worktree-created", "console-switch-checkout"],
    "the shell hears of the worktree before the console moves",
  );
  assert.deepEqual(t.emitted[0][1], { project: "o/r", name: "feat-x", message: "made" });
  assert.deepEqual(t.relaunched, ["feat-x"]);
  assert.deepEqual(t.writes, [{ checkout: "feat-x" }]);
});

test("a worktree created from a console is listed by one read", async () => {
  const t = openPrompt({ branchList: ok, adds: [{ status: "ok", message: "made" }] });
  await settle();
  const prompt = promptFields(t.all);
  prompt.name.value = "feat-x";
  prompt.create.fire("click");
  await settle();
  assert.deepEqual(
    t.calls.filter(([verb]) => verb === "worktree.list"),
    [["worktree.list", { repo: "o/r" }]],
  );
});

test("the prompt starts from the current branch the daemon names, in a list of its branches", async () => {
  const t = openPrompt({ branchList: ok, adds: [] });
  await settle();
  const prompt = promptFields(t.all);
  assert.equal(prompt.base.tagName, "select");
  assert.deepEqual(
    prompt.base.children.map((o) => o.value),
    ["main", "dev"],
  );
  assert.equal(prompt.base.value, "dev");
});

test("a branch list the daemon cannot give leaves a free text field for the base", async () => {
  for (const branchList of [new Error("down"), { status: "failed" }, { status: "ok", branches: { current: "HEAD" } }]) {
    const t = openPrompt({ branchList, adds: [] });
    await settle();
    const prompt = promptFields(t.all);
    assert.equal(prompt.base.tagName, "input", String(branchList));
    assert.equal(prompt.base.value, "", "a detached HEAD is no base to offer");
  }
});
