// Unit tests for assets/ui/wb-viewport.ts — the phone-layout and touch-input
// queries — and the drift check for their copies in the stylesheets.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { UI } from "./harness.mjs";
import { PHONE_QUERY, TOUCH_QUERY, phoneLayout, touchInput } from "../assets/ui/wb-viewport.ts";

// Every `@media` condition in the stylesheets and in the popup's inline
// <style>, as `{ file, query }`, with its whitespace collapsed.
function mediaQueries() {
  const sources = readdirSync(join(UI, "styles"))
    .filter((name) => name.endsWith(".css"))
    .map((name) => ({ file: `styles/${name}`, text: readFileSync(join(UI, "styles", name), "utf8") }));
  sources.push({ file: "detached-fence.html", text: readFileSync(join(UI, "detached-fence.html"), "utf8") });
  return sources.flatMap(({ file, text }) =>
    [...text.matchAll(/@media\s+([^{]+)\{/g)].map((m) => ({ file, query: m[1].replace(/\s+/g, " ").trim() })),
  );
}

test("the stylesheets copy the phone and touch queries exactly", () => {
  const all = mediaQueries();
  const phone = all.filter((m) => m.query === PHONE_QUERY);
  const touch = all.filter((m) => m.query.includes(TOUCH_QUERY));
  // NEGATIVE CONTROL: the scan must reach the blocks it guards. A regex that
  // matched nothing would report no drift forever.
  assert.ok(phone.length >= 3, `found ${phone.length} phone-layout blocks`);
  assert.ok(touch.length >= 3, `found ${touch.length} touch blocks`);
  // The landscape half of the phone query names only that query.
  const landscape = all.filter((m) => m.query.includes("max-height") && m.query !== PHONE_QUERY);
  assert.deepEqual(landscape, [], "a phone-layout block differs from wb-viewport.ts PHONE_QUERY");
  // A touch rule reads every pointer, not only the primary one.
  const primary = all.filter((m) => m.query.includes("(pointer: coarse)") && m.query !== PHONE_QUERY);
  assert.deepEqual(primary, [], "a touch rule uses (pointer: coarse); use wb-viewport.ts TOUCH_QUERY");
});

test("phoneLayout and touchInput ask matchMedia their own query", () => {
  const asked = [];
  const win = {
    matchMedia(q) {
      asked.push(q);
      return { matches: q === TOUCH_QUERY };
    },
  };
  assert.equal(phoneLayout(win), false);
  assert.equal(touchInput(win), true);
  assert.deepEqual(asked, [PHONE_QUERY, TOUCH_QUERY]);
});

test("a window with no matchMedia, or one that throws, is neither a phone nor touch", () => {
  assert.equal(phoneLayout({}), false);
  assert.equal(touchInput({}), false);
  const throws = {
    matchMedia() {
      throw new Error("no media");
    },
  };
  assert.equal(phoneLayout(throws), false);
  assert.equal(touchInput(throws), false);
});
