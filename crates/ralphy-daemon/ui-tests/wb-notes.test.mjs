// Unit tests for assets/ui/wb-notes.ts — the note card's map and its desk rules
// (ADR-0064 §§2, 8). The folds are in wb-notes-folds.test.mjs. The card's DOM is
// exercised in the browser (the slice's Playwright pass), not here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createNotes } from "../assets/ui/wb-notes.ts";

// A stage the module can read: `list()` walks the LIVE cards, because the
// title is the card's text and not the desk's. The fake is
// three properties deep, which is exactly what the fold touches — anything
// more would be testing the DOM instead of the fold.
function withCards(cards, records, fences) {
  const window = {};
  const stub = {
    notes: () => records,
    fenceRecords: () => fences || [],
  };
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => cards } : null),
  };
  window.WBNotes = createNotes(window, document, { console: stub });
  return window.WBNotes;
}

test("the map is one row per card, with its title, tone and fence", () => {
  const records = [
    { id: "a", path: ".ralphy/notes/a.note", rect: { left: 10, top: 10, width: 100, height: 100 } },
    { id: "b", path: "docs/b.note", rect: { left: 500, top: 500, width: 100, height: 100 } },
  ];
  const fences = [
    { id: "f", name: "backend", rect: { left: 0, top: 0, width: 200, height: 200 }, locked: false },
  ];
  const cards = [
    {
      dataset: { noteId: "a" },
      _noteMarkdown: ["# Deploy", "", "## Checklist", "", "## Rollback", ""].join("\n"),
      _noteTone: "plum",
    },
    { dataset: { noteId: "b" }, _noteMarkdown: "no heading here\n", _noteTone: "bogus" },
  ];
  const rows = withCards(cards, records, fences).list();

  assert.equal(rows.length, 2);
  // Desk order, not DOM order: the menu is the desk's list.
  assert.deepEqual(
    rows.map((r) => r.id),
    ["a", "b"],
  );
  assert.equal(rows[0].title, "Deploy");
  assert.equal(rows[0].tone, "plum");
  // The fence the card sits in, by the centre-point rule — the same one the
  // lock is derived from.
  assert.equal(rows[0].fence, "backend");
  // The menu lists notes, not their sections: those are the card's index.
  assert.ok(!("anchors" in rows[0]));
  // A card outside every fence, with no heading and a tone from a
  // hand-edited file.
  assert.equal(rows[1].title, "Untitled note");
  assert.equal(rows[1].fence, "");
  assert.equal(rows[1].tone, "sand");
  assert.equal(rows[1].path, "docs/b.note");
});

test("a card the desk holds but this window does not show is still listed", () => {
  // `list()` reads the record for placement and the CARD for text: a card in
  // a detached popup has no node here, and the row must still name it rather
  // than vanishing from the map.
  const records = [{ id: "away", path: "x.note", rect: { left: 0, top: 0, width: 10, height: 10 } }];
  const rows = withCards([], records, []).list();
  assert.equal(rows.length, 1);
  assert.equal(rows[0].title, "Untitled note");
});

test("the map says which card is on top, and which is in a detached popup", () => {
  const records = [
    { id: "home", path: "a.note", rect: { left: 500, top: 500, width: 100, height: 100 } },
    { id: "away", path: "b.note", rect: { left: 10, top: 10, width: 100, height: 100 } },
  ];
  const fences = [{ id: "f", name: "popup", rect: { left: 0, top: 0, width: 200, height: 200 } }];
  const window = {};
  const stub = {
    notes: () => records,
    fenceRecords: () => fences,
    isDetached: (id) => id === "f",
  };
  // The card IS on this stage, so the refusal below is the popup rule and not
  // a missing node.
  const cards = [{ dataset: { noteId: "away" } }];
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => cards } : null),
  };
  window.WBNotes = createNotes(window, document, { console: stub });
  const rows = window.WBNotes.list();
  assert.deepEqual(
    rows.map((r) => [r.id, r.onTop, r.away]),
    [
      ["home", false, false],
      ["away", false, true],
    ],
  );
  // Nothing is on top in a fresh tab, and a card in the popup is refused.
  assert.equal(window.WBNotes.onTopNow(), null);
  assert.equal(window.WBNotes.keepOnTop("away"), false);
});

