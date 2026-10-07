// Unit tests for assets/ui/wb-notes.ts — the note card (ADR-0064 §§2, 8).
//
// The folds only: what a note is CALLED, where it jumps to, what it is saved
// as, and who may move it. The card's DOM is exercised in the browser (the
// slice's Playwright pass), not here — this file loads the module against a
// document that answers nothing, which is exactly the state that proves these
// functions do no DOM work.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createNotes } from "../assets/ui/wb-notes.ts";

// One window. `lockedBy` asks `WBGeometry.fenceOf`, which the notes module
// imports, which fence holds a rect — the SAME fold a console's lock
// uses, which is the point of the assertion below.
function load() {
  const window = { document: { getElementById: () => null } };
  window.WBNotes = createNotes(window, window.document);
  return window.WBNotes;
}

const N = load();

test("a note is titled by its front matter, and a legacy heading still names one", () => {
  assert.equal(N.titleOf('---\ntitle: "Standup"\ncolor: sage\n---\nbody\n'), "Standup");
  // A colon and a quote survive the quoted scalar they are written as.
  const tricky = N.withTitle("body\n", 'Sprint 12: "what is left"');
  assert.equal(N.titleOf(tricky), 'Sprint 12: "what is left"');
  assert.equal(N.bodyOf(tricky), "body\n");
  // LEGACY — every note written before the amendment carries its name as the
  // body's first heading, and opens under it.
  assert.equal(N.titleOf("# Standup\n\nbody\n"), "Standup");
  assert.equal(N.titleOf("---\ncolor: sage\n---\n# Groceries\n"), "Groceries");
  // The FIRST LINE only: a `#` further down is a section the operator wrote.
  assert.equal(N.titleOf("some preamble\n\n# Real title\n"), "Untitled note");
  assert.equal(N.titleOf("## Section\n\nbody\n"), "Untitled note");
  // The field WINS over a heading, so a migrated note never reads the old one.
  assert.equal(N.titleOf('---\ntitle: "New"\ncolor: sand\n---\n# Old\n'), "New");
  assert.equal(N.titleOf(""), "Untitled note");
  assert.equal(N.titleOf("", null), null);
});

test("the anchors are the `##` headings, in document order", () => {
  const md = "# Title\n\n## First\n\ntext\n\n### Deeper\n\n## Second\n";
  assert.deepEqual(N.anchorsOf(md), [
    { text: "First", index: 0 },
    { text: "Second", index: 1 },
  ]);
  // NEGATIVE CONTROL: a `##` inside a code fence is source, not a heading —
  // otherwise a markdown snippet in a note fills the jump menu with noise.
  const fenced = "## Real\n\n```md\n## Not a heading\n```\n\n## Also real\n";
  assert.deepEqual(
    N.anchorsOf(fenced).map((a) => a.text),
    ["Real", "Also real"],
  );
  assert.deepEqual(N.anchorsOf(""), []);
});

test("the filename comes from the title, and says when it cannot", () => {
  assert.equal(N.noteSlug("Standup notes"), "standup-notes");
  assert.equal(N.noteSlug("Reunião de terça"), "reuniao-de-terca");
  assert.equal(N.noteSlug("  Spaces  &  symbols!! "), "spaces-symbols");
  assert.equal(N.noteSlug("a".repeat(80)).length, 48);
  // Empty is the caller's signal to stamp instead — never a bare `-.note`.
  assert.equal(N.noteSlug("日本語"), "");
  assert.equal(N.noteSlug("!!!"), "");
  assert.equal(N.noteSlug(""), "");
});

test("a tone outside the closed set is sand", () => {
  for (const tone of N.TONES) assert.equal(N.toneOf(tone), tone);
  assert.equal(N.toneOf("chartreuse"), "sand");
  assert.equal(N.toneOf(undefined), "sand");
  assert.equal(N.DEFAULT_TONE, "sand");
  assert.ok(N.TONES.includes(N.DEFAULT_TONE), "the fallback is a tone the palette draws");
});

