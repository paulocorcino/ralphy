// Unit tests for assets/ui/wb-dbml.ts — the reader of the DBML subset the
// workbench shows, and the grouping of its tables. The two fixtures are
// synthetic: `legacy-shape.dbml` has 14 tables and no relations;
// `mixed.dbml` holds every construct the parser reads, plus garbage.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { colKey, matchSchema, parseDbml, refsOf, sameColumns, shownColumns, toMermaidEr } from "../assets/ui/wb-dbml.ts";
import { groupTables } from "../assets/ui/wb-dbml-groups.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const fixture = (name) => readFileSync(join(HERE, "fixtures", "dbml", name), "utf8");

const col = (name, type, over = {}) => ({
  name,
  type,
  pk: false,
  notNull: false,
  unique: false,
  note: null,
  ref: null,
  ...over,
});

test("parseDbml reads tables, columns and their settings", () => {
  const schema = parseDbml(fixture("legacy-shape.dbml"));
  assert.equal(schema.tables.length, 14);
  assert.deepEqual(schema.refs, []);
  assert.equal(schema.skipped, 0);
  const bank = schema.tables.find((t) => t.name === "bank");
  assert.deepEqual(bank, {
    name: "bank",
    note: null,
    columns: [
      col("id", "int", { pk: true, notNull: true }),
      col("code", "char(3)", { notNull: true }),
      col("name", "varchar(60)"),
      col("opened", "date"),
      col("limit_amount", "decimal(10, 2)", { notNull: true }),
    ],
  });
  const user = schema.tables.find((t) => t.name === "user");
  assert.deepEqual(user.columns[1], col("login", "varchar(40)", { notNull: true, unique: true }));
  // A composite key is two pk columns in one table.
  const service = schema.tables.find((t) => t.name === "BOT_GROUP_SERVICE");
  assert.deepEqual(service.columns.map((c) => c.pk), [true, true]);
});

test("parseDbml skips the blocks it does not show", () => {
  const schema = parseDbml(fixture("mixed.dbml"));
  // Project, Enum, TablePartial and the table inside the block comment add no table.
  assert.deepEqual(
    schema.tables.map((t) => t.name),
    ["users", "order items", "sales.invoices", "orders"],
  );
  const [users, items, invoices] = schema.tables;
  // `indexes { … }` adds no column.
  assert.deepEqual(
    users.columns.map((c) => c.name),
    ["id", "email", "nickname", "status"],
  );
  assert.deepEqual(users.columns[0], col("id", "integer", { pk: true }));
  assert.deepEqual(users.columns[1], col("email", "varchar(255)", { unique: true, notNull: true, note: "login address" }));
  assert.deepEqual(users.columns[3], col("status", "status"));
  assert.equal(users.note, "Accounts of the shop");
  assert.equal(items.note, "A note in a block");
  assert.deepEqual(
    items.columns.map((c) => c.name),
    ["id", "user_id", "price"],
  );
  assert.equal(items.columns[2].type, "decimal(10, 2)");
  assert.equal(invoices.columns[2].type, "double precision");
  assert.deepEqual(schema.groups, [{ name: "shop", tables: ["users", "orders"] }]);
});

