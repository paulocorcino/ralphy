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

test("a note is titled by its first `#` heading and nothing else", () => {
  assert.equal(N.titleOf("# Standup\n\nbody\n"), "Standup");
  // Front matter is not a heading, and neither is a `##`.
  assert.equal(N.titleOf("---\ncolor: sage\n---\n# Groceries\n"), "Groceries");
  assert.equal(N.titleOf("## Section\n\nbody\n"), "Untitled note");
  // A later `#` still counts — the FIRST one wins, not the first line.
  assert.equal(N.titleOf("some preamble\n\n# Real title\n"), "Real title");
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
  // The set is the daemon's `note::Color`, in its order.
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
