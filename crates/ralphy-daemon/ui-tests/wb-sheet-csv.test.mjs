// Unit tests for assets/ui/wb-sheet-csv.ts: the rows of a delimited text and
// the guess of its separator.
import { test } from "node:test";
import assert from "node:assert/strict";
import { guessSeparator, parseDelimited } from "../assets/ui/wb-sheet-csv.ts";

test("a semicolon text splits on semicolons and keeps the commas of a decimal", () => {
  assert.deepEqual(parseDelimited("name;value\nfig;3,5\n", ";"), [
    ["name", "value"],
    ["fig", "3,5"],
  ]);
});

test("quotes hold a separator, a line break and a doubled quote", () => {
  const text = 'a,"b,c","line\none","say ""hi"""\r\nx,y,z,w';
  assert.deepEqual(parseDelimited(text, ","), [
    ["a", "b,c", "line\none", 'say "hi"'],
    ["x", "y", "z", "w"],
  ]);
});

test("empty cells stay, and a last line with no break is read", () => {
  assert.deepEqual(parseDelimited("a,,c\n,b,", ","), [
    ["a", "", "c"],
    ["", "b", ""],
  ]);
});

test("the guess is the separator used most in the first line", () => {
  assert.equal(guessSeparator("a;b;c\n1,5;2;3"), ";");
  assert.equal(guessSeparator("a\tb\tc"), "\t");
  assert.equal(guessSeparator("a|b|c,d"), "|");
  assert.equal(guessSeparator("a,b"), ",");
  assert.equal(guessSeparator("single"), ",", "no separator: a comma");
});
