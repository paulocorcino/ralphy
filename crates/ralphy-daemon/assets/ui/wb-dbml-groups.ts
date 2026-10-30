/* ---------------------------------------------------------------------------
   The grouping of the tables of a `.dbml` file (`wb-dbml.ts`): the groups the
   file declares, then the tables that share a name prefix. Pure functions: no
   DOM, no globals.
   --------------------------------------------------------------------------- */

import type { DbmlSchema, DbmlTable } from "./wb-dbml.ts";

/** The label of the group that holds the tables no other rule placed. */
export const OTHER_TABLES = "Other tables";

/** The shortest shared prefix, in characters, that puts two names in one group. */
const MIN_PREFIX = 4;

const SEPARATOR = /[_\-.]/;

// Compare as lower case by code unit: the same order on every machine.
function byLower(a: string, b: string) {
  const x = a.toLowerCase();
  const y = b.toLowerCase();
  if (x === y) return 0;
  return x < y ? -1 : 1;
}

function prefixLength(a: string, b: string) {
  const x = a.toLowerCase();
  const y = b.toLowerCase();
  let n = 0;
  while (n < x.length && n < y.length && x[n] === y[n]) n += 1;
  return n;
}

// Neighbours of the sorted names that share a prefix of MIN_PREFIX or more.
function runsOf(sorted: DbmlTable[]): DbmlTable[][] {
  const runs: DbmlTable[][] = [];
  for (const table of sorted) {
    const run = runs.at(-1);
    const last = run?.at(-1);
    if (run && last && prefixLength(last.name, table.name) >= MIN_PREFIX) run.push(table);
    else runs.push([table]);
  }
  return runs;
}

// True when the label stops inside a word of some name: no table has exactly
// that name, and the character after it is not the start of a new word.
function endsInsideWord(label: string, names: string[]) {
  if (names.some((n) => n.toLowerCase() === label.toLowerCase())) return false;
  return !names.every((n) => /^[\p{Lu}\d_\-.]/u.test(n.slice(label.length)));
}

// The label of a run of names: their shared prefix, cut after its last
// separator, so `BOT_GROUP` and `BOT_FILES` give `BOT`.
function labelOf(names: string[]) {
  const [first = ""] = names;
  const length = Math.min(...names.map((n) => prefixLength(first, n)));
  const prefix = first.slice(0, length);
  let cut = prefix;
  if (SEPARATOR.test(prefix) && !SEPARATOR.test(prefix.slice(-1))) {
    cut = prefix.slice(
      0,
      Math.max(prefix.lastIndexOf("_"), prefix.lastIndexOf("-"), prefix.lastIndexOf(".")) + 1,
    );
  }
  const label = cut.replace(/[_\-.]+$/, "") || prefix;
  return endsInsideWord(label, names) ? `${label}…` : label;
}

function addTo(buckets: Map<string, DbmlTable[]>, label: string, tables: DbmlTable[]) {
  const list = buckets.get(label);
  if (list) list.push(...tables);
  else buckets.set(label, [...tables]);
}

function byLabel(a: { label: string }, b: { label: string }) {
  if (a.label === OTHER_TABLES || b.label === OTHER_TABLES)
    return Number(a.label === OTHER_TABLES) - Number(b.label === OTHER_TABLES);
  return byLower(a.label, b.label);
}

/**
 * The tables in groups: a `TableGroup` of the file first, then the tables
 * that share a name prefix, then `Other tables`. Groups and the tables in
 * them are in name order.
 */
export function groupTables(schema: DbmlSchema): { label: string; tables: DbmlTable[] }[] {
  const byName = new Map(schema.tables.map((t) => [t.name, t]));
  const buckets = new Map<string, DbmlTable[]>();
  const taken = new Set<string>();
  for (const group of schema.groups) {
    for (const name of group.tables) {
      const table = byName.get(name);
      if (!table || taken.has(name)) continue;
      taken.add(name);
      addTo(buckets, group.name, [table]);
    }
  }
  const rest = schema.tables
    .filter((t) => !taken.has(t.name))
    .sort((a, b) => byLower(a.name, b.name));
  for (const run of runsOf(rest)) {
    addTo(buckets, run.length > 1 ? labelOf(run.map((t) => t.name)) : OTHER_TABLES, run);
  }
  return [...buckets]
    .map(([label, tables]) => ({ label, tables: tables.sort((a, b) => byLower(a.name, b.name)) }))
    .sort(byLabel);
}
