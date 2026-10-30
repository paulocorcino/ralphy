/* ---------------------------------------------------------------------------
   The model of a `.dbml` file: a reader for the subset of DBML the workbench
   shows, and the grouping of its tables. Pure functions: no DOM, no globals.

   The text is repo content, so the reader never throws: a line it does not
   understand is counted in `skipped`, and the rest of the file is read.
   --------------------------------------------------------------------------- */

export type DbmlEndpoint = { table: string; column: string };

export type DbmlColumn = {
  name: string;
  type: string;
  pk: boolean;
  notNull: boolean;
  unique: boolean;
  note: string | null;
  /** The other side of an inline `ref:` setting. */
  ref: DbmlEndpoint | null;
};

export type DbmlTable = { name: string; note: string | null; columns: DbmlColumn[] };

/** `<` is stored as many-to-one with `from` and `to` swapped. */
export type DbmlRefKind = "many-to-one" | "one-to-one" | "many-to-many";

export type DbmlRef = { from: DbmlEndpoint; to: DbmlEndpoint; kind: DbmlRefKind };

export type DbmlGroup = { name: string; tables: string[] };

export type DbmlSchema = {
  tables: DbmlTable[];
  refs: DbmlRef[];
  groups: DbmlGroup[];
  skipped: number;
};

// ---- the text, as lines ---------------------------------------------------------

const QUOTES = "'\"`";

// The end of the comment that starts at `i`, or -1 when none starts there.
function commentEnd(text: string, i: number) {
  if (text.startsWith("//", i)) {
    const nl = text.indexOf("\n", i);
    return nl < 0 ? text.length : nl;
  }
  if (!text.startsWith("/*", i)) return -1;
  const close = text.indexOf("*/", i + 2);
  return close < 0 ? text.length : close + 2;
}

// The end of the string that starts at `i`, or -1 when none starts there. A
// triple-quoted string may span lines; any other string ends at its line.
function stringEnd(text: string, i: number) {
  if (!QUOTES.includes(text.charAt(i))) return -1;
  if (!text.startsWith("'''", i)) return lineStringEnd(text, i);
  const close = text.indexOf("'''", i + 3);
  return close < 0 ? text.length : close + 3;
}

// A string that ends at its closing quote, or at its line when it has none.
function lineStringEnd(text: string, i: number) {
  const q = text[i];
  let j = i + 1;
  while (j < text.length && text[j] !== q && text[j] !== "\n") j += text[j] === "\\" ? 2 : 1;
  return text[j] === q ? j + 1 : Math.min(j, text.length);
}

// A brace ends the line of a block header, and a closing brace is a line of
// its own. A brace inside a string never reaches this function.
function braceBreak(c: string) {
  if (c === "{") return "{\n";
  return c === "}" ? "\n}\n" : c;
}

// The file as trimmed, non-blank lines without comments. A string that spans
// lines is joined into one line.
function toLines(text: string): string[] {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const cEnd = commentEnd(text, i);
    const sEnd = cEnd < 0 ? stringEnd(text, i) : -1;
    if (cEnd >= 0) {
      out += text.slice(i, cEnd).replace(/[^\n]/g, "");
      i = cEnd;
    } else if (sEnd >= 0) {
      out += text.slice(i, sEnd).replace(/\s*\n\s*/g, " ");
      i = sEnd;
    } else {
      out += braceBreak(text.charAt(i));
      i += 1;
    }
  }
  return out
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

// ---- pieces of a line -----------------------------------------------------------

const IDENT = /[\p{L}\p{N}_$]+/uy;

function skipSpaces(s: string, i: number) {
  let j = i;
  while (j < s.length && /\s/.test(s.charAt(j))) j += 1;
  return j;
}

// One name: an identifier, or a quoted name (its quotes removed).
function readPart(s: string, i: number): { value: string; end: number } | null {
  if (s[i] === '"' || s[i] === "`") {
    const close = s.indexOf(s[i], i + 1);
    return close < 0 ? null : { value: s.slice(i + 1, close), end: close + 1 };
  }
  IDENT.lastIndex = i;
  const m = IDENT.exec(s);
  return m ? { value: m[0], end: i + m[0].length } : null;
}

type Dotted = { parts: string[]; end: number } | "composite" | null;

