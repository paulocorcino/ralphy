// Unit tests for assets/ui/wb-notes-folds.ts — the note's constants, folds and
// front matter (ADR-0064 §§2, 4, 8, 14).
//
// The folds only: what a note is CALLED, where it jumps to, what it is saved
// as, who may move it, and when it sleeps. The module is loaded on its own, so
// these tests need no document: a fold that reached for the DOM would fail here.
import { test } from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_FILL, DEFAULT_FONT, DEFAULT_INK, DEFAULT_SIZE, DEFAULT_TONE, FILLS, FONTS, INKS, NEW_NOTE_STYLE, NOTE_DEFAULT, ON_TOP_BAND_BELOW, ON_TOP_TOP, SIZES, TONES, anchorsOf, bodyOf, colorOf, fontOf, lockedBy, noteDormancyDecision, noteSlug, onTopClamp, onTopRect, savedLabel, sizeOf, spawnRect, stampName, styleOf, titleFieldOf, titleOf, toneOf, veiledOf, withStyle, withTitle, withVeil } from "../assets/ui/wb-notes-folds.ts";

test("a note is titled by its front matter, and a legacy heading still names one", () => {
  assert.equal(titleOf('---\ntitle: "Standup"\ncolor: sage\n---\nbody\n'), "Standup");
  // A colon and a quote survive the quoted scalar they are written as.
  const tricky = withTitle("body\n", 'Sprint 12: "what is left"');
  assert.equal(titleOf(tricky), 'Sprint 12: "what is left"');
  assert.equal(bodyOf(tricky), "body\n");
  // LEGACY — every note written before the amendment carries its name as the
  // body's first heading, and opens under it.
  assert.equal(titleOf("# Standup\n\nbody\n"), "Standup");
  assert.equal(titleOf("---\ncolor: sage\n---\n# Groceries\n"), "Groceries");
  // The FIRST LINE only: a `#` further down is a section the operator wrote.
  assert.equal(titleOf("some preamble\n\n# Real title\n"), "Untitled note");
  assert.equal(titleOf("## Section\n\nbody\n"), "Untitled note");
  // The field WINS over a heading, so a migrated note never reads the old one.
  assert.equal(titleOf('---\ntitle: "New"\ncolor: sand\n---\n# Old\n'), "New");
  assert.equal(titleOf(""), "Untitled note");
  assert.equal(titleOf("", null), null);
});

test("the anchors are the `##` headings, in document order", () => {
  const md = "# Title\n\n## First\n\ntext\n\n### Deeper\n\n## Second\n";
  assert.deepEqual(anchorsOf(md), [
    { text: "First", index: 0 },
    { text: "Second", index: 1 },
  ]);
  // NEGATIVE CONTROL: a `##` inside a code fence is source, not a heading —
  // otherwise a markdown snippet in a note fills the jump menu with noise.
  const fenced = "## Real\n\n```md\n## Not a heading\n```\n\n## Also real\n";
  assert.deepEqual(
    anchorsOf(fenced).map((a) => a.text),
    ["Real", "Also real"],
  );
  assert.deepEqual(anchorsOf(""), []);
});

test("the filename comes from the title, and says when it cannot", () => {
  assert.equal(noteSlug("Standup notes"), "standup-notes");
  assert.equal(noteSlug("Reunião de terça"), "reuniao-de-terca");
  assert.equal(noteSlug("  Spaces  &  symbols!! "), "spaces-symbols");
  assert.equal(noteSlug("a".repeat(80)).length, 48);
  // Empty is the caller's signal to stamp instead — never a bare `-.note`.
  assert.equal(noteSlug("日本語"), "");
  assert.equal(noteSlug("!!!"), "");
  assert.equal(noteSlug(""), "");
});

test("a tone outside the closed set is sand", () => {
  for (const tone of TONES) assert.equal(toneOf(tone), tone);
  assert.equal(toneOf("chartreuse"), "sand");
  assert.equal(toneOf(undefined), "sand");
  assert.equal(DEFAULT_TONE, "sand");
  assert.ok(TONES.includes(DEFAULT_TONE), "the fallback is a tone the palette draws");
});

