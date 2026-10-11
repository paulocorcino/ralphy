// Unit tests for assets/ui/wb-stage-stack.ts — the z stack and the gestures,
// one of each per document, shared by the console windows and the note cards.
// Driven with a fake workspace, with no browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createStack, createGestures } from "../assets/ui/wb-stage-stack.ts";
import { createConsole } from "../assets/ui/wb-console.ts";

// A surface on the plane: a console window or a note card. `style.zIndex`
// keeps what was written, as the fold reads it back with `parseInt`.
function surface(kind) {
  const classes = new Set([kind]);
  return {
    kind,
    style: { zIndex: "" },
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      contains: (c) => classes.has(c),
    },
  };
}

// A document whose `#workspace` answers the two selectors the stack uses:
// `.a, .b` and `.a.focused, .b.focused`.
function page(surfaces) {
  const matches = (el, part) => part.split(".").filter(Boolean).every((c) => el.classList.contains(c));
  const workspace = {
    querySelectorAll: (sel) => surfaces.filter((el) => sel.split(",").some((p) => matches(el, p.trim()))),
  };
  return {
    readyState: "loading",
    addEventListener() {},
    getElementById: (id) => (id === "workspace" ? workspace : null),
  };
}

const z = (el) => parseInt(el.style.zIndex, 10);

test("a card and a window share one stack: the one focused last is on top", () => {
  const card = surface("note-card");
  const win = surface("session-window");
  const document = page([card, win]);
  const stack = createStack(document);
  // The console the entry module builds with the same stack. Node 22 ships a
  // real `BroadcastChannel`, and an open channel keeps `node --test` alive.
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  let console_;
  try {
    console_ = createConsole({ addEventListener() {} }, document, { protocol: "http:", host: "127.0.0.1:7431" }, { stack });
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
  // The card raises itself through the stack, as `wb-notes.ts` does.
  stack.focusWin(card);
  console_.focusWin(win);
  assert.ok(z(win) > z(card), `the window (${z(win)}) is above the card (${z(card)})`);
  assert.equal(card.classList.contains("focused"), false, "the card lost the focus");
  assert.equal(win.classList.contains("focused"), true);
  // And back: the card focused after the window is above it.
  stack.focusWin(card);
  assert.ok(z(card) > z(win), `the card (${z(card)}) is above the window (${z(win)})`);
});

test("the renormalize at the ceiling keeps the order of windows and cards together", () => {
  const under = surface("session-window");
  const card = surface("note-card");
  const win = surface("session-window");
  const stack = createStack(page([under, card, win]));
  stack.focusWin(under);
  // Far past the ceiling (120 - 60), so the stack renormalizes more than once.
  for (let i = 0; i < 80; i += 1) {
    stack.focusWin(card);
    stack.focusWin(win);
    assert.ok(z(win) > z(card), `round ${i}: the window (${z(win)}) is above the card (${z(card)})`);
  }
  assert.ok(z(card) > z(under), "the window never focused again stays under both");
  for (const el of [under, card, win]) assert.ok(z(el) <= 120, `z ${z(el)} stays under the overlay tier`);
});

test("the focus hook hears each surface that lost the focus, then the focus", () => {
  const card = surface("note-card");
  const win = surface("session-window");
  const stack = createStack(page([card, win]));
  const heard = [];
  stack.setFocusHook({
    blurred: (w) => heard.push(["blurred", w.kind]),
    focused: (w) => heard.push(["focused", w.kind]),
  });
  stack.focusWin(card);
  stack.focusWin(win);
  assert.deepEqual(heard, [
    ["focused", "note-card"],
    ["blurred", "note-card"],
    ["focused", "session-window"],
  ]);
});

test("stackWin gives a surface a z once, without the focus", () => {
  const card = surface("note-card");
  const stack = createStack(page([card]));
  stack.stackWin(card);
  const first = z(card);
  assert.ok(first > 60);
  stack.stackWin(card);
  assert.equal(z(card), first, "a surface that has a z keeps it");
  assert.equal(card.classList.contains("focused"), false);
});

test("a gesture owner tracks each element from begin to end, one element at a time", () => {
  const gestures = createGestures();
  const win = {};
  const fence = {};
  gestures.begin(win);
  gestures.begin(fence);
  assert.equal(gestures.active(win), true);
  assert.equal(gestures.active(fence), true);
  gestures.end(fence);
  assert.equal(gestures.active(fence), false);
  assert.equal(gestures.active(win), true, "ending one gesture leaves the other");
  gestures.end(win);
  assert.equal(gestures.active(win), false);
});
