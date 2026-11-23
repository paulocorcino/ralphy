// SheetJS, the reader of xlsx and ods files and of the zip of an odt
// (ADR-0091 D8, D10, D11, amendment 2026-11-23). It loads the first time a
// file of those kinds opens.
import type { VendorLoad } from "./wb-vendor-load.ts";

export const SHEETJS_PATH = "./vendor/sheetjs/xlsx.mjs";
export const SHEETJS_ERROR_TEXT = "This file could not be read.";

type WorkSheet = object;
type WorkBook = { SheetNames: string[]; Sheets: Record<string, WorkSheet | undefined> };

/** One file of a zip that `CFB.read` opened. */
type ZipEntry = { content: Uint8Array; size: number };
/** A zip that `CFB.read` opened: each path, and the entry at the same index. */
export type Zip = { FullPaths: string[]; FileIndex: ZipEntry[] };

/** The part of SheetJS the viewers use. */
export type SheetJs = {
  read(data: Uint8Array, options: { type: "array"; cellDates: true }): WorkBook;
  utils: {
    sheet_to_json(
      sheet: WorkSheet,
      options: { header: 1; raw: false; defval: ""; blankrows: true },
    ): string[][];
  };
  CFB: { read(data: Uint8Array, options: { type: "array" }): Zip };
};

function isSheetJs(m: unknown): m is SheetJs {
  return typeof m === "object" && m !== null && "read" in m && "utils" in m && "CFB" in m;
}

/** SheetJS, or the text a pane shows when it does not load. */
export async function loadSheetJs(load: (path: string) => Promise<VendorLoad>) {
  const outcome = await load(SHEETJS_PATH);
  if (!outcome.ok) return outcome.text;
  return isSheetJs(outcome.module) ? outcome.module : SHEETJS_ERROR_TEXT;
}

/** One sheet: its name and its rows of cell text. */
export type Sheet = { name: string; rows: string[][] };

/** The sheets of an xlsx or ods file, each cell as the text the file shows. */
export function readSheets(xlsx: SheetJs, bytes: Uint8Array): Sheet[] {
  const book = xlsx.read(bytes, { type: "array", cellDates: true });
  return book.SheetNames.map((name) => {
    const sheet = book.Sheets[name];
    const options = { header: 1, raw: false, defval: "", blankrows: true } as const;
    return { name, rows: sheet ? xlsx.utils.sheet_to_json(sheet, options) : [] };
  });
}

/** The bytes of the file at `path` inside a zip, or null when it has none. */
export function zipFile(zip: Zip, path: string): Uint8Array | null {
  const at = zip.FullPaths.findIndex((full) => full.endsWith(`/${path}`));
  return at === -1 ? null : (zip.FileIndex[at]?.content ?? null);
}

/** The paths of the files of a zip, without the root that `CFB` adds. */
export function zipPaths(zip: Zip): string[] {
  return zip.FullPaths.map((full) => full.slice(full.indexOf("/") + 1)).filter(
    (path) => path !== "" && !path.endsWith("/"),
  );
}