test("a card is locked by its own record or by the fence holding it", () => {
  const fences = [
    { id: "f-open", rect: { left: 0, top: 0, width: 200, height: 200 }, locked: false },
    { id: "f-held", rect: { left: 200, top: 0, width: 200, height: 200 }, locked: true },
  ];
  const inOpen = { rect: { left: 10, top: 10, width: 100, height: 100 } };
  const inHeld = { rect: { left: 210, top: 10, width: 100, height: 100 } };
  assert.equal(lockedBy(inOpen, fences), null);
  assert.equal(lockedBy({ ...inOpen, locked: true }, fences), "self");
  assert.equal(lockedBy(inHeld, fences), "fence");
  // Outside every fence, and with no fences at all.
  assert.equal(lockedBy({ rect: { left: 900, top: 900, width: 10, height: 10 } }, fences), null);
  assert.equal(lockedBy(inHeld, []), null);
});

test("a new card lands in the middle of what the operator is looking at", () => {
  const viewport = { width: 1000, height: 800 };
  const first = spawnRect(viewport, { left: 0, top: 0 }, 0);
  assert.equal(first.width, NOTE_DEFAULT.width);
  assert.equal(first.height, NOTE_DEFAULT.height);
  assert.equal(first.left, Math.round(500 - NOTE_DEFAULT.width / 2));
  assert.equal(first.top, Math.round(400 - NOTE_DEFAULT.height / 2));
  // A second card is offset, so two in a row do not stack perfectly.
  const second = spawnRect(viewport, { left: 0, top: 0 }, 1);
  assert.notEqual(second.left, first.left);
  // The scroll offset is the plane's origin, and nothing lands off it.
  const panned = spawnRect(viewport, { left: 4000, top: 2000 }, 0);
  assert.ok(panned.left > first.left && panned.top > first.top);
  assert.ok(spawnRect({ width: 10, height: 10 }, { left: 0, top: 0 }, 0).left >= 0);
});

// The front matter is the shell's alone — the daemon's `.note` codec carries
// bytes and reads no field (`src/note.rs`). What must hold is that ONE shape
// comes out for one look, or an autosave after a read rewrites the header for
// nothing, and every read-modify-write becomes a diff.
test("the front-matter block has exactly one shape per look", () => {
  const plain = { tone: "plum", fill: "wash", ink: "default" };
  assert.equal(withStyle("# Title\n\nbody\n", plain), "---\ncolor: plum\n---\n# Title\n\nbody\n");
  // Replacing, not stacking.
  const once = withStyle("# Title\n\nbody\n", plain);
  assert.equal(
    withStyle(once, { ...plain, tone: "sage" }),
    "---\ncolor: sage\n---\n# Title\n\nbody\n",
  );
  assert.equal(colorOf(once), "plum");
  assert.equal(bodyOf(once), "# Title\n\nbody\n");
  // The DEFAULTS are omitted: a note nobody restyled keeps the single
  // `color:` line it has always had, so this change rewrites no existing file.
  assert.equal(withStyle("x\n", { tone: "sand" }), "---\ncolor: sand\n---\nx\n");
  // An unknown name in any of the three falls back, never lands in the file.
  assert.equal(
    withStyle("x\n", { tone: "chartreuse", fill: "glossy", ink: "neon" }),
    "---\ncolor: sand\n---\nx\n",
  );
  // A full look, in the declared field order.
  assert.equal(
    withStyle("x\n", { tone: "ochre", fill: "solid", ink: "light" }),
    "---\ncolor: ochre\nfill: solid\nink: light\n---\nx\n",
  );
});

