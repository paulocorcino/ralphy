// Unit tests for assets/ui/wb-sheet-read.ts against the vendored SheetJS: the
// sheets of a workbook with their names and cell text, and a file of a zip.
import { test } from "node:test";
import assert from "node:assert/strict";
import * as XLSX from "../assets/ui/vendor/sheetjs/xlsx.mjs";
import { loadSheetJs, readSheets, zipFile, zipPaths } from "../assets/ui/wb-sheet-read.ts";
import { VIEWERS, viewerKindOf } from "../assets/ui/wb-viewers.ts";

function workbook(bookType = "xlsx") {
  const book = XLSX.utils.book_new();
  const data = XLSX.utils.aoa_to_sheet([
    ["name", "amount"],
    ["fig", 1.5],
  ]);
  data.C2 = { t: "n", f: "B2*2", v: 3 };
  data["!ref"] = "A1:C2";
  XLSX.utils.book_append_sheet(book, data, "Relatório");
  XLSX.utils.book_append_sheet(book, XLSX.utils.aoa_to_sheet([["only"]]), "Second");
  return new Uint8Array(XLSX.write(book, { type: "array", bookType }));
}

test("the sheets keep their names, and a formula shows its stored value", async () => {
  const xlsx = await loadSheetJs((path) => import(path.replace("./", "../assets/ui/")).then(
    (module) => ({ ok: true, module }),
  ));
  assert.notEqual(typeof xlsx, "string", "the vendored module is SheetJS");
  const sheets = readSheets(xlsx, workbook());
  assert.deepEqual(
    sheets.map((sheet) => sheet.name),
    ["Relatório", "Second"],
  );
  assert.deepEqual(sheets[0].rows, [
    ["name", "amount", ""],
    ["fig", "1.5", "3"],
  ]);
});

test("a module that is not SheetJS gives the error text", async () => {
  const outcome = await loadSheetJs(() => Promise.resolve({ ok: true, module: {} }));
  assert.equal(typeof outcome, "string");
});

test("a file of a zip is found by its path, and the paths have no root", () => {
  const zip = XLSX.CFB.read(workbook(), { type: "array" });
  const workbookXml = new TextDecoder().decode(zipFile(zip, "xl/workbook.xml"));
  assert.match(workbookXml, /Relatório/u);
  assert.equal(zipFile(zip, "missing.xml"), null);
  assert.ok(zipPaths(zip).includes("xl/workbook.xml"));
  assert.equal(zipPaths(zip).some((path) => path.startsWith("Root Entry")), false);
});

test("an xls (Excel 97-2003) reads like an xlsx, and opens in the sheet viewer", async () => {
  const xlsx = await loadSheetJs((path) =>
    import(path.replace("./", "../assets/ui/")).then((module) => ({ ok: true, module })),
  );
  const sheets = readSheets(xlsx, workbook("biff8"));
  assert.deepEqual(
    sheets.map((sheet) => sheet.name),
    ["Relatório", "Second"],
  );
  assert.deepEqual(sheets[0].rows[1], ["fig", "1.5", "3"]);
  assert.equal(viewerKindOf("xls"), "sheet");
  assert.ok(VIEWERS.sheet.extensions.includes("xls"));
});
