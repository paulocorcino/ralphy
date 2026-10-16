/* ---------------------------------------------------------------------------
   The body of a `.dbml` tab: a grid of table cards, in groups.

   The file is repo content, so every node is made with `createElement` and
   `textContent`. The headings are what the pane's side index reads: an `h2`
   per group and an `h3` per table.

   Navigation: a card with more than COLLAPSED_COLUMNS columns is closed (its
   other rows carry `hidden`) until its button opens it; a click on a column
   name marks the columns with the same name; a ref is a link to its table.
   The links are plain `#fragment` anchors: the viewer's `linkClick` moves the
   view to the heading, so this module adds no listener for them.
   --------------------------------------------------------------------------- */

import { colKey, groupTables, matchSchema, parseDbml, refsOf, sameColumns, shownColumns } from "./wb-dbml.ts";
import type { DbmlColumn, DbmlEndpoint, DbmlRef, DbmlSchema, DbmlTable } from "./wb-dbml.ts";
// The two modules import each other; both use the other only inside a function
// that runs after both have loaded.
import { slugOf } from "./wb-viewer-render.ts";

type Ctx = { doc: Document; article: Element; schema: DbmlSchema; names: Set<string> };

// What the last render put in an article, for `filterDbml`.
const schemas = new WeakMap<Element, DbmlSchema>();
const tableOfCard = new WeakMap<Element, DbmlTable>();

function plural(n: number, one: string, many: string) {
  return `${n} ${n === 1 ? one : many}`;
}