test("parseDbml reads refs and never throws", () => {
  const schema = parseDbml(fixture("mixed.dbml"));
  const ep = (table, column) => ({ table, column });
  assert.deepEqual(schema.refs, [
    { from: ep("order items", "user_id"), to: ep("users", "id"), kind: "many-to-one" },
    { from: ep("orders", "user_id"), to: ep("users", "id"), kind: "many-to-one" },
    { from: ep("sales.invoices", "item_id"), to: ep("order items", "id"), kind: "many-to-one" },
    { from: ep("orders", "id"), to: ep("users", "id"), kind: "one-to-one" },
  ]);
  // The inline ref keeps the other side on the column.
  assert.deepEqual(schema.tables[1].columns[1].ref, ep("users", "id"));
  // The composite ref and the garbage line.
  assert.equal(schema.skipped, 2);

  // `<` is written the other way round; `<>` is many-to-many; `s.t.c` keeps `s.t`.
  const kinds = parseDbml("Ref: a.x < b.y\nRef: s.t.c <> b.y\n");
  assert.deepEqual(kinds.refs, [
    { from: ep("b", "y"), to: ep("a", "x"), kind: "many-to-one" },
    { from: ep("s.t", "c"), to: ep("b", "y"), kind: "many-to-many" },
  ]);

  // A string over several lines is one value.
  const note = parseDbml("Table t {\n  Note: '''first\n  second'''\n  id int\n}\n");
  assert.equal(note.tables[0].note, "first second");
  assert.equal(note.tables[0].columns.length, 1);

  const broken = [
    "Table t {\n  id int\n",
    "}\n}\n",
    "",
    "\u0000\u0001\u0002 �� binary-like \u007f}{][)(",
    `Table ${"x".repeat(1000)} {\n  ${"c".repeat(1000)} ${"t".repeat(1000)} [${"n".repeat(1000)}\n}\n`,
    "// only a comment\n/* and another */\n",
  ];
  for (const text of broken) assert.doesNotThrow(() => parseDbml(text), JSON.stringify(text.slice(0, 20)));
  // An unclosed brace keeps the table it opened.
  assert.equal(parseDbml(broken[0]).tables[0].columns.length, 1);
  // A lone `}` is a line it did not understand.
  assert.equal(parseDbml(broken[1]).skipped, 2);
  assert.deepEqual(parseDbml(broken[5]).tables, []);
});

test("groupTables groups tables by a common name prefix", () => {
  const groups = groupTables(parseDbml(fixture("legacy-shape.dbml")));
  assert.deepEqual(
    groups.map((g) => ({ label: g.label, tables: g.tables.map((t) => t.name) })),
    [
      { label: "approval…", tables: ["approvalboss", "approvaldesk"] },
      { label: "BOT", tables: ["BOT_FILES", "BOT_GROUP", "BOT_GROUP_SERVICE"] },
      { label: "lots", tables: ["lotsRefund", "lotsRefund_20241220", "lotsStock"] },
      { label: "title", tables: ["title", "title_20241220", "titleItem"] },
      { label: "Other tables", tables: ["bank", "city", "user"] },
    ],
  );
});

test("groupTables puts a TableGroup first", () => {
  const text = "Table shop_a { id int }\nTable shop_b { id int }\nTable shop_c { id int }\nTableGroup core {\n  shop_a\n}\n";
  const groups = groupTables(parseDbml(text));
  assert.deepEqual(
    groups.map((g) => ({ label: g.label, tables: g.tables.map((t) => t.name) })),
    [
      { label: "core", tables: ["shop_a"] },
      { label: "shop", tables: ["shop_b", "shop_c"] },
    ],
  );
  // Without the TableGroup the three tables share one group.
  const plain = groupTables(parseDbml(text.split("TableGroup")[0]));
  assert.deepEqual(plain.map((g) => g.label), ["shop"]);
  // A TableGroup that names no table of the file makes no group.
  assert.deepEqual(groupTables(parseDbml("Table a { id int }\nTableGroup g {\n  nothing\n}\n")).map((g) => g.label), ["Other tables"]);
});

// A synthetic schema for the navigation rules.
const NAV = parseDbml(`
Table shop_orders {
  id int [pk]
  buyer varchar(40)
  Note: 'x'
}
Table shop_items {
  id int [pk]
  Total int
  order_id int [ref: > shop_orders.id]
}
Table audit {
  id int
}
`);