test("a card is locked by its own record or by the fence holding it", () => {
  const fences = [
    { id: "f-open", rect: { left: 0, top: 0, width: 200, height: 200 }, locked: false },
    { id: "f-held", rect: { left: 200, top: 0, width: 200, height: 200 }, locked: true },
  ];
  const inOpen = { rect: { left: 10, top: 10, width: 100, height: 100 } };
  const inHeld = { rect: { left: 210, top: 10, width: 100, height: 100 } };
  assert.equal(N.lockedBy(inOpen, fences), null);
  assert.equal(N.lockedBy({ ...inOpen, locked: true }, fences), "self");
  assert.equal(N.lockedBy(inHeld, fences), "fence");
  // Outside every fence, and with no fences at all.
  assert.equal(N.lockedBy({ rect: { left: 900, top: 900, width: 10, height: 10 } }, fences), null);
  assert.equal(N.lockedBy(inHeld, []), null);
});

test("a new card lands in the middle of what the operator is looking at", () => {
  const viewport = { width: 1000, height: 800 };
  const first = N.spawnRect(viewport, { left: 0, top: 0 }, 0);
  assert.equal(first.width, N.NOTE_DEFAULT.width);
  assert.equal(first.height, N.NOTE_DEFAULT.height);
  assert.equal(first.left, Math.round(500 - N.NOTE_DEFAULT.width / 2));
  assert.equal(first.top, Math.round(400 - N.NOTE_DEFAULT.height / 2));
  // A second card is offset, so two in a row do not stack perfectly.
  const second = N.spawnRect(viewport, { left: 0, top: 0 }, 1);
  assert.notEqual(second.left, first.left);
  // The scroll offset is the plane's origin, and nothing lands off it.
  const panned = N.spawnRect(viewport, { left: 4000, top: 2000 }, 0);
  assert.ok(panned.left > first.left && panned.top > first.top);
  assert.ok(N.spawnRect({ width: 10, height: 10 }, { left: 0, top: 0 }, 0).left >= 0);
});

// The front matter is the shell's alone — the daemon's `.note` codec carries
// bytes and reads no field (`src/note.rs`). What must hold is that ONE shape
// comes out for one look, or an autosave after a read rewrites the header for
// nothing, and every read-modify-write becomes a diff.
test("the front-matter block has exactly one shape per look", () => {
  const plain = { tone: "plum", fill: "wash", ink: "default" };
  assert.equal(N.withStyle("# Title\n\nbody\n", plain), "---\ncolor: plum\n---\n# Title\n\nbody\n");
  // Replacing, not stacking.
  const once = N.withStyle("# Title\n\nbody\n", plain);
  assert.equal(
    N.withStyle(once, { ...plain, tone: "sage" }),
    "---\ncolor: sage\n---\n# Title\n\nbody\n",
  );
  assert.equal(N.colorOf(once), "plum");
  assert.equal(N.bodyOf(once), "# Title\n\nbody\n");
  // The DEFAULTS are omitted: a note nobody restyled keeps the single
  // `color:` line it has always had, so this change rewrites no existing file.
  assert.equal(N.withStyle("x\n", { tone: "sand" }), "---\ncolor: sand\n---\nx\n");
  // An unknown name in any of the three falls back, never lands in the file.
  assert.equal(
    N.withStyle("x\n", { tone: "chartreuse", fill: "glossy", ink: "neon" }),
    "---\ncolor: sand\n---\nx\n",
  );
  // A full look, in the declared field order.
  assert.equal(
    N.withStyle("x\n", { tone: "ochre", fill: "solid", ink: "light" }),
    "---\ncolor: ochre\nfill: solid\nink: light\n---\nx\n",
  );
});

