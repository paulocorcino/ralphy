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
