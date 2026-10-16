/* ---------------------------------------------------------------------------
   The body of a `.dbml` tab: a grid of table cards, in groups.

   The file is repo content, so every node is made with `createElement` and
   `textContent`. The headings are what the pane's side index reads: an `h2`
   per group and an `h3` per table.
   --------------------------------------------------------------------------- */

import { groupTables, parseDbml } from "./wb-dbml.ts";
import type { DbmlColumn, DbmlTable } from "./wb-dbml.ts";

function plural(n: number, one: string, many: string) {
  return `${n} ${n === 1 ? one : many}`;
}

function el(doc: Document, tag: string, className: string, text?: string) {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// Only the exceptions are tagged: `not null` is on nearly every column of a
// real schema (1526 of 1604 columns in a measured file), so a tag on it would
// be noise on every row.
function tagsOf(column: DbmlColumn) {
  const tags: string[] = [];
  if (!column.pk && !column.notNull) tags.push("null");
  if (column.unique) tags.push("unique");
  return tags;
}

function columnRow(doc: Document, column: DbmlColumn) {
  const row = el(doc, "tr", "dbml-col");
  row.dataset.col = column.name;
  const name = el(doc, "td", "dbml-col-name");
  name.title = column.note ? `${column.name}: ${column.note}` : column.name;
  if (column.pk) name.append(el(doc, "i", "bi bi-key"), " ");
  name.append(column.name);
  const type = el(doc, "td", "dbml-col-type");
  type.append(el(doc, "span", "dbml-type", column.type));
  for (const tag of tagsOf(column)) type.append(el(doc, "span", "dbml-tag", tag));
  row.append(name, type);
  return row;
}

function card(doc: Document, table: DbmlTable) {
  const box = el(doc, "div", "dbml-card");
  box.dataset.table = table.name;
  box.append(el(doc, "h3", "", table.name));
  if (table.note) box.append(el(doc, "p", "dbml-note", table.note));
  const cols = el(doc, "table", "dbml-cols");
  const body = el(doc, "tbody", "");
  for (const column of table.columns) body.append(columnRow(doc, column));
  cols.append(body);
  box.append(cols);
  return box;
}

/** Fill `article` with the table cards of the DBML `text`. */
export function renderDbmlGrid(doc: Document, article: Element, text: string): void {
  article.replaceChildren();
  const schema = parseDbml(text);
  if (schema.tables.length === 0) {
    article.append(el(doc, "p", "dbml-empty", "No tables found in this file."));
    return;
  }
  const groups = groupTables(schema);
  let summary = `${plural(schema.tables.length, "table", "tables")} in ${plural(groups.length, "group", "groups")}.`;
  if (schema.skipped > 0) summary += ` ${plural(schema.skipped, "line was", "lines were")} not understood.`;
  article.append(el(doc, "div", "dbml-summary", summary));
  for (const group of groups) {
    const section = el(doc, "section", "dbml-group");
    section.append(el(doc, "h2", "", group.label));
    const grid = el(doc, "div", "dbml-grid");
    for (const table of group.tables) grid.append(card(doc, table));
    section.append(grid);
    article.append(section);
  }
}