test("matchSchema finds tables by table name and by column name", () => {
  const keys = (...pairs) => new Set(pairs.map(([t, c]) => colKey(t, c)));
  // A table-name term: the tables only, no column.
  let m = matchSchema(NAV, "shop");
  assert.deepEqual(m.tables, new Set(["shop_orders", "shop_items"]));
  assert.deepEqual(m.columns, new Set());
  // A column-name term: the table holding the column, and the column.
  m = matchSchema(NAV, "buyer");
  assert.deepEqual(m.tables, new Set(["shop_orders"]));
  assert.deepEqual(m.columns, keys(["shop_orders", "buyer"]));
  // A different case matches (`Total` is written with a capital).
  m = matchSchema(NAV, "TOTAL");
  assert.deepEqual(m.tables, new Set(["shop_items"]));
  assert.deepEqual(m.columns, keys(["shop_items", "Total"]));
  // An empty term matches every table and no column.
  m = matchSchema(NAV, "");
  assert.deepEqual(m.tables, new Set(["shop_orders", "shop_items", "audit"]));
  assert.deepEqual(m.columns, new Set());
  // A term that matches nothing.
  m = matchSchema(NAV, "zzz");
  assert.deepEqual(m, { tables: new Set(), columns: new Set() });
  // A column in two tables marks both columns.
  assert.deepEqual(matchSchema(NAV, "id").columns, keys(["shop_orders", "id"], ["shop_items", "id"], ["shop_items", "order_id"], ["audit", "id"]));
});

test("sameColumns matches the exact name and the name without its prefix", () => {
  const schema = parseDbml(`
Table t1 { ab_nb_owner int
  ab_nb_id int [pk]
  code int
  plain int }
Table t2 { cd_nb_owner int
  cd_nb_id int [pk]
  CODE int
  Plain int
  ab_nb_id int }
Table t3 { ab_nb_owner int
  ef_nb_id int
  x_ int }
Table t4 { y_ int
  ef_nb_owner int [pk] }
`);
  const set = (...pairs) => new Set(pairs.map(([t, c]) => colKey(t, c)));
  // The same name under another prefix, and the exact name in another table.
  assert.deepEqual(sameColumns(schema, "t1", "ab_nb_owner"), set(["t2", "cd_nb_owner"], ["t3", "ab_nb_owner"]));
  // A pk never matches by its name without the prefix, on either side (`t4.ef_nb_owner`
  // is a pk; `t3.ef_nb_id` is not), but an exact name still does.
  assert.deepEqual(sameColumns(schema, "t1", "ab_nb_id"), set(["t2", "ab_nb_id"]));
  assert.deepEqual(sameColumns(schema, "t2", "cd_nb_id"), set());
  // A name with no `_` matches the exact name only, in any case.
  assert.deepEqual(sameColumns(schema, "t1", "code"), set(["t2", "CODE"]));
  // A name with nothing after its `_` has no prefix form: the exact name only.
  assert.deepEqual(sameColumns(schema, "t3", "x_"), set());
  // The clicked column is never in the set.
  assert.equal(sameColumns(schema, "t1", "plain").has(colKey("t1", "plain")), false);
});