test("a look survives the round trip, and a hand-edited one falls back", () => {
  const look = { tone: "ochre", fill: "solid", ink: "light", font: "serif", size: "l" };
  assert.deepEqual(styleOf(withStyle("x\n", look)), look);
  // No front matter at all, and a block naming nothing this shell knows.
  assert.deepEqual(styleOf("# T\n"), {
    tone: "sand",
    fill: "wash",
    ink: "default",
    font: "sans",
    size: "m",
  });
  assert.deepEqual(styleOf("---\ncolor: sage\nfill: glossy\nfont: comic\nsize: 42\n---\nx\n"), {
    tone: "sage",
    fill: "wash",
    ink: "default",
    font: "sans",
    size: "m",
  });
  // Every name in a closed set survives the round trip; the defaults are in
  // their sets. Which names the palette offers, and in what order, is its data.
  for (const [set, def] of [
    [FILLS, DEFAULT_FILL],
    [INKS, DEFAULT_INK],
    [FONTS, DEFAULT_FONT],
    [SIZES, DEFAULT_SIZE],
  ]) {
    assert.ok(set.includes(def), `${def} in ${set}`);
  }
  for (const tone of TONES) assert.ok(INKS.includes(tone), `ink ${tone}`);
  for (const font of FONTS) assert.equal(fontOf(font), font);
  for (const size of SIZES) assert.equal(sizeOf(size), size);
  // A hand-edited name falls back rather than reaching the stylesheet, where
  // it would land as a `data-font` no rule matches — a card with no face.
  assert.equal(fontOf("Comic Sans"), "sans");
  assert.equal(sizeOf("14px"), "m");
  assert.equal(fontOf("mono"), "mono");
  assert.equal(sizeOf("xl"), "xl");
});

// Renaming a note writes the header and leaves the document alone — and it is
// the one door a legacy note's heading comes up through.
test("the title is renamed in the header, and the look rides along", () => {
  const md = withStyle("body\n", { tone: "plum", ink: "light" });
  const next = withTitle(md, "New");
  assert.equal(titleOf(next), "New");
  assert.equal(bodyOf(next), "body\n");
  assert.deepEqual(styleOf(next), {
    tone: "plum",
    fill: "wash",
    ink: "light",
    font: "sans",
    size: "m",
  });
  // MIGRATION: the legacy heading is lifted out of the body, with the blank
  // line that followed it — the name is no longer printed twice.
  const migrated = withTitle("# Old\n\nbody\n", "New");
  assert.equal(titleOf(migrated), "New");
  assert.equal(bodyOf(migrated), "body\n");
  // A `#` that is NOT the body's first line stays where the operator put it.
  assert.equal(bodyOf(withTitle("preamble\n\n# Section\n", "New")), "preamble\n\n# Section\n");
  // An empty name UNTITLES: the field goes rather than being written empty.
  assert.equal(titleOf(withTitle(migrated, "  ")), "Untitled note");
  assert.equal(titleFieldOf(withTitle(migrated, "")), null);
  // A newline pasted into the field is a name, not a second block.
  assert.equal(titleOf(withTitle("body\n", "One\nTwo")), "One Two");
});

test("a document without recognised front matter has no colour", () => {
  assert.equal(colorOf(""), null);
  assert.equal(colorOf("# Title\n"), null);
  // An unterminated fence is not front matter, and a fence that is not the
  // first line is a horizontal rule.
  assert.equal(colorOf("---\ncolor: sage\n"), null);
  assert.equal(colorOf("# Title\n---\ncolor: sage\n---\n"), null);
  assert.equal(colorOf("---\ncolor: chartreuse\n---\n"), null);
  assert.equal(bodyOf("# Title\n"), "# Title\n");
  // CRLF, from an editor that touched the file.
  assert.equal(colorOf("---\r\ncolor: rose\r\n---\r\n# T\r\n"), "rose");
  assert.equal(bodyOf("---\r\ncolor: rose\r\n---\r\n# T\r\n"), "# T\r\n");
});

test("a note with no heading is named by a UTC stamp", () => {
  assert.equal(stampName(new Date(Date.UTC(2026, 8, 22, 7, 5, 3))), "note-20260922-070503");
  assert.match(stampName(), /^note-\d{8}-\d{6}$/);
});