// A detach snapshot carries the text of a note that has no file yet (#475):
// the popup opens in the click, before a first save could land.
test("a detach takes the draft of an unnamed dirty note, and nothing else", () => {
  const records = [
    { id: "new", rect: { left: 0, top: 0, width: 10, height: 10 } },
    { id: "clean", rect: { left: 0, top: 0, width: 10, height: 10 } },
    { id: "named", path: "a.note", rect: { left: 0, top: 0, width: 10, height: 10 } },
  ];
  const cards = [
    { dataset: { noteId: "new" }, _noteDirty: true, _noteMarkdown: "typed\n", _noteClaim: ".ralphy/notes/t.note" },
    { dataset: { noteId: "clean" }, _noteDirty: false, _noteMarkdown: "" },
    { dataset: { noteId: "named" }, _noteDirty: true, _noteMarkdown: "saved soon\n" },
  ];
  const notes = withCards(cards, records, []);
  assert.deepEqual(notes.draftOf("new"), { draft: "typed\n", claim: ".ralphy/notes/t.note" });
  // Handed off: from here the popup's card is the only writer.
  assert.equal(cards[0]._noteHandedOff, true);
  // A clean card has nothing to carry, and a named note is written by the
  // teardown flush instead.
  assert.equal(notes.draftOf("clean"), null);
  assert.equal(notes.draftOf("named"), null);
  assert.equal(cards[2]._noteHandedOff, undefined);
  assert.equal(notes.draftOf("gone"), null);
});

test("a card that handed its draft to a popup does not write the note", async () => {
  const writes = [];
  const record = { id: "new", repo: "r", rect: { left: 0, top: 0, width: 10, height: 10 } };
  const cards = [
    {
      dataset: { noteId: "new" },
      _noteDirty: true,
      _noteMarkdown: "typed\n",
      _noteClaim: "t.note",
      classList: { add() {}, remove() {}, contains: () => false },
      querySelector: () => null,
    },
  ];
  const window = {
    WBDaemon: {
      withCheckout: (args) => args,
      write: (verb, args) => {
        writes.push(args.path);
        return Promise.resolve({});
      },
    },
  };
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => cards } : null),
    querySelector: () => null,
  };
  window.WBNotes = createNotes(window, document, {
    console: { notes: () => [record], fenceRecords: () => [], saveNotes() {} },
  });
  const notes = window.WBNotes;
  // The control: without the hand-off the claimed name is written.
  await (notes.flushAll(), cards[0]._noteWrite);
  assert.deepEqual(writes, ["t.note"]);
  // A name already claimed, so the check below is the hand-off and not a
  // probe that never ran.
  cards[0]._noteDirty = true;
  cards[0]._noteClaim = "t.note";
  notes.draftOf("new");
  await (notes.flushAll(), cards[0]._noteWrite);
  assert.deepEqual(writes, ["t.note"]);
});

// ADR-0070 D6: the build reload asks whether a note holds unsaved work. The
// editor's change notification can land after the last keystroke, so the
// question goes to the editor, not to the flag.
test("a keystroke the editor holds but has not reported yet counts as unsaved", () => {
  const card = {
    dataset: { noteId: "n" },
    _noteDirty: false,
    _noteMarkdown: "saved\n",
    _noteEditor: { getMarkdown: () => "saved plus a key\n" },
    querySelector: () => null,
  };
  const window = {};
  const stub = { notes: () => [], fenceRecords: () => [] };
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => [card] } : null),
  };
  window.WBNotes = createNotes(window, document, { console: stub });
  assert.equal(window.WBNotes.anyDirty(), true);
  // NEGATIVE CONTROL: an editor that holds the saved text is clean.
  const clean = { ...card, _noteDirty: false, _noteMarkdown: "saved\n", _noteEditor: { getMarkdown: () => "saved\n" } };
  const w2 = {};
  const d2 = { getElementById: (id) => (id === "stage" ? { querySelectorAll: () => [clean] } : null) };
  w2.WBNotes = createNotes(w2, d2, { console: stub });
  assert.equal(w2.WBNotes.anyDirty(), false);
});

// The cap refuses a new card, it does not evict one, and says why (#623: one
// cap rule for `create` and `openFromExplorer`).
test("a card is refused at the cap, with a toast and no write", () => {
  const records = Array.from({ length: 32 }, (_, i) => ({ id: `n${i}`, repo: "r", path: `${i}.note`, rect: {} }));
  const saved = [];
  const toasts = [];
  const window = {};
  const stub = { notes: () => records, fenceRecords: () => [], saveNotes: (next) => saved.push(next) };
  const document = { getElementById: () => null };
  window.WBNotes = createNotes(window, document, { console: stub, messages: { toast: (t) => toasts.push(t.text) } });
  assert.equal(window.WBNotes.create({ repo: "r" }), null);
  assert.equal(window.WBNotes.openFromExplorer({ repo: "r", path: "new.note" }), null);
  assert.deepEqual(saved, []);
  assert.deepEqual(toasts, Array(2).fill("You can have at most 32 notes. Close one first."));
});

