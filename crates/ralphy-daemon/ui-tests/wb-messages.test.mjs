// Unit tests for assets/ui/wb-messages.ts — the one door for operator
// messages. `createMessages` is driven with a fake document and a fake shell,
// with no browser: each test checks which drawing the door uses when the page
// has a shell, and when it has none.
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

const fakeWindow = () => ({ setTimeout: () => 1, clearTimeout() {} });

// The buttons of the dialog the door appended last: `[cancel, go]`, or `[go]`.
function buttonsOf(document) {
  const scrim = document.body.children.at(-1);
  const modal = scrim.children[0];
  const foot = modal.children[2];
  return { scrim, buttons: foot.children };
}

test("with a shell, the door asks in the shell's modal and flashes on the shell's line", async () => {
  const asked = [];
  const flashed = [];
  const shell = {
    askConfirm: (opts) => {
      asked.push(opts);
      return Promise.resolve(true);
    },
    flash: (msg) => flashed.push(msg),
  };
  const document = fakeDocument();
  const messages = createMessages(fakeWindow(), document, { shell: () => shell });

  const ask = messages.askInShell({ title: "Delete", message: "Delete x?", confirmLabel: "Delete", danger: true });
  assert.ok(ask, "a page with a shell gets the shell's answer");
  assert.equal(await ask, true);
  assert.deepEqual(asked, [{ title: "Delete", message: "Delete x?", confirmLabel: "Delete", danger: true }]);
  assert.equal(document.body.children.length, 0, "the shell's modal draws it: the door builds no dialog");

  messages.flash("Run started.");
  assert.deepEqual(flashed, ["Run started."]);
});

test("with no shell, the door has no shell modal, a flash shows nothing, and the dialog is the DOM one", async () => {
  const document = fakeDocument();
  const messages = createMessages(fakeWindow(), document, { shell: () => null });

  assert.equal(messages.askInShell({ title: "Delete" }), null, "the caller decides what a page with no shell does");
  messages.flash("Run started.");
  assert.equal(document.body.children.length, 0, "a flash with no shell draws nothing");

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
  const messages = createMessages(fakeWindow(), document, { shell: () => null });
  const first = messages.toast({ text: "Note closed" });
  const second = messages.toast({ text: "Note a.md closed" });
  assert.equal(first.removed, true);
  assert.deepEqual(document.body.children, [second]);
  messages.dismissToast();
  assert.deepEqual(document.body.children, []);
});