test("a look survives the round trip, and a hand-edited one falls back", () => {
  const look = { tone: "ochre", fill: "solid", ink: "light", font: "serif", size: "l" };
  assert.deepEqual(N.styleOf(N.withStyle("x\n", look)), look);
  // No front matter at all, and a block naming nothing this shell knows.
  assert.deepEqual(N.styleOf("# T\n"), {
    tone: "sand",
    fill: "wash",
    ink: "default",
    font: "sans",
    size: "m",
  });
  assert.deepEqual(N.styleOf("---\ncolor: sage\nfill: glossy\nfont: comic\nsize: 42\n---\nx\n"), {
    tone: "sage",
    fill: "wash",
    ink: "default",
    font: "sans",
    size: "m",
  });
  // Every name in a closed set survives the round trip; the defaults are in
  // their sets. Which names the palette offers, and in what order, is its data.
  for (const [set, def] of [
    [N.FILLS, N.DEFAULT_FILL],
    [N.INKS, N.DEFAULT_INK],
    [N.FONTS, N.DEFAULT_FONT],
    [N.SIZES, N.DEFAULT_SIZE],
  ]) {
    assert.ok(set.includes(def), `${def} in ${set}`);
  }
  for (const tone of N.TONES) assert.ok(N.INKS.includes(tone), `ink ${tone}`);
  for (const font of N.FONTS) assert.equal(N.fontOf(font), font);
  for (const size of N.SIZES) assert.equal(N.sizeOf(size), size);
  // A hand-edited name falls back rather than reaching the stylesheet, where
  // it would land as a `data-font` no rule matches — a card with no face.
  assert.equal(N.fontOf("Comic Sans"), "sans");
  assert.equal(N.sizeOf("14px"), "m");
  assert.equal(N.fontOf("mono"), "mono");
  assert.equal(N.sizeOf("xl"), "xl");
});

// Renaming a note writes the header and leaves the document alone — and it is
// the one door a legacy note's heading comes up through.
test("the title is renamed in the header, and the look rides along", () => {
  const md = N.withStyle("body\n", { tone: "plum", ink: "light" });
  const next = N.withTitle(md, "New");
  assert.equal(N.titleOf(next), "New");
  assert.equal(N.bodyOf(next), "body\n");
  assert.deepEqual(N.styleOf(next), {
    tone: "plum",
    fill: "wash",
    ink: "light",
    font: "sans",
    size: "m",
  });
  // MIGRATION: the legacy heading is lifted out of the body, with the blank
  // line that followed it — the name is no longer printed twice.
  const migrated = N.withTitle("# Old\n\nbody\n", "New");
  assert.equal(N.titleOf(migrated), "New");
  assert.equal(N.bodyOf(migrated), "body\n");
  // A `#` that is NOT the body's first line stays where the operator put it.
  assert.equal(N.bodyOf(N.withTitle("preamble\n\n# Section\n", "New")), "preamble\n\n# Section\n");
  // An empty name UNTITLES: the field goes rather than being written empty.
  assert.equal(N.titleOf(N.withTitle(migrated, "  ")), "Untitled note");
  assert.equal(N.titleFieldOf(N.withTitle(migrated, "")), null);
  // A newline pasted into the field is a name, not a second block.
  assert.equal(N.titleOf(N.withTitle("body\n", "One\nTwo")), "One Two");
});

test("a document without recognised front matter has no colour", () => {
  assert.equal(N.colorOf(""), null);
  assert.equal(N.colorOf("# Title\n"), null);
  // An unterminated fence is not front matter, and a fence that is not the
  // first line is a horizontal rule.
  assert.equal(N.colorOf("---\ncolor: sage\n"), null);
  assert.equal(N.colorOf("# Title\n---\ncolor: sage\n---\n"), null);
  assert.equal(N.colorOf("---\ncolor: chartreuse\n---\n"), null);
  assert.equal(N.bodyOf("# Title\n"), "# Title\n");
  // CRLF, from an editor that touched the file.
  assert.equal(N.colorOf("---\r\ncolor: rose\r\n---\r\n# T\r\n"), "rose");
  assert.equal(N.bodyOf("---\r\ncolor: rose\r\n---\r\n# T\r\n"), "# T\r\n");
});