// ---- the stage: what `render` builds and what it drops ------------------------
// A DOM that remembers structure: children, classes and a class selector. It
// is enough for `buildCard`, which `mountDetached` needs to make a window a
// fragment; nothing here asserts on the card's chrome.
function fakeEl(tag) {
  const classes = new Set();
  const el = {
    tag,
    dataset: {},
    style: { setProperty() {}, removeProperty() {} },
    children: [],
    parent: null,
    get className() {
      return [...classes].join(" ");
    },
    set className(v) {
      classes.clear();
      for (const c of String(v).split(/\s+/).filter(Boolean)) classes.add(c);
    },
    classList: {
      add: (...cs) => cs.forEach((c) => classes.add(c)),
      remove: (...cs) => cs.forEach((c) => classes.delete(c)),
      toggle: (c, force) => {
        const on = force === undefined ? !classes.has(c) : !!force;
        if (on) classes.add(c);
        else classes.delete(c);
        return on;
      },
      contains: (c) => classes.has(c),
    },
    get firstChild() {
      return el.children[0] || null;
    },
    get isConnected() {
      return !!el.parent;
    },
    append(...kids) {
      for (const k of kids) {
        k.parent = el;
        el.children.push(k);
      }
    },
    remove() {
      if (el.parent) el.parent.children = el.parent.children.filter((k) => k !== el);
      el.parent = null;
    },
    querySelectorAll(sel) {
      const cls = sel.replace(/^\./, "");
      const out = [];
      const walk = (n) => {
        for (const k of n.children) {
          if (k.classList.contains(cls)) out.push(k);
          walk(k);
        }
      };
      walk(el);
      return out;
    },
    querySelector(sel) {
      return el.querySelectorAll(sel)[0] || null;
    },
    contains: () => false,
    addEventListener() {},
    setAttribute() {},
    focus() {},
  };
  return el;
}

// Members of the host a test replaces, for `onStage` to spread over its own.
let hostOver = {};
function withHost(over, body) {
  hostOver = over;
  try {
    return body();
  } finally {
    hostOver = {};
  }
}

function onStage({ records, WBDaemon = {} }) {
  const stage = fakeEl("stage");
  const window = {
    WBDaemon: {
      withCheckout: (args) => args,
      // A read that never answers: the cards here are about the stage.
      observe: () => new Promise(() => {}),
      ...WBDaemon,
    },
  };
  const document = {
    getElementById: (id) => (id === "stage" ? stage : null),
    createElement: (tag) => fakeEl(tag),
    querySelector: () => null,
    addEventListener() {},
    events: [],
    dispatchEvent(ev) {
      this.events.push([ev.type, ev.detail]);
    },
  };
  const desk = { records };
  const host = {
    notes: () => desk.records,
    fenceRecords: () => [],
    saveNotes() {},
    startResize: () => () => {},
    makeDraggable() {},
    ...hostOver,
  };
  const notes = createNotes(window, document, { console: host });
  const ids = () => stage.querySelectorAll(".note-card").map((c) => c.dataset.noteId);
  return { notes, stage, desk, ids, document };
}

const RECT = { left: 0, top: 0, width: 10, height: 10 };

test("a detached popup does not build a card for a record it was not handed", () => {
  const mine = { id: "mine", repo: "r", path: "a.note", rect: RECT };
  const other = { id: "other", repo: "r", path: "b.note", rect: RECT };
  const { notes, desk, ids } = onStage({ records: [mine, other] });
  notes.mountDetached(mine);
  notes.render();
  assert.deepEqual(ids(), ["mine"]);
  // The control: the shell's window builds every record of its desk.
  const shell = onStage({ records: [mine, other] });
  shell.notes.render();
  assert.deepEqual(shell.ids(), ["mine", "other"]);
  assert.equal(desk.records.length, 2);
});

test("a popup card whose record this desk never held is kept on the stage", () => {
  const snap = { id: "snap", repo: "r", path: "a.note", rect: RECT };
  const { notes, ids } = onStage({ records: [] });
  const card = notes.mountDetached(snap);
  notes.render();
  assert.deepEqual(ids(), ["snap"]);
  assert.notEqual(card._noteGone, true);
});

