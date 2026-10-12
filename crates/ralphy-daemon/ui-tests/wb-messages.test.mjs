// Unit tests for assets/ui/wb-messages.ts — the one door for operator
// messages. `createMessages` is driven with a fake document and a fake Alpine
// store `modals`, with no browser: each test checks which drawing the door
// uses when the page has the store, and when it has none.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createMessages } from "../assets/ui/wb-messages.ts";

// An element with the few DOM members the dialog and the toast use. `click`
// runs the element's click listeners.
function fakeElement(tag) {
  const listeners = {};
  const el = {
    tag,
    className: "",
    textContent: "",
    style: {},
    children: [],
    attrs: {},
    removed: false,
    setAttribute(k, v) {
      el.attrs[k] = v;
    },
    append(...kids) {
      el.children.push(...kids);
    },
    addEventListener(type, fn) {
      (listeners[type] ||= []).push(fn);
    },
    click() {
      for (const fn of listeners.click || []) fn();
    },
    focus() {
      doc.activeElement = el;
    },
    remove() {
      el.removed = true;
      doc.body.children = doc.body.children.filter((c) => c !== el);
    },
  };
  return el;
}

// The page's document: a body that holds what the door appends, and the
// keydown listeners a test fires.
let doc;
function fakeDocument() {
  const keydown = new Set();
  doc = {
    activeElement: null,
    body: {
      children: [],
      append(...kids) {
        this.children.push(...kids);
      },
    },
    createElement: (tag) => fakeElement(tag),
    addEventListener: (type, fn) => type === "keydown" && keydown.add(fn),
    removeEventListener: (type, fn) => type === "keydown" && keydown.delete(fn),
    key(key) {
      for (const fn of [...keydown]) fn({ key, stopPropagation() {} });
    },
  };
  return doc;
}

// The page's window. `modals` is the Alpine store the page registered, or
// absent for a page with no Alpine.
const fakeWindow = (modals) => ({
  setTimeout: () => 1,
  clearTimeout() {},
  ...(modals ? { Alpine: { store: (name) => (name === "modals" ? modals : undefined) } } : {}),
});

// The buttons of the dialog the door appended last: `[cancel, go]`, or `[go]`.
function buttonsOf(document) {
  const scrim = document.body.children.at(-1);
  const modal = scrim.children[0];
  const foot = modal.children[2];
  return { scrim, buttons: foot.children };
}

test("with the store modals, the door asks in its modal and flashes on its line", async () => {
  const asked = [];
  const flashed = [];
  const modals = {
    askConfirm: (opts) => {
      asked.push(opts);
      return Promise.resolve(true);
    },
    flash: (msg) => flashed.push(msg),
  };
  const document = fakeDocument();
  const messages = createMessages(fakeWindow(modals), document);

  const ask = messages.askInShell({ title: "Delete", message: "Delete x?", confirmLabel: "Delete", danger: true });
  assert.ok(ask, "a page with the store gets the store's answer");
  assert.equal(await ask, true);
  assert.deepEqual(asked, [{ title: "Delete", message: "Delete x?", confirmLabel: "Delete", danger: true }]);
  assert.equal(document.body.children.length, 0, "the store's modal draws it: the door builds no dialog");

  messages.flash("Run started.");
  assert.deepEqual(flashed, ["Run started."]);
});

test("with no Alpine, the door has no Alpine modal, a flash shows nothing, and the dialog is the DOM one", async () => {
  const document = fakeDocument();
  const messages = createMessages(fakeWindow(), document);

  assert.equal(messages.askInShell({ title: "Delete" }), null, "the caller decides what a page with no Alpine does");
  messages.flash("Run started.");
  assert.equal(document.body.children.length, 0, "a flash with no Alpine draws nothing");

  const yes = messages.askConfirm({ title: "Close this console?", message: "The session ends." });
  const { scrim, buttons } = buttonsOf(document);
  assert.equal(scrim.className, "modal-scrim wb-confirm", "the dialog wears the shell's modal classes");
  assert.equal(buttons.length, 2);
  assert.equal(document.activeElement, buttons[0], "Cancel takes the keyboard");
  buttons[1].click();
  assert.equal(await yes, true);
  assert.equal(document.body.children.length, 0, "the answer takes the dialog away");

  const no = messages.askConfirm({ title: "Close this console?", message: "The session ends." });
  document.key("Escape");
  assert.equal(await no, false, "Escape is a no");

  const ok = messages.askNotice({ title: "Could not delete", message: "The daemon refused." });
  const notice = buttonsOf(document);
  assert.equal(notice.buttons.length, 1, "a notice has OK alone");
  assert.equal(notice.buttons[0].textContent, "OK");
  assert.equal(document.activeElement, notice.buttons[0], "OK takes the keyboard");
  document.key("Enter");
  assert.equal(await ok, true);
});

test("a second toast replaces the first", () => {
  const document = fakeDocument();
  const messages = createMessages(fakeWindow(), document);
  const first = messages.toast({ text: "Note closed" });
  const second = messages.toast({ text: "Note a.md closed" });
  assert.equal(first.removed, true);
  assert.deepEqual(document.body.children, [second]);
  messages.dismissToast();
  assert.deepEqual(document.body.children, []);
});