// Names joined by dots (`schema.table.column`). A `(` after a dot starts a
// composite column list, which the reader does not show.
function readDotted(s: string, from: number): Dotted {
  const parts: string[] = [];
  let i = skipSpaces(s, from);
  for (;;) {
    if (s[i] === "(") return "composite";
    const part = readPart(s, i);
    if (!part) return null;
    parts.push(part.value);
    i = part.end;
    if (s[i] !== ".") return { parts, end: i };
    i += 1;
  }
}

// The text of a quoted value, without its quotes. Whitespace runs become one
// space: a value that spanned lines was joined by `toLines`.
function unquote(raw: string) {
  const s = raw.trim();
  const triple = s.length >= 6 && s.startsWith("'''") && s.endsWith("'''");
  if (triple) return s.slice(3, -3).trim().replace(/\s+/g, " ");
  const q = s[0];
  const quoted = s.length >= 2 && (q === "'" || q === '"' || q === "`") && s.endsWith(q);
  if (!quoted) return s;
  return s.slice(1, -1).replace(/\\(['"`])/g, "$1");
}

// The end of a column type: `varchar(255)`, `DECIMAL(10, 2)`, `int[]`, or a
// quoted `"double precision"`. A `[` that starts the type is a setting list.
function typeEnd(s: string, start: number) {
  if (s[start] === '"') {
    const close = s.indexOf('"', start + 1);
    return close < 0 ? s.length : close + 1;
  }
  let i = start;
  while (i < s.length && !/\s/.test(s.charAt(i))) {
    if (s[i] === "[" && i === start) break;
    i = skipGroup(s, i);
  }
  return i;
}

function closerOf(opener: string) {
  if (opener === "(") return ")";
  return opener === "[" ? "]" : "";
}

// The index after the `(…)` or `[…]` group that starts at `i`, or after the
// one character there when no group starts.
function skipGroup(s: string, i: number) {
  const closer = closerOf(s.charAt(i));
  if (!closer) return i + 1;
  const close = s.indexOf(closer, i);
  return close < 0 ? s.length : close + 1;
}

// ---- settings `[pk, not null, note: '…']` ---------------------------------------

// Move the scan past the character `c`. True when `c` is a comma outside
// strings and parentheses.
function isSplitComma(scan: { quote: string; depth: number }, c: string) {
  if (scan.quote) {
    if (c === scan.quote) scan.quote = "";
  } else if (QUOTES.includes(c)) {
    scan.quote = c;
  } else if (c === "(") {
    scan.depth += 1;
  } else if (c === ")") {
    scan.depth -= 1;
  } else {
    return c === "," && scan.depth <= 0;
  }
  return false;
}

// The text between the first `[` and the last `]`, cut at the commas that are
// outside strings and parentheses.
function splitSettings(s: string): string[] {
  const open = s.indexOf("[");
  if (open < 0) return [];
  const close = s.lastIndexOf("]");
  const inner = s.slice(open + 1, close > open ? close : s.length);
  const parts: string[] = [];
  const scan = { quote: "", depth: 0 };
  let from = 0;
  for (let i = 0; i < inner.length; i += 1) {
    if (!isSplitComma(scan, inner.charAt(i))) continue;
    parts.push(inner.slice(from, i));
    from = i + 1;
  }
  parts.push(inner.slice(from));
  return parts.map((p) => p.trim()).filter((p) => p !== "");
}

type Settings = {
  pk: boolean;
  notNull: boolean;
  unique: boolean;
  note: string | null;
  ref: string | null;
};

// The settings the reader shows. `increment`, `default: …` and the others are
// not shown, so they are dropped here.
function readSettings(parts: string[]): Settings {
  const out: Settings = { pk: false, notNull: false, unique: false, note: null, ref: null };
  for (const part of parts) {
    const low = part.toLowerCase();
    const value = part.slice(part.indexOf(":") + 1);
    if (low === "pk" || low === "primary key") out.pk = true;
    else if (low === "not null") out.notNull = true;
    else if (low === "unique") out.unique = true;
    else if (/^note\s*:/.test(low)) out.note = unquote(value);
    else if (/^ref\s*:/.test(low)) out.ref = value.trim();
  }
  return out;
}

// ---- refs ----------------------------------------------------------------------

const REF_OP = /\s*(<>|>|<|-)\s*/y;

function readOp(s: string, i: number): { op: string; end: number } | null {
  REF_OP.lastIndex = i;
  const m = REF_OP.exec(s);
  const op = m?.[1];
  return m && op !== undefined ? { op, end: i + m[0].length } : null;
}

function endpointOf(d: Dotted): DbmlEndpoint | null {
  if (!d || d === "composite" || d.parts.length < 2) return null;
  const column = d.parts.at(-1);
  if (column === undefined) return null;
  return { table: d.parts.slice(0, -1).join("."), column };
}

function makeRef(a: DbmlEndpoint, op: string, b: DbmlEndpoint): DbmlRef {
  if (op === "<") return { from: b, to: a, kind: "many-to-one" };
  if (op === "-") return { from: a, to: b, kind: "one-to-one" };
  if (op === "<>") return { from: a, to: b, kind: "many-to-many" };
  return { from: a, to: b, kind: "many-to-one" };
}

// `a.b > c.d [settings]`, or just `> c.d` when `self` is the column it is
// written on. The result is null when the text has no ref the reader shows.
function readRef(text: string, self: DbmlEndpoint | null): DbmlRef | null {
  let i = 0;
  let a = self;
  if (!a) {
    const first = readDotted(text, 0);
    if (!first || first === "composite") return null;
    a = endpointOf(first);
    if (!a) return null;
    i = first.end;
  }
  const op = readOp(text, i);
  if (!op) return null;
  const b = endpointOf(readDotted(text, op.end));
  return b ? makeRef(a, op.op, b) : null;
}

// ---- the blocks ------------------------------------------------------------------

type Block =
  | { kind: "table"; table: DbmlTable }
  | { kind: "tablenote"; table: DbmlTable }
  | { kind: "ref" }
  | { kind: "group"; group: DbmlGroup }
  | { kind: "skip" };

type State = { schema: DbmlSchema; stack: Block[]; aliases: Map<string, string> };

const SKIP: Block = { kind: "skip" };

function openTable(st: State, header: string): Block {
  const rest = header.replace(/^table\s*/i, "");
  const name = readDotted(rest, 0);
  if (!name || name === "composite") {
    st.schema.skipped += 1;
    return SKIP;
  }
  const after = rest.slice(name.end);
  const table: DbmlTable = {
    name: name.parts.join("."),
    note: readSettings(splitSettings(after)).note,
    columns: [],
  };
  const alias = /^\s+as\s+("[^"]*"|\S+)/i.exec(after);
  const aliasName = alias?.[1];
  if (aliasName !== undefined) st.aliases.set(unquote(aliasName), table.name);
  st.schema.tables.push(table);
  return { kind: "table", table };
}

function openGroup(st: State, header: string): Block {
  const name = readPart(header.replace(/^tablegroup\s*/i, ""), 0);
  if (!name) {
    st.schema.skipped += 1;
    return SKIP;
  }
  const group: DbmlGroup = { name: name.value, tables: [] };
  st.schema.groups.push(group);
  return { kind: "group", group };
}

// The block a header line opens at the top of the file.
function openTopBlock(st: State, header: string): Block {
  const word = (/^[A-Za-z]+/.exec(header)?.[0] ?? "").toLowerCase();
  switch (word) {
    case "table":
      return openTable(st, header);
    case "tablegroup":
      return openGroup(st, header);
    case "ref":
      return { kind: "ref" };
    case "project":
    case "enum":
    case "tablepartial":
    case "note":
      return SKIP;
    default:
      st.schema.skipped += 1;
      return SKIP;
  }
}

function openBlock(st: State, header: string) {
  const top = st.stack[st.stack.length - 1];
  if (!top) st.stack.push(openTopBlock(st, header));
  else if (top.kind === "table" && /^note$/i.test(header))
    st.stack.push({ kind: "tablenote", table: top.table });
  else st.stack.push(SKIP);
}

function addInlineRef(st: State, table: DbmlTable, column: DbmlColumn, text: string) {
  const self = { table: table.name, column: column.name };
  const ref = readRef(text, self);
  if (!ref) {
    st.schema.skipped += 1;
    return;
  }
  st.schema.refs.push(ref);
  column.ref = ref.from.table === self.table && ref.from.column === self.column ? ref.to : ref.from;
}

function readColumn(st: State, table: DbmlTable, line: string) {
  const name = readPart(line, 0);
  const start = name ? skipSpaces(line, name.end) : 0;
  const end = typeEnd(line, start);
  if (!name || start === name.end || end === start) {
    st.schema.skipped += 1;
    return;
  }
  const settings = readSettings(splitSettings(line.slice(end)));
  const column: DbmlColumn = {
    name: name.value,
    type: unquote(line.slice(start, end)),
    pk: settings.pk,
    notNull: settings.notNull,
    unique: settings.unique,
    note: settings.note,
    ref: null,
  };
  table.columns.push(column);
  if (settings.ref !== null) addInlineRef(st, table, column, settings.ref);
}

function readTableLine(st: State, table: DbmlTable, line: string) {
  if (/^note\s*:/i.test(line)) table.note = unquote(line.slice(line.indexOf(":") + 1));
  else readColumn(st, table, line);
}

function readGroupLine(st: State, group: DbmlGroup, line: string) {
  if (/^note\s*:/i.test(line)) return;
  let i = 0;
  while (skipSpaces(line, i) < line.length) {
    const name = readDotted(line, i);
    if (!name || name === "composite") {
      st.schema.skipped += 1;
      return;
    }
    group.tables.push(name.parts.join("."));
    i = name.end;
  }
}

function readRefLine(st: State, line: string) {
  const ref = readRef(line, null);
  if (ref) st.schema.refs.push(ref);
  else st.schema.skipped += 1;
}

// A line at the top of the file that is not a block header: only
// `Ref name: a.b > c.d` is one the reader knows.
function readTopLine(st: State, line: string) {
  const m = /^ref\b[^:]*:(.*)$/i.exec(line);
  const rest = m?.[1];
  if (rest !== undefined) readRefLine(st, rest);
  else st.schema.skipped += 1;
}

function readBlockLine(st: State, block: Block, line: string) {
  switch (block.kind) {
    case "table":
      readTableLine(st, block.table, line);
      break;
    case "tablenote":
      block.table.note = unquote(line);
      break;
    case "ref":
      readRefLine(st, line);
      break;
    case "group":
      readGroupLine(st, block.group, line);
      break;
    case "skip":
      break;
    default: {
      const unknown: never = block;
      throw new Error(`no reader for the block ${String(unknown)}`);
    }
  }
}

function readLine(st: State, line: string) {
  if (line === "}") {
    if (st.stack.pop() === undefined) st.schema.skipped += 1;
    return;
  }
  if (line.endsWith("{")) {
    openBlock(st, line.slice(0, -1).trim());
    return;
  }
  const top = st.stack[st.stack.length - 1];
  if (top) readBlockLine(st, top, line);
  else readTopLine(st, line);
}

// A table may be named by its alias in a ref or a group.
function resolveAliases(st: State) {
  if (st.aliases.size === 0) return;
  const name = (n: string) => st.aliases.get(n) ?? n;
  for (const ref of st.schema.refs) {
    ref.from.table = name(ref.from.table);
    ref.to.table = name(ref.to.table);
  }
  for (const group of st.schema.groups) group.tables = group.tables.map(name);
}

/** Read the text of a `.dbml` file. It never throws. */
export function parseDbml(text: string): DbmlSchema {
  const st: State = {
    schema: { tables: [], refs: [], groups: [], skipped: 0 },
    stack: [],
    aliases: new Map(),
  };
  for (const line of toLines(text)) {
    try {
      readLine(st, line);
    } catch {
      // A reader fault on one line counts like a line it did not understand.
      st.schema.skipped += 1;
    }
  }
  resolveAliases(st);
  return st.schema;
}

// ---- navigation -------------------------------------------------------------------

/** The key of one column in the sets that `matchSchema` and `sameColumns` return. */
export function colKey(table: string, column: string) {
  return `${table}\u001f${column}`;
}

/**
 * What a Find term matches, case-insensitively and as a substring: a table by
 * its own name or by any of its column names, and each column by its name.
 * An empty term matches every table and no column.
 */
export function matchSchema(
  schema: DbmlSchema,
  term: string,
): { tables: Set<string>; columns: Set<string> } {
  const needle = term.toLowerCase();
  if (needle === "")
    return { tables: new Set(schema.tables.map((t) => t.name)), columns: new Set() };
  const tables = new Set<string>();
  const columns = new Set<string>();
  for (const table of schema.tables) {
    const found = table.columns.filter((c) => c.name.toLowerCase().includes(needle));
    for (const column of found) columns.add(colKey(table.name, column.name));
    if (found.length > 0 || table.name.toLowerCase().includes(needle)) tables.add(table.name);
  }
  return { tables, columns };
}

// The name without its prefix, the text up to and including the first `_`.
// Null when the name has no `_`, or nothing after it.
function withoutPrefix(name: string) {
  const at = name.indexOf("_");
  return at < 0 || at === name.length - 1 ? null : name.slice(at + 1).toLowerCase();
}

// True when `c` has the clicked name, or `bare` is its name without the prefix.
function sameName(c: DbmlColumn, column: string, bare: string | null) {
  if (c.name.toLowerCase() === column.toLowerCase()) return true;
  return bare !== null && !c.pk && withoutPrefix(c.name) === bare;
}

/**
 * The columns that the click on `table.column` marks, as `colKey`s: the other
 * columns with the same name, and, when neither side is a key, the columns
 * whose name without its prefix is the same (`ab_nb_owner` and `cd_nb_owner`).
 * The prefix rule is a guess about legacy names, so it never reaches a key
 * column: every table has its own `id`-like key.
 */
export function sameColumns(schema: DbmlSchema, table: string, column: string): Set<string> {
  const self = colKey(table, column);
  const clicked = schema.tables
    .find((t) => t.name === table)
    ?.columns.find((c) => c.name === column);
  const bare = clicked && !clicked.pk ? withoutPrefix(column) : null;
  const same = new Set<string>();
  for (const t of schema.tables) {
    for (const c of t.columns.filter((x) => sameName(x, column, bare)))
      same.add(colKey(t.name, c.name));
  }
  same.delete(self);
  return same;
}

/** The columns a closed card shows. */
export const COLLAPSED_COLUMNS = 8;

/**
 * The columns of a card, in file order. A closed card of more than
 * COLLAPSED_COLUMNS columns shows every key column and, with the room that is
 * left, the first other columns.
 */
export function shownColumns(table: DbmlTable, open: boolean): DbmlColumn[] {
  if (open || table.columns.length <= COLLAPSED_COLUMNS) return table.columns;
  let room = COLLAPSED_COLUMNS - table.columns.filter((c) => c.pk).length;
  return table.columns.filter((c) => c.pk || room-- > 0);
}

/** The refs that start in `table`, and the refs that end in it. */
export function refsOf(schema: DbmlSchema, table: string): { out: DbmlRef[]; in: DbmlRef[] } {
  return {
    out: schema.refs.filter((r) => r.from.table === table),
    in: schema.refs.filter((r) => r.to.table === table),
  };
}

// ---- drawing a selection -----------------------------------------------------------

/**
 * The most tables one diagram draws. Mermaid places tables that have no
 * relation in one row; on a measured 79-table schema, about 12 tables are the
 * most that the zoom view (4 times at most) still shows readable.
 */
export const DRAW_LIMIT = 12;

// A word of the `erDiagram` grammar: space is removed, any character outside
// `A-Za-z0-9_()[]-` becomes `_`, and the word starts with a letter or `_`.
function mermaidWord(raw: string) {
  const word = raw.replace(/\s+/g, "").replace(/[^A-Za-z0-9_()[\]-]/g, "_");
  return /^[A-Za-z_]/.test(word) ? word : `_${word}`;
}

function entityLines(id: string, table: DbmlTable) {
  const label = table.name.replace(/"/g, "'");
  if (table.columns.length === 0) return [`  ${id}["${label}"]`];
  const rows = table.columns.map(
    (c) => `    ${mermaidWord(c.type)} ${mermaidWord(c.name)}${c.pk ? " PK" : ""}`,
  );
  return [`  ${id}["${label}"] {`, ...rows, "  }"];
}

const CARDINALITY: Record<DbmlRefKind, string> = {
  "many-to-one": "}o--||",
  "one-to-one": "||--||",
  "many-to-many": "}o--o{",
};

/**
 * The Mermaid `erDiagram` source that draws the tables `names`, in that order,
 * and the refs whose two ends are both among them. A name the file does not
 * have, or a repeated name, adds no entity. Entities have the ids `t0`, `t1`…
 * and their real names as quoted labels, so no name reaches the grammar.
 */
export function toMermaidEr(schema: DbmlSchema, names: string[]): string {
  const byName = new Map(schema.tables.map((t) => [t.name, t]));
  const picked = [...new Set(names)].flatMap((n) => byName.get(n) ?? []);
  const ids = new Map(picked.map((t, i) => [t.name, `t${i}`]));
  const lines = ["erDiagram"];
  picked.forEach((table, i) => lines.push(...entityLines(`t${i}`, table)));
  for (const ref of schema.refs) {
    const from = ids.get(ref.from.table);
    const to = ids.get(ref.to.table);
    if (from && to)
      lines.push(`  ${from} ${CARDINALITY[ref.kind]} ${to} : "${mermaidWord(ref.from.column)}"`);
  }
  return lines.join("\n");
}
