// Unit tests for assets/ui/wb-editor-status.ts: the counts of the editor's
// status bar, as pure functions.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  clampColumn,
  estimateTokens,
  estimateWrappedLines,
  linesText,
  readsFor,
  selectionText,
  shortCount,
  typedColumn,
  tokensText,
  wrapOptions,
} from "../assets/ui/wb-editor-status.ts";

test("a wrapped-line estimate cuts each line at the column, and an empty line is one", () => {
  assert.equal(estimateWrappedLines([""], 10, 4), 1);
  assert.equal(estimateWrappedLines(["x".repeat(10)], 10, 4), 1);
  assert.equal(estimateWrappedLines(["x".repeat(11)], 10, 4), 2);
  assert.equal(estimateWrappedLines(["x".repeat(25), "", "abc"], 10, 4), 5);
});

test("a tab counts to the next tab stop", () => {
  // "a\tb" with tab size 4 is 5 columns wide: a, three spaces, b.
  assert.equal(estimateWrappedLines(["a\tb"], 4, 4), 2);
  assert.equal(estimateWrappedLines(["\t\t"], 8, 4), 1);
});

test("a token estimate is a quarter of the characters, rounded up", () => {
  assert.equal(estimateTokens(0), 0);
  assert.equal(estimateTokens(1), 1);
  assert.equal(estimateTokens(9892), 2473);
});

test("a count reads short in the bar", () => {
  assert.equal(shortCount(950), "950");
  assert.equal(shortCount(2473), "2.5k");
  assert.equal(shortCount(150_000), "150k");
  assert.equal(shortCount(1_250_000), "1.3M");
});

test("an agent needs one read per 25k tokens, and at least one", () => {
  assert.equal(readsFor(0), 1);
  assert.equal(readsFor(25_000), 1);
  assert.equal(readsFor(25_001), 2);
  assert.equal(readsFor(150_000), 6);
});

test("the bar's texts are short, and their values sit at the even places", () => {
  assert.deepEqual(linesText(61, 74, 100), ["61", " ln ≈", "74", "@", "100"]);
  assert.deepEqual(tokensText(150_000), ["≈150k", " tok (", "6", " rd)"]);
  assert.deepEqual(selectionText(412), ["", "sel ", "412", " ch ", "≈103", " tok"]);
});

test("a column wrap keeps the text inside a narrow pane", () => {
  assert.deepEqual(wrapOptions({ mode: "column", column: 100 }), {
    wordWrap: "bounded",
    wordWrapColumn: 100,
  });
  assert.deepEqual(wrapOptions({ mode: "window", column: 100 }), { wordWrap: "on" });
  assert.deepEqual(wrapOptions({ mode: "off", column: 100 }), { wordWrap: "off" });
});

test("text that is not a number keeps the column it had", () => {
  assert.equal(typedColumn(" 80 ", 100), 80);
  assert.equal(typedColumn("abc", 100), 100);
  assert.equal(typedColumn("", 100), 100);
  assert.equal(typedColumn("9", 100), 20);
});

test("a typed column is held to the field's range", () => {
  assert.equal(clampColumn(5), 20);
  assert.equal(clampColumn(1000), 400);
  assert.equal(clampColumn(99.6), 100);
  assert.equal(clampColumn(Number.NaN), 100);
});