test("a card with unsaved text never sleeps", () => {
  const base = { visible: false, dirty: false, inFlight: false, asleep: false, elapsed: 99e3, after: 15e3 };
  assert.equal(noteDormancyDecision(base), "sleep");
  // The two refusals are the point: tearing the editor down is what would lose
  // the text, so neither state may ever answer "sleep".
  assert.equal(noteDormancyDecision({ ...base, dirty: true }), "stay");
  assert.equal(noteDormancyDecision({ ...base, inFlight: true }), "stay");
  // Not yet off-screen for long enough.
  assert.equal(noteDormancyDecision({ ...base, elapsed: 1e3 }), "stay");
  // Visible: wake if asleep, and never sleep.
  assert.equal(noteDormancyDecision({ ...base, visible: true }), "stay");
  assert.equal(noteDormancyDecision({ ...base, visible: true, asleep: true }), "wake");
  // On top: the observer cannot see a fixed card, so the fold must not sleep it.
  assert.equal(noteDormancyDecision({ ...base, onTop: true }), "stay");
  assert.equal(noteDormancyDecision({ ...base, onTop: true, asleep: true }), "wake");
  // Asleep and still away: nothing to do.
  assert.equal(noteDormancyDecision({ ...base, asleep: true }), "stay");
});

test("a new note is born in the operator's colours, and reading still defaults apart", () => {
  // The two questions the constants answer are NOT the same one. A file that
  // names no `fill:` is a wash, whatever a new card is dressed in — otherwise
  // changing what a new note looks like would repaint every note ever
  // written, because the field they omit is the one being redefined.
  const { tone, fill, ink } = NEW_NOTE_STYLE;
  assert.deepEqual(Object.keys(NEW_NOTE_STYLE).toSorted(), ["fill", "ink", "tone"]);
  assert.notEqual(fill, DEFAULT_FILL, "a new card is not dressed in the reading default");
  assert.deepEqual(styleOf("---\ncolor: sage\n---\nbody\n"), {
    tone: "sage",
    fill: DEFAULT_FILL,
    ink: DEFAULT_INK,
    font: DEFAULT_FONT,
    size: DEFAULT_SIZE,
  });
  // And a card dressed in them writes all three out, because two of them are
  // no longer what an absent field means.
  const md = withStyle("body\n", NEW_NOTE_STYLE);
  assert.equal(md, `---\ncolor: ${tone}\nfill: ${fill}\nink: ${ink}\n---\nbody\n`);
  // A new note takes the operator's COLOURS and the reading defaults for the
  // hand and the size — which is why `NEW_NOTE_STYLE` names three fields and
  // not five, and why neither `font:` nor `size:` is in the file above.
  assert.deepEqual(styleOf(md), {
    ...NEW_NOTE_STYLE,
    font: DEFAULT_FONT,
    size: DEFAULT_SIZE,
  });
});

// The hand and the size are fields of the look like any other, which means the
// one shape rule holds for them too: a default is OMITTED, so turning them on
// rewrites no file that never used them.
test("the hand and the size are written only when they are not the default", () => {
  assert.equal(withStyle("x\n", { tone: "sage" }), "---\ncolor: sage\n---\nx\n");
  assert.equal(
    withStyle("x\n", { tone: "sage", font: "sans", size: "m" }),
    "---\ncolor: sage\n---\nx\n",
  );
  // In the declared order, after the colour they refine.
  assert.equal(
    withStyle("x\n", { tone: "sage", font: "mono", size: "xl" }),
    "---\ncolor: sage\nfont: mono\nsize: xl\n---\nx\n",
  );
  // Replacing, never stacking — the same rule the tone's own field follows.
  const once = withStyle("x\n", { tone: "sage", font: "mono" });
  assert.equal(withStyle(once, { tone: "sage", font: "serif" }), "---\ncolor: sage\nfont: serif\n---\nx\n");
  // And a hand or a size rides through a RETITLE, like every other field.
  const named = withTitle(withStyle("body\n", { tone: "plum", size: "l" }), "Named");
  assert.equal(styleOf(named).size, "l");
  assert.equal(titleOf(named), "Named");
});

test("the footer says when a note last landed, and says the day when it was not today", () => {
  const at = new Date(2026, 8, 22, 19, 42).getTime();
  // Same day: the time is the whole answer.
  assert.equal(savedLabel(at, new Date(2026, 8, 22, 23, 59).getTime()), "Saved 19:42");
  // Another day: "19:42" alone would claim this afternoon.
  assert.equal(savedLabel(at, new Date(2026, 8, 23, 0, 1).getTime()), "Saved 22/09 19:42");
  assert.equal(savedLabel(at, new Date(2027, 8, 22, 19, 42).getTime()), "Saved 22/09 19:42");
  // A note no save has landed for claims no time at all.
  assert.equal(savedLabel(null, Date.now()), "");
  assert.equal(savedLabel(0, Date.now()), "");
});