test("a note with no heading is named by a UTC stamp", () => {
  assert.equal(N.stampName(new Date(Date.UTC(2026, 8, 22, 7, 5, 3))), "note-20260922-070503");
  assert.match(N.stampName(), /^note-\d{8}-\d{6}$/);
});

test("a card with unsaved text never sleeps", () => {
  const base = { visible: false, dirty: false, inFlight: false, asleep: false, elapsed: 99e3, after: 15e3 };
  assert.equal(N.noteDormancyDecision(base), "sleep");
  // The two refusals are the point: tearing the editor down is what would lose
  // the text, so neither state may ever answer "sleep".
  assert.equal(N.noteDormancyDecision({ ...base, dirty: true }), "stay");
  assert.equal(N.noteDormancyDecision({ ...base, inFlight: true }), "stay");
  // Not yet off-screen for long enough.
  assert.equal(N.noteDormancyDecision({ ...base, elapsed: 1e3 }), "stay");
  // Visible: wake if asleep, and never sleep.
  assert.equal(N.noteDormancyDecision({ ...base, visible: true }), "stay");
  assert.equal(N.noteDormancyDecision({ ...base, visible: true, asleep: true }), "wake");
  // On top: the observer cannot see a fixed card, so the fold must not sleep it.
  assert.equal(N.noteDormancyDecision({ ...base, onTop: true }), "stay");
  assert.equal(N.noteDormancyDecision({ ...base, onTop: true, asleep: true }), "wake");
  // Asleep and still away: nothing to do.
  assert.equal(N.noteDormancyDecision({ ...base, asleep: true }), "stay");
});

// A stage the module can read: `list()` walks the LIVE cards, because the
// title is the card's text and not the desk's. The fake is
// three properties deep, which is exactly what the fold touches — anything
// more would be testing the DOM instead of the fold.
function withCards(cards, records, fences) {
  const window = {
    WBConsole: {
      notes: () => records,
      fenceRecords: () => fences || [],
    },
  };
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => cards } : null),
  };
  window.WBNotes = createNotes(window, document);
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

test("a new note is born in the operator's colours, and reading still defaults apart", () => {
  // The two questions the constants answer are NOT the same one. A file that
  // names no `fill:` is a wash, whatever a new card is dressed in — otherwise
  // changing what a new note looks like would repaint every note ever
  // written, because the field they omit is the one being redefined.
  const { tone, fill, ink } = N.NEW_NOTE_STYLE;
  assert.deepEqual(Object.keys(N.NEW_NOTE_STYLE).toSorted(), ["fill", "ink", "tone"]);
  assert.notEqual(fill, N.DEFAULT_FILL, "a new card is not dressed in the reading default");
  assert.deepEqual(N.styleOf("---\ncolor: sage\n---\nbody\n"), {
    tone: "sage",
    fill: N.DEFAULT_FILL,
    ink: N.DEFAULT_INK,
    font: N.DEFAULT_FONT,
    size: N.DEFAULT_SIZE,
  });
  // And a card dressed in them writes all three out, because two of them are
  // no longer what an absent field means.
  const md = N.withStyle("body\n", N.NEW_NOTE_STYLE);
  assert.equal(md, `---\ncolor: ${tone}\nfill: ${fill}\nink: ${ink}\n---\nbody\n`);
  // A new note takes the operator's COLOURS and the reading defaults for the
  // hand and the size — which is why `NEW_NOTE_STYLE` names three fields and
  // not five, and why neither `font:` nor `size:` is in the file above.
  assert.deepEqual(N.styleOf(md), {
    ...N.NEW_NOTE_STYLE,
    font: N.DEFAULT_FONT,
    size: N.DEFAULT_SIZE,
  });
});

