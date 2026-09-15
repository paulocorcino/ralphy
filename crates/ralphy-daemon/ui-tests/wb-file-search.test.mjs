// Unit tests for assets/ui/wb-file-search.js — the pure half of the FILES
// search (ADR-0036 amendment 2026-09-15): which verb a mode names, when a
// query is worth a walk, which levels the hits need loaded, what the gutter
// says, and what to fold back on clear.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { UI } from "./harness.mjs";

const SRC = readFileSync(join(UI, "wb-file-search.js"), "utf8");
function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBFileSearch;
}
const fs = load();

test("the toggle names the verb; anything else is a name search", () => {
  assert.equal(fs.verbFor("content"), "tree.grep");
  assert.equal(fs.verbFor("name"), "tree.find");
  assert.equal(fs.verbFor(undefined), "tree.find");
});

test("a query is worth a walk at two trimmed characters", () => {
  assert.equal(fs.worthSearching("ab"), true);
  assert.equal(fs.worthSearching("  ab  "), true);
  // NEGATIVE CONTROL: one character, or two of whitespace, is not.
  assert.equal(fs.worthSearching("a"), false);
  assert.equal(fs.worthSearching("  "), false);
  assert.equal(fs.worthSearching(null), false);
});

test("dirsToLoad is every distinct ancestor, shallow-first", () => {
  const hits = [
    { path: "src/deep/task_list.md" },
    { path: "src/main.rs" },
    { path: ".ralphy/plan.md" },
    { path: "README.md" }, // a root hit needs nothing loaded
  ];
  assert.deepEqual(fs.dirsToLoad(hits), [".ralphy", "src", "src/deep"]);
  assert.deepEqual(fs.dirsToLoad([]), []);
});

test("the note names the cap, the miss, or nothing", () => {
  assert.equal(fs.note({ hits: [], truncated: false }), "no matches");
  assert.equal(
    fs.note({ hits: [{ path: "a" }], truncated: true }),
    "showing the first 200 — refine the search",
  );
  assert.equal(fs.note({ hits: [{ path: "a" }], truncated: false }), "");
});

test("toCollapse folds only what the search opened, deepest first", () => {
  const before = ["src"];
  const now = ["src", "src/deep", ".ralphy", "src/deep/er"];
  assert.deepEqual(fs.toCollapse(before, now), ["src/deep/er", "src/deep", ".ralphy"]);
  // NEGATIVE CONTROL: no snapshot means no search expanded anything.
  assert.deepEqual(fs.toCollapse(null, now), []);
});

test("hitMap carries a count for content hits and true for name hits", () => {
  const m = fs.hitMap([{ path: "a", count: 3 }, { path: "b" }]);
  assert.equal(m.get("a"), 3);
  assert.equal(m.get("b"), true);
  assert.equal(m.has("c"), false);
});
