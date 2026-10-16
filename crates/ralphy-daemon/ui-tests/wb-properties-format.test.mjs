// Unit tests for assets/ui/wb-properties-format.ts: the words of the
// Properties dialog, as pure functions.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  folderContents,
  folderSize,
  formatBytes,
  formatModified,
  modifiedText,
  typeText,
  yesNo,
} from "../assets/ui/wb-properties-format.ts";

test("a size reads in units of 1024 and keeps the exact bytes", () => {
  const rows = [
    [0, "0 bytes"],
    [1, "1 byte"],
    [1023, "1,023 bytes"],
    [1024, "1.0 KB (1,024 bytes)"],
    [12698, "12.4 KB (12,698 bytes)"],
    [3435973837, "3.2 GB (3,435,973,837 bytes)"],
  ];
  for (const [bytes, want] of rows) assert.equal(formatBytes(bytes), want, String(bytes));
});

test("a size above the last unit stays in terabytes", () => {
  assert.equal(formatBytes(2 * 1024 ** 5), "2048.0 TB (2,251,799,813,685,248 bytes)");
});

test("a time reads as day, month, year and the time of day, in the zone it is given", () => {
  const ms = Date.UTC(2026, 9, 15, 19, 48);
  assert.equal(formatModified(ms, "UTC"), "15 Oct 2026, 19:48");
  assert.equal(formatModified(ms, "America/Sao_Paulo"), "15 Oct 2026, 16:48");
});

test("a time the system does not keep reads Unknown", () => {
  assert.equal(modifiedText(undefined), "Unknown");
  assert.equal(modifiedText(Date.UTC(2026, 9, 15, 19, 48)).startsWith("15 Oct 2026"), true);
});

test("the type of a file is what the workbench would do with it", () => {
  const rows = [
    [{ class: "text", encoding: "UTF-8", bom: false }, "Text · UTF-8"],
    [{ class: "text", encoding: "UTF-16LE", bom: true }, "Text · UTF-16LE with BOM"],
    [{ class: "image", image: "png" }, "Image · PNG"],
    [{ class: "image", image: "jpg" }, "Image · JPG"],
    [{ class: "binary" }, "Binary"],
    [{ class: "too_large" }, "Too large to open in the workbench"],
    [undefined, "Unknown"],
  ];
  for (const [type, want] of rows) assert.equal(typeText(type), want, JSON.stringify(type));
});

test("a yes or no fact reads Yes only for true", () => {
  assert.deepEqual([yesNo(true), yesNo(false), yesNo(undefined)], ["Yes", "No", "No"]);
});

test("a folder size is a minimum when the count stopped early", () => {
  const whole = { bytes: 2048, truncated: false };
  assert.equal(folderSize(whole), "2.0 KB (2,048 bytes)");
  assert.equal(folderSize({ ...whole, truncated: true }), "≥ 2.0 KB (2,048 bytes)");
});

test("a folder says how many files and folders it holds, with the right plural", () => {
  assert.equal(folderContents({ files: 3, dirs: 1 }), "3 files, 1 folder");
  assert.equal(folderContents({ files: 1, dirs: 0 }), "1 file, 0 folders");
  assert.equal(folderContents({ files: 214000, dirs: 2 }), "214,000 files, 2 folders");
});