test("a named note whose record left the desk writes what was typed as its card goes", async () => {
  const writes = [];
  const record = { id: "n", repo: "r", path: "a.note", rect: RECT };
  const { notes, desk, ids } = onStage({
    records: [record],
    WBDaemon: {
      write: (verb, args) => {
        writes.push([verb, args.path, args.markdown]);
        return Promise.resolve({});
      },
    },
  });
  notes.render();
  const card = notes.cardEl("n");
  card._noteMarkdown = "typed\n";
  card._noteDirty = true;
  // Closed from another client: the record is gone, the card follows it.
  desk.records = [];
  notes.render();
  assert.deepEqual(ids(), []);
  assert.equal(card._noteGone, true);
  await card._noteWrite;
  assert.deepEqual(writes, [["note.write", "a.note", "typed\n"]]);
});

test("the card on top is put back before its card leaves the stage", () => {
  const record = { id: "t", repo: "r", path: "a.note", rect: RECT };
  const { notes, desk } = onStage({ records: [record] });
  notes.render();
  const card = notes.cardEl("t");
  const real = globalThis.getComputedStyle;
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => "" });
  try {
    assert.equal(notes.keepOnTop("t"), true);
  } finally {
    globalThis.getComputedStyle = real;
  }
  assert.equal(card.classList.contains("on-top"), true);
  desk.records = [];
  notes.render();
  assert.equal(notes.onTopNow(), null);
  assert.equal(card.classList.contains("on-top"), false);
  assert.equal(card._noteShadow, null);
});

test("in a fence's popup, a card the desk does not hold floats on the record it was handed", () => {
  const record = { id: "o", repo: "r", path: "a.note", rect: RECT };
  const { notes } = onStage({ records: [] });
  notes.mountDetached(record);
  const real = globalThis.getComputedStyle;
  globalThis.getComputedStyle = () => ({ getPropertyValue: () => "" });
  try {
    assert.equal(notes.keepOnTop("o"), true);
  } finally {
    globalThis.getComputedStyle = real;
  }
  assert.equal(notes.cardEl("o").classList.contains("on-top"), true);
});

test("a file that is not a note leaves the stage and opens as bytes", async () => {
  const record = { id: "x", repo: "r", checkout: null, path: "a.note", rect: RECT };
  const saved = [];
  const { notes, desk, document } = withHost({ saveNotes: (next) => saved.push(next) }, () =>
    onStage({ records: [record], WBDaemon: { observe: async () => ({ status: "error", message: "not a note" }) } }),
  );
  desk.records = [record];
  notes.render();
  for (let i = 0; i < 5; i++) await new Promise((r) => setImmediate(r));
  assert.deepEqual(saved.at(-1), []);
  assert.deepEqual(document.events, [["workbench:open-request", { project: "r", path: "a.note", checkout: null, as: "bytes" }]]);
});

test("a note whose file cannot be read stays on the stage and says why beside its path", async () => {
  const record = { id: "m", repo: "r", path: "gone.note", rect: RECT };
  const { notes, document } = onStage({
    records: [record],
    WBDaemon: { observe: async () => ({ status: "error", message: "not found" }) },
  });
  notes.render();
  for (let i = 0; i < 5; i++) await new Promise((r) => setImmediate(r));
  const card = notes.cardEl("m");
  assert.equal(card.classList.contains("missing"), true);
  assert.match(card.querySelector(".note-state").title, /^gone\.note — /);
  assert.deepEqual(document.events, []);
});

test("a render builds no card for a note whose fence is in a popup", () => {
  const home = { id: "home", repo: "r", path: "a.note", rect: { left: 500, top: 500, width: 10, height: 10 } };
  const away = { id: "away", repo: "r", path: "b.note", rect: RECT };
  const fence = { id: "f", name: "popup", rect: { left: 0, top: 0, width: 200, height: 200 } };
  const detached = { fenceRecords: () => [fence], isDetached: (id) => id === "f" };
  const { notes, ids } = withHost(detached, () => onStage({ records: [home, away] }));
  notes.render();
  assert.deepEqual(ids(), ["home"]);
  // The control: with no fence in a popup, the same desk shows both.
  const shell = onStage({ records: [home, away] });
  shell.notes.render();
  assert.deepEqual(shell.ids(), ["home", "away"]);
});