// The hand and the size are fields of the look like any other, which means the
// one shape rule holds for them too: a default is OMITTED, so turning them on
// rewrites no file that never used them.
test("the hand and the size are written only when they are not the default", () => {
  assert.equal(N.withStyle("x\n", { tone: "sage" }), "---\ncolor: sage\n---\nx\n");
  assert.equal(
    N.withStyle("x\n", { tone: "sage", font: "sans", size: "m" }),
    "---\ncolor: sage\n---\nx\n",
  );
  // In the declared order, after the colour they refine.
  assert.equal(
    N.withStyle("x\n", { tone: "sage", font: "mono", size: "xl" }),
    "---\ncolor: sage\nfont: mono\nsize: xl\n---\nx\n",
  );
  // Replacing, never stacking — the same rule the tone's own field follows.
  const once = N.withStyle("x\n", { tone: "sage", font: "mono" });
  assert.equal(N.withStyle(once, { tone: "sage", font: "serif" }), "---\ncolor: sage\nfont: serif\n---\nx\n");
  // And a hand or a size rides through a RETITLE, like every other field.
  const named = N.withTitle(N.withStyle("body\n", { tone: "plum", size: "l" }), "Named");
  assert.equal(N.styleOf(named).size, "l");
  assert.equal(N.titleOf(named), "Named");
});

test("the footer says when a note last landed, and says the day when it was not today", () => {
  const at = new Date(2026, 8, 22, 19, 42).getTime();
  // Same day: the time is the whole answer.
  assert.equal(N.savedLabel(at, new Date(2026, 8, 22, 23, 59).getTime()), "Saved 19:42");
  // Another day: "19:42" alone would claim this afternoon.
  assert.equal(N.savedLabel(at, new Date(2026, 8, 23, 0, 1).getTime()), "Saved 22/09 19:42");
  assert.equal(N.savedLabel(at, new Date(2027, 8, 22, 19, 42).getTime()), "Saved 22/09 19:42");
  // A note no save has landed for claims no time at all.
  assert.equal(N.savedLabel(null, Date.now()), "");
  assert.equal(N.savedLabel(0, Date.now()), "");
});

test("a note carries whether it opens veiled, and every writer carries it along", () => {
  const plain = '---\ntitle: "Keys"\ncolor: ochre\n---\nsecret\n';
  assert.equal(N.veiledOf(plain), false);
  const hidden = N.withVeil(plain, true);
  assert.equal(N.veiledOf(hidden), true);
  assert.match(hidden, /^---\ntitle: "Keys"\ncolor: ochre\nhidden: true\n---\nsecret\n$/);
  // `hidden: false` is what every note already is; writing it would put a line
  // in every file to say nothing.
  assert.equal(N.withVeil(hidden, false), plain);
  // The two OTHER writers must carry it, or a recolour or a rename would
  // quietly un-hide the note.
  assert.equal(N.veiledOf(N.withStyle(hidden, { tone: "plum", fill: "solid", ink: "dark" })), true);
  assert.equal(N.veiledOf(N.withTitle(hidden, "Passwords")), true);
  assert.equal(N.titleOf(N.withTitle(hidden, "Passwords")), "Passwords");
  // And the body survives both, which is the thing the veil must never cost.
  assert.equal(N.bodyOf(N.withTitle(hidden, "Passwords")), "secret\n");
  // A hand-written `hidden:` that is not a boolean names nothing in the set.
  assert.equal(N.veiledOf('---\ncolor: sand\nhidden: sometimes\n---\nx\n'), false);
});