function el(doc: Document, tag: string, className: string, text?: string) {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(doc: Document, className: string, text: string) {
  const node = el(doc, "button", className, text) as HTMLButtonElement;
  node.type = "button";
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

// A link to the card of `ep.table`, or plain text when the file has no such table.
function refNode(ctx: Ctx, ep: DbmlEndpoint, arrow: string): HTMLElement {
  const text = `${arrow}${ep.table}.${ep.column}`;
  if (!ctx.names.has(ep.table)) return el(ctx.doc, "span", "dbml-ref", text);
  const link = el(ctx.doc, "a", "dbml-ref", text) as HTMLAnchorElement;
  link.href = `#${slugOf(ep.table)}`;
  link.title = text;
  return link;
}

function columnRow(ctx: Ctx, column: DbmlColumn, out: DbmlRef[]) {
  const { doc } = ctx;
  const row = el(doc, "tr", "dbml-col");
  row.dataset.col = column.name;
  const name = el(doc, "td", "dbml-col-name");
  name.title = column.note ? `${column.name}: ${column.note}` : column.name;
  if (column.pk) name.append(el(doc, "i", "bi bi-key"), " ");
  name.append(column.name);
  for (const ref of out) if (ref.from.column === column.name) name.append(refNode(ctx, ref.to, "→ "));
  const type = el(doc, "td", "dbml-col-type");
  type.append(el(doc, "span", "dbml-type", column.type));
  for (const tag of tagsOf(column)) type.append(el(doc, "span", "dbml-tag", tag));
  row.append(name, type);
  return row;
}

function incoming(ctx: Ctx, refs: DbmlRef[]) {
  const box = el(ctx.doc, "p", "dbml-in", "Referenced by:");
  for (const ref of refs) box.append(" ", refNode(ctx, ref.from, ""));
  return box;
}

// Show or hide the other rows of a card, and say what its button does next.
function setOpen(card: HTMLElement, open: boolean) {
  const table = tableOfCard.get(card);
  if (!table) return;
  const shown = new Set(shownColumns(table, open));
  const rows = card.querySelectorAll<HTMLElement>("tr.dbml-col");
  table.columns.forEach((column, i) => {
    rows[i].hidden = !shown.has(column);
  });
  card.classList.toggle("dbml-open", open);
  const more = card.querySelector("button.dbml-more");
  const rest = table.columns.length - shown.size;
  if (more) more.textContent = open ? "Show fewer columns" : `Show ${rest} more ${rest === 1 ? "column" : "columns"}`;
}

function clearMarks(article: Element) {
  for (const row of article.querySelectorAll(".dbml-origin, .dbml-same")) row.classList.remove("dbml-origin", "dbml-same");
}

// A click on a column name marks the columns with the same name, and opens
// the cards that hold them. A second click on the same column clears the marks.
function toggleMark(ctx: Ctx, row: HTMLElement) {
  const wasOrigin = row.classList.contains("dbml-origin");
  clearMarks(ctx.article);
  const table = row.closest<HTMLElement>(".dbml-card")?.dataset.table;
  if (wasOrigin || table === undefined) return;
  row.classList.add("dbml-origin");
  const same = sameColumns(ctx.schema, table, row.dataset.col ?? "");
  for (const card of ctx.article.querySelectorAll<HTMLElement>(".dbml-card")) {
    const rows = [...card.querySelectorAll<HTMLElement>("tr.dbml-col")].filter((r) => same.has(colKey(card.dataset.table ?? "", r.dataset.col ?? "")));
    for (const r of rows) r.classList.add("dbml-same");
    if (rows.length > 0) setOpen(card, true);
  }
}

function onCardClick(ctx: Ctx, card: HTMLElement, ev: Event) {
  const target = ev.target as Element;
  if (target.closest("a")) return;
  if (target.closest("button.dbml-more")) {
    setOpen(card, !card.classList.contains("dbml-open"));
    return;
  }
  const row = target.closest<HTMLElement>("tr.dbml-col");
  if (row && target.closest(".dbml-col-name")) toggleMark(ctx, row);
}

function card(ctx: Ctx, table: DbmlTable) {
  const { doc } = ctx;
  const box = el(doc, "div", "dbml-card");
  box.dataset.table = table.name;
  tableOfCard.set(box, table);
  box.append(el(doc, "h3", "", table.name));
  if (table.note) box.append(el(doc, "p", "dbml-note", table.note));
  const refs = refsOf(ctx.schema, table.name);
  const cols = el(doc, "table", "dbml-cols");
  const body = el(doc, "tbody", "");
  for (const column of table.columns) body.append(columnRow(ctx, column, refs.out));
  cols.append(body);
  box.append(cols);
  if (refs.in.length > 0) box.append(incoming(ctx, refs.in));
  if (shownColumns(table, false).length < table.columns.length) {
    box.append(button(doc, "dbml-btn dbml-more", ""));
    setOpen(box, false);
  }
  box.addEventListener("click", (ev) => onCardClick(ctx, box, ev));
  return box;
}

function setAll(ctx: Ctx, open: boolean) {
  for (const box of ctx.article.querySelectorAll<HTMLElement>(".dbml-card")) setOpen(box, open);
}

// The summary line, with the two buttons that open and close every card.
function summaryBar(ctx: Ctx, text: string) {
  const { doc } = ctx;
  const bar = el(doc, "div", "dbml-bar");
  const expand = button(doc, "dbml-btn", "Expand all");
  expand.addEventListener("click", () => setAll(ctx, true));
  const collapse = button(doc, "dbml-btn", "Collapse all");
  collapse.addEventListener("click", () => setAll(ctx, false));
  bar.append(el(doc, "div", "dbml-summary", text), expand, collapse);
  return bar;
}

/** Fill `article` with the table cards of the DBML `text`. */
export function renderDbmlGrid(doc: Document, article: Element, text: string): void {
  article.replaceChildren();
  const schema = parseDbml(text);
  schemas.set(article, schema);
  if (schema.tables.length === 0) {
    article.append(el(doc, "p", "dbml-empty", "No tables found in this file."));
    return;
  }
  const ctx: Ctx = { doc, article, schema, names: new Set(schema.tables.map((t) => t.name)) };
  const groups = groupTables(schema);
  let summary = `${plural(schema.tables.length, "table", "tables")} in ${plural(groups.length, "group", "groups")}.`;
  if (schema.skipped > 0) summary += ` ${plural(schema.skipped, "line was", "lines were")} not understood.`;
  article.append(summaryBar(ctx, summary));
  for (const group of groups) {
    const section = el(doc, "section", "dbml-group");
    section.append(el(doc, "h2", "", group.label));
    const grid = el(doc, "div", "dbml-grid");
    for (const table of group.tables) grid.append(card(ctx, table));
    section.append(grid);
    article.append(section);
  }
}

// True when the card hides a column that the term matches.
function hidesMatch(table: DbmlTable, columns: Set<string>) {
  const shown = new Set(shownColumns(table, false));
  return table.columns.some((c) => columns.has(colKey(table.name, c.name)) && !shown.has(c));
}

/**
 * Show only the cards that the Find `term` matches, and the groups that still
 * hold a card. A card that hides a matching column is opened. An empty term
 * shows every card, closed. `article` is a body that `renderDbmlGrid` filled.
 */
export function filterDbml(article: Element, term: string): void {
  const schema = schemas.get(article);
  if (!schema) return;
  const match = matchSchema(schema, term);
  for (const box of article.querySelectorAll<HTMLElement>(".dbml-card")) {
    const table = tableOfCard.get(box);
    if (!table) continue;
    const shown = match.tables.has(table.name);
    box.hidden = !shown;
    if (term === "") setOpen(box, false);
    else if (shown && hidesMatch(table, match.columns)) setOpen(box, true);
  }
  for (const group of article.querySelectorAll<HTMLElement>(".dbml-group")) {
    group.hidden = group.querySelector(".dbml-card:not([hidden])") === null;
  }
}
