/* ---------------------------------------------------------------------------
   The model of a `.dbml` file: the types of the subset of DBML the workbench
   shows, and what the view asks of it. Pure functions: no DOM, no globals.
   The reader that makes the model from the text is wb-dbml-read.ts.
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

export { parseDbml } from "./wb-dbml-read.ts";

// ---- navigation -------------------------------------------------------------------

/** The key of one column in the sets that `matchSchema` and `sameColumns` return. */
export function colKey(table: string, column: string) {
  return `${table}\u001F${column}`;
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
  if (needle === "") {
    return { tables: new Set(schema.tables.map((t) => t.name)), columns: new Set() };
  }
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
    for (const c of t.columns.filter((x) => sameName(x, column, bare))) {
      same.add(colKey(t.name, c.name));
    }
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
  const word = raw
    .replaceAll(/\s+/gu, "")
    // One `_` per UTF-16 unit, as the pattern had no `u` flag.
    .replaceAll(/[^A-Za-z0-9_()[\]-]/gu, (c) => "_".repeat(c.length));
  return /^[A-Za-z_]/.test(word) ? word : `_${word}`;
}

function entityLines(id: string, table: DbmlTable) {
  const label = table.name.replaceAll('"', "'");
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
  picked.forEach((table, i) => {
    lines.push(...entityLines(`t${i}`, table));
  });
  for (const ref of schema.refs) {
    const from = ids.get(ref.from.table);
    const to = ids.get(ref.to.table);
    if (from !== undefined && to !== undefined) {
      lines.push(`  ${from} ${CARDINALITY[ref.kind]} ${to} : "${mermaidWord(ref.from.column)}"`);
    }
  }
  return lines.join("\n");
}
