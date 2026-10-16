// Unit tests for assets/ui/wb-dbml.ts — the reader of the DBML subset the
// workbench shows, and the grouping of its tables. The two fixtures are
// synthetic: `legacy-shape.dbml` has 14 tables and no relations;
// `mixed.dbml` holds every construct the parser reads, plus garbage.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { groupTables, parseDbml } from "../assets/ui/wb-dbml.ts";

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
