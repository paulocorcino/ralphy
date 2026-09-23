// Unit tests for assets/ui/wb-notes.js — the note card (ADR-0064 §§2, 8).
//
// The folds only: what a note is CALLED, where it jumps to, what it is saved
// as, and who may move it. The card's DOM is exercised in the browser (the
// slice's Playwright pass), not here — this file loads the module against a
// document that answers nothing, which is exactly the state that proves these
// functions do no DOM work.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-notes.js"), "utf8");
const GEO = readFileSync(join(UI, "wb-geometry.js"), "utf8");

// One window, and `wb-geometry.js` beside it: `lockedBy` asks `fenceOf` which
// fence holds a rect — the SAME fold a console's lock uses, which is the point
// of the assertion below.
function load() {
  const window = { document: { getElementById: () => null } };
  new Function("window", GEO)(window);
  new Function("window", "document", SRC)(window, window.document);
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
  // The closed set, in the order the palette draws it.
  assert.deepEqual(N.TONES, ["ochre", "sage", "rose", "slate", "plum", "sand"]);
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
  // The closed sets, in the order the palette draws them.
  assert.deepEqual(N.FILLS, ["wash", "solid"]);
  assert.deepEqual(N.INKS, ["default", "light", "dark", ...N.TONES]);
  assert.deepEqual(N.FONTS, ["sans", "serif", "mono"]);
  assert.deepEqual(N.SIZES, ["xs", "s", "m", "l", "xl"]);
  assert.equal(N.DEFAULT_FILL, "wash");
  assert.equal(N.DEFAULT_INK, "default");
  assert.equal(N.DEFAULT_FONT, "sans");
  assert.equal(N.DEFAULT_SIZE, "m");
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
  // Asleep and still away: nothing to do.
  assert.equal(N.noteDormancyDecision({ ...base, asleep: true }), "stay");
});

// A stage the module can read: `list()` walks the LIVE cards, because the
// title and the anchors are the card's text and not the desk's. The fake is
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
  new Function("window", GEO)(window);
  new Function("window", "document", SRC)(window, document);
  return window.WBNotes;
}

test("the map is one row per card, with its title, tone, fence and sections", () => {
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
  assert.deepEqual(
    rows[0].anchors.map((a) => [a.index, a.text]),
    [
      [0, "Checklist"],
      [1, "Rollback"],
    ],
  );
  // A card outside every fence, with no heading and a tone from a
  // hand-edited file.
  assert.equal(rows[1].title, "Untitled note");
  assert.equal(rows[1].fence, "");
  assert.equal(rows[1].tone, "sand");
  assert.deepEqual(rows[1].anchors, []);
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
  assert.deepEqual(rows[0].anchors, []);
});

test("a new note is born in the operator's colours, and reading still defaults apart", () => {
  // The two questions the constants answer are NOT the same one. A file that
  // names no `fill:` is a wash, whatever a new card is dressed in — otherwise
  // changing what a new note looks like would repaint every note ever
  // written, because the field they omit is the one being redefined.
  assert.deepEqual(N.NEW_NOTE_STYLE, { tone: "ochre", fill: "solid", ink: "dark" });
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
  assert.match(md, /^---\ncolor: ochre\nfill: solid\nink: dark\n---\nbody\n$/);
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

test("the cheat sheet names every mark the editor actually recognises", () => {
  // The sheet is the only place the marks are readable — the editor dissolves
  // them as they are typed (ADR-0064 §6). Each row was measured against the
  // real bundle; this pins the ones a reader would look for first, including
  // the two that are this card's own (`## ` is an index anchor, and a
  // ```mermaid fence draws).
  const typed = N.MARKDOWN_HELP.map(([t]) => t);
  for (const mark of [
    "**bold**",
    "*italic*",
    "`code`",
    "[ ]",
    "##",
    "```mermaid",
    "/",
    // The three this bundle adds itself — upstream has no input rule for a
    // link at all, so a sheet that named only its marks would be listing
    // things that do not happen.
    "[text](url)",
    "@@path/to/file",
  ]) {
    assert.ok(typed.includes(mark), `the sheet is missing ${mark}`);
  }
  // Two columns, both filled: a row with no explanation is a row that says
  // nothing to the operator who opened this.
  for (const row of N.MARKDOWN_HELP) {
    assert.equal(row.length, 2);
    assert.ok(row[0].length && row[1].length);
    // NO TRAILING SPACE in a chip: it is what fires a block mark and it
    // cannot be seen, so the sheet says so in prose instead.
    assert.equal(row[0], row[0].trim());
  }
});

test("the footer says when a note last landed, and says the day when it was not today", () => {
  const at = new Date(2026, 8, 22, 19, 42).getTime();
  // Same day: the time is the whole answer.
  assert.equal(N.savedLabel(at, new Date(2026, 8, 22, 23, 59).getTime()), "saved 19:42");
  // Another day: "19:42" alone would claim this afternoon.
  assert.equal(N.savedLabel(at, new Date(2026, 8, 23, 0, 1).getTime()), "saved 22/09 19:42");
  assert.equal(N.savedLabel(at, new Date(2027, 8, 22, 19, 42).getTime()), "saved 22/09 19:42");
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