// A card on top (ADR-0064, 2026-09-26 amendment §§2, 8).
test("a card on top floats in the top-right corner, between its floor and its ceiling", () => {
  const vp = { width: 1600, height: 1000 };
  // A small card grows to the floor: 240×180 is too small to edit over a console.
  assert.deepEqual(N.onTopRect({ width: 240, height: 180 }, vp), {
    band: false,
    left: 1600 - 420 - 12,
    // Below a maximized console's title bar, whose buttons share the corner.
    top: N.ON_TOP_TOP,
    width: 420,
    height: 320,
  });
  // A big card shrinks to the ceiling: half the width, 80 % of the height.
  const big = N.onTopRect({ width: 1400, height: 950 }, vp);
  assert.equal(big.width, 800);
  assert.equal(big.height, 800);
  assert.equal(big.left + big.width, 1600 - 12);
  // A card between the two keeps its own size.
  const mid = N.onTopRect({ width: 500, height: 400 }, vp);
  assert.deepEqual([mid.width, mid.height], [500, 400]);
  // A short viewport: the height ceiling wins over the floor, and the card
  // never runs past the bottom.
  assert.equal(N.onTopRect({ width: 240, height: 180 }, { width: 1600, height: 300 }).height, 240);
  const short = N.onTopRect({ width: 240, height: 180 }, { width: 1600, height: 200 });
  assert.equal(short.height, 200 - N.ON_TOP_TOP - 12);
});

test("below the band width the card on top is a band, because half the width is under the floor", () => {
  const edge = N.ON_TOP_BAND_BELOW;
  assert.deepEqual(N.onTopRect({ width: 240, height: 180 }, { width: edge - 1, height: 800 }), {
    band: true,
  });
  assert.equal(N.onTopRect({ width: 240, height: 180 }, { width: edge, height: 800 }).band, false);
  // A phone: the band, whatever the card's size.
  assert.equal(N.onTopRect({ width: 2000, height: 2000 }, { width: 390, height: 800 }).band, true);
});

test("a floating box is kept inside the viewport after a drag or a resize of the window", () => {
  const box = { left: 900, top: -40, width: 500, height: 400 };
  // Pulled back in on both axes, size kept.
  assert.deepEqual(N.onTopClamp(box, { width: 1200, height: 900 }), {
    band: false,
    left: 700,
    top: 0,
    width: 500,
    height: 400,
  });
  // A viewport smaller than the box: the size shrinks first.
  const small = N.onTopClamp({ left: 0, top: 0, width: 1000, height: 1000 }, { width: 900, height: 600 });
  assert.deepEqual([small.width, small.height, small.left, small.top], [900, 600, 0, 0]);
  // Narrow again: back to the band.
  assert.deepEqual(N.onTopClamp(box, { width: 600, height: 900 }), { band: true });
});

test("the map says which card is on top, and which is in a detached popup", () => {
  const records = [
    { id: "home", path: "a.note", rect: { left: 500, top: 500, width: 100, height: 100 } },
    { id: "away", path: "b.note", rect: { left: 10, top: 10, width: 100, height: 100 } },
  ];
  const fences = [{ id: "f", name: "popup", rect: { left: 0, top: 0, width: 200, height: 200 } }];
  const window = {
    WBConsole: {
      notes: () => records,
      fenceRecords: () => fences,
      isDetached: (id) => id === "f",
    },
  };
  // The card IS on this stage, so the refusal below is the popup rule and not
  // a missing node.
  const cards = [{ dataset: { noteId: "away" } }];
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => cards } : null),
  };
  window.WBNotes = createNotes(window, document);
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
    WBConsole: { notes: () => [record], fenceRecords: () => [], saveNotes() {} },
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
  window.WBNotes = createNotes(window, document);
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
  const window = { WBConsole: { notes: () => [], fenceRecords: () => [] } };
  const document = {
    getElementById: (id) => (id === "stage" ? { querySelectorAll: () => [card] } : null),
  };
  window.WBNotes = createNotes(window, document);
  assert.equal(window.WBNotes.anyDirty(), true);
  // NEGATIVE CONTROL: an editor that holds the saved text is clean.
  const clean = { ...card, _noteDirty: false, _noteMarkdown: "saved\n", _noteEditor: { getMarkdown: () => "saved\n" } };
  const w2 = { WBConsole: { notes: () => [], fenceRecords: () => [] } };
  const d2 = { getElementById: (id) => (id === "stage" ? { querySelectorAll: () => [clean] } : null) };
  w2.WBNotes = createNotes(w2, d2);
  assert.equal(w2.WBNotes.anyDirty(), false);
});