test("shownColumns keeps the key columns and the first 8 columns of a closed card", () => {
  const names = (t, open) => shownColumns(t, open).map((c) => c.name);
  const cols = Array.from({ length: 12 }, (_, i) => `c${i + 1}`);
  const table = (lines) => parseDbml(["Table t {", ...lines, "}"].join("\n")).tables[0];
  const text = (n, pkAt) => cols.slice(0, n).map((c, i) => `  ${c} int${i + 1 === pkAt ? " [pk]" : ""}`);
  const twelve = table(text(12, 5));
  assert.deepEqual(names(twelve, false), ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8"]);
  // A key past the eighth column stays, and a non-key column leaves room for it.
  const late = table(text(12, 11));
  assert.deepEqual(names(late, false), ["c1", "c2", "c3", "c4", "c5", "c6", "c7", "c11"]);
  assert.equal(names(twelve, true).length, 12);
  const five = table(text(5, 0));
  assert.equal(names(five, false).length, 5);
  // Every key column is kept, even over 8 of them.
  const keys = table([...cols.slice(0, 10).map((c) => `  ${c} int [pk]`), "  extra int"]);
  assert.equal(names(keys, false).length, 10);
});

test("refsOf lists the links out of a table and into it", () => {
  const schema = parseDbml(
    ["Table a { id int }", "Table b { a_id int [ref: > a.id] }", "Table c { a_id int }", "Ref: c.a_id > a.id", "Ref: a.id - c.a_id"].join("\n"),
  );
  const from = (r) => `${r.from.table}.${r.from.column}`;
  assert.deepEqual(refsOf(schema, "a").out.map(from), ["a.id"]);
  assert.deepEqual(refsOf(schema, "a").in.map(from), ["b.a_id", "c.a_id"]);
  assert.deepEqual(refsOf(schema, "b").out.map(from), ["b.a_id"]);
  assert.deepEqual(refsOf(schema, "b").in, []);
  assert.deepEqual(refsOf(schema, "c").out.map(from), ["c.a_id"]);
  assert.deepEqual(refsOf(schema, "c").in.map(from), ["a.id"]);
  assert.deepEqual(refsOf(schema, "nothing"), { out: [], in: [] });
});

// Three tables joined three ways: many-to-one (`order items` to `users`),
// one-to-one (`users` to `c`) and many-to-many (`order items` to `c`).
const FK_SCHEMA = [
  "Table users {",
  "  id integer [pk]",
  "  email varchar(255)",
  "}",
  'Table "order items" {',
  "  id integer [pk]",
  "  user_id integer",
  "}",
  "Table c { id int [pk] }",
  'Ref: "order items".user_id > users.id',
  "Ref: users.id - c.id",
  'Ref: "order items".id <> c.id',
].join("\n");

test("toMermaidEr draws the selected tables and the refs between them", () => {
  const schema = parseDbml(FK_SCHEMA);
  assert.equal(
    toMermaidEr(schema, ["order items", "users"]),
    [
      "erDiagram",
      '  t0["order items"] {',
      "    integer id PK",
      "    integer user_id",
      "  }",
      '  t1["users"] {',
      "    integer id PK",
      "    varchar(255) email",
      "  }",
      '  t0 }o--|| t1 : "user_id"',
    ].join("\n"),
  );
  // The order given is the order of the entities.
  assert.match(toMermaidEr(schema, ["users", "order items"]), /^erDiagram\n {2}t0\["users"\] \{/);
});

test("toMermaidEr keeps only the refs inside the selection", () => {
  const schema = parseDbml(FK_SCHEMA);
  const lines = (names) =>
    toMermaidEr(schema, names)
      .split("\n")
      .filter((l) => l.includes("--"));
  assert.deepEqual(lines(["order items", "users"]), ['  t0 }o--|| t1 : "user_id"']);
  assert.deepEqual(lines(["users", "c"]), ['  t0 ||--|| t1 : "id"']);
  assert.deepEqual(lines(["order items", "c"]), ['  t0 }o--o{ t1 : "id"']);
  // A ref with one end outside the selection adds no line, and a name the
  // file does not have adds no entity.
  assert.deepEqual(lines(["users"]), []);
  assert.equal(toMermaidEr(schema, ["users", "ghost"]).includes("t1"), false);
  assert.equal(toMermaidEr(schema, []), "erDiagram");
});

test("toMermaidEr makes names and types safe for Mermaid", () => {
  const table = (name, columns) => ({ name, note: null, columns });
  const schema = {
    tables: [
      table("order items", [col("1st col", "DECIMAL(10, 2)")]),
      table('say "hi"', [col("naïve", "double precision")]),
      table("<img src=x onerror=alert(1)>", [col("a-b", "int[]")]),
    ],
    refs: [{ from: { table: "order items", column: "1st col" }, to: { table: 'say "hi"', column: "x y" }, kind: "many-to-one" }],
    groups: [],
    skipped: 0,
  };
  assert.equal(
    toMermaidEr(
      schema,
      schema.tables.map((t) => t.name),
    ),
    [
      "erDiagram",
      '  t0["order items"] {',
      "    DECIMAL(10_2) _1stcol",
      "  }",
      "  t1[\"say 'hi'\"] {",
      "    doubleprecision na_ve",
      "  }",
      '  t2["<img src=x onerror=alert(1)>"] {',
      "    int[] a-b",
      "  }",
      '  t0 }o--|| t1 : "_1stcol"',
    ].join("\n"),
  );
});
