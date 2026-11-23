// The rows of a csv or tsv text with a chosen separator. Tabulator's csv
// importer reads only commas, and a csv from a spreadsheet set to Portuguese
// or German uses semicolons.

/** A separator a delimited text can use. */
export type Separator = "," | ";" | "\t" | "|";

/** The separators the grid offers, with the name the selector shows. */
export const SEPARATORS: readonly { value: Separator; label: string }[] = [
  { value: ",", label: "Comma" },
  { value: ";", label: "Semicolon" },
  { value: "\t", label: "Tab" },
  { value: "|", label: "Bar" },
];

function firstLine(text: string): string {
  const end = text.search(/\r?\n/u);
  return end === -1 ? text : text.slice(0, end);
}

/** The separator used most in the first line; a comma when none is used. */
export function guessSeparator(text: string): Separator {
  const line = firstLine(text);
  let best: Separator = ",";
  let most = 0;
  for (const { value } of SEPARATORS) {
    const count = line.split(value).length - 1;
    if (count > most) {
      best = value;
      most = count;
    }
  }
  return best;
}

type Reader = { rows: string[][]; row: string[]; cell: string; quoted: boolean };

function endCell(r: Reader) {
  r.row.push(r.cell);
  r.cell = "";
}

function endRow(r: Reader) {
  endCell(r);
  r.rows.push(r.row);
  r.row = [];
}

// One character outside quotes; returns how many characters it used.
function readPlain(r: Reader, text: string, at: number, separator: Separator): number {
  const char = text[at];
  if (char === '"' && r.cell === "") r.quoted = true;
  else if (char === separator) endCell(r);
  else if (char === "\n") endRow(r);
  else if (char === "\r") {
    endRow(r);
    return text[at + 1] === "\n" ? 2 : 1;
  } else r.cell += char;
  return 1;
}

// One character inside quotes: a doubled quote is one quote.
function readQuoted(r: Reader, text: string, at: number): number {
  if (text[at] !== '"') {
    r.cell += text[at];
    return 1;
  }
  if (text[at + 1] === '"') {
    r.cell += '"';
    return 2;
  }
  r.quoted = false;
  return 1;
}

/** The rows of a delimited text: quotes hold separators and line breaks. */
export function parseDelimited(text: string, separator: Separator): string[][] {
  const r: Reader = { rows: [], row: [], cell: "", quoted: false };
  let at = 0;
  while (at < text.length) {
    at += r.quoted ? readQuoted(r, text, at) : readPlain(r, text, at, separator);
  }
  if (r.cell !== "" || r.row.length > 0) endRow(r);
  return r.rows;
}