test("a note carries whether it opens veiled, and every writer carries it along", () => {
  const plain = '---\ntitle: "Keys"\ncolor: ochre\n---\nsecret\n';
  assert.equal(veiledOf(plain), false);
  const hidden = withVeil(plain, true);
  assert.equal(veiledOf(hidden), true);
  assert.match(hidden, /^---\ntitle: "Keys"\ncolor: ochre\nhidden: true\n---\nsecret\n$/);
  // `hidden: false` is what every note already is; writing it would put a line
  // in every file to say nothing.
  assert.equal(withVeil(hidden, false), plain);
  // The two OTHER writers must carry it, or a recolour or a rename would
  // quietly un-hide the note.
  assert.equal(veiledOf(withStyle(hidden, { tone: "plum", fill: "solid", ink: "dark" })), true);
  assert.equal(veiledOf(withTitle(hidden, "Passwords")), true);
  assert.equal(titleOf(withTitle(hidden, "Passwords")), "Passwords");
  // And the body survives both, which is the thing the veil must never cost.
  assert.equal(bodyOf(withTitle(hidden, "Passwords")), "secret\n");
  // A hand-written `hidden:` that is not a boolean names nothing in the set.
  assert.equal(veiledOf('---\ncolor: sand\nhidden: sometimes\n---\nx\n'), false);
});

// A card on top (ADR-0064, 2026-09-26 amendment §§2, 8).
test("a card on top floats in the top-right corner, between its floor and its ceiling", () => {
  const vp = { width: 1600, height: 1000 };
  // A small card grows to the floor: 240×180 is too small to edit over a console.
  assert.deepEqual(onTopRect({ width: 240, height: 180 }, vp), {
    band: false,
    left: 1600 - 420 - 12,
    // Below a maximized console's title bar, whose buttons share the corner.
    top: ON_TOP_TOP,
    width: 420,
    height: 320,
  });
  // A big card shrinks to the ceiling: half the width, 80 % of the height.
  const big = onTopRect({ width: 1400, height: 950 }, vp);
  assert.equal(big.width, 800);
  assert.equal(big.height, 800);
  assert.equal(big.left + big.width, 1600 - 12);
  // A card between the two keeps its own size.
  const mid = onTopRect({ width: 500, height: 400 }, vp);
  assert.deepEqual([mid.width, mid.height], [500, 400]);
  // A short viewport: the height ceiling wins over the floor, and the card
  // never runs past the bottom.
  assert.equal(onTopRect({ width: 240, height: 180 }, { width: 1600, height: 300 }).height, 240);
  const short = onTopRect({ width: 240, height: 180 }, { width: 1600, height: 200 });
  assert.equal(short.height, 200 - ON_TOP_TOP - 12);
});

test("below the band width the card on top is a band, because half the width is under the floor", () => {
  const edge = ON_TOP_BAND_BELOW;
  assert.deepEqual(onTopRect({ width: 240, height: 180 }, { width: edge - 1, height: 800 }), {
    band: true,
  });
  assert.equal(onTopRect({ width: 240, height: 180 }, { width: edge, height: 800 }).band, false);
  // A phone: the band, whatever the card's size.
  assert.equal(onTopRect({ width: 2000, height: 2000 }, { width: 390, height: 800 }).band, true);
});

test("a floating box is kept inside the viewport after a drag or a resize of the window", () => {
  const box = { left: 900, top: -40, width: 500, height: 400 };
  // Pulled back in on both axes, size kept.
  assert.deepEqual(onTopClamp(box, { width: 1200, height: 900 }), {
    band: false,
    left: 700,
    top: 0,
    width: 500,
    height: 400,
  });
  // A viewport smaller than the box: the size shrinks first.
  const small = onTopClamp({ left: 0, top: 0, width: 1000, height: 1000 }, { width: 900, height: 600 });
  assert.deepEqual([small.width, small.height, small.left, small.top], [900, 600, 0, 0]);
  // Narrow again: back to the band.
  assert.deepEqual(onTopClamp(box, { width: 600, height: 900 }), { band: true });
});
