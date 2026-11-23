// The sheet pane (ADR-0091 D11, amended 2026-11-20): an xlsx or ods file is
// read by the document worker, a csv or tsv text by Tabulator's own importer,
// and both draw in a virtual Tabulator grid. Only the filter, column and
// import modules are registered, so the grid has no download (D12). A cell
// draws with the `plaintext` formatter: text from the file never becomes markup.
import { setMarkup } from "./wb-html.ts";
import { need, required } from "./wb-dom-need.ts";
import { fileBytesUrl } from "./wb-file-bytes.ts";
import { devError } from "./wb-log.ts";
import { showLoading } from "./wb-viewer-loading.ts";
import { guessSeparator, parseDelimited, SEPARATORS } from "./wb-sheet-csv.ts";
import type { Separator } from "./wb-sheet-csv.ts";
import { readAnydoc } from "./wb-anydoc.ts";
import { loadVendor } from "./wb-vendor-load.ts";
import type { AnydocFormat, AnydocResult } from "./wb-anydoc.ts";
import type { VendorLoad } from "./wb-vendor-load.ts";
import type { Block, Document as DocModel, Inline, Table } from "./vendor/anydoc/anydoc_wasm.js";
import type { ViewerReadOnlyDeps } from "./wb-file-viewer-read-only.ts";
import type { BytesRecord } from "./wb-file-viewer-records.ts";

export const TABULATOR_PATH = "./vendor/tabulator/tabulator_esm.min.mjs";
const TABULATOR_CSS_PATH = "vendor/tabulator/tabulator.min.css";
/** The most rows a sheet draws (ADR-0091 D11, D14). */
export const MAX_SHEET_ROWS = 500_000;
export const SHEET_TOO_LARGE_TEXT = "This sheet is too large to show: the limit is 2 megabytes.";
export const SHEET_ERROR_TEXT = "This file could not be read as a sheet.";

/** One sheet: its name and its rows of cell text. */
export type Sheet = { name: string; rows: string[][] };

type GridRow = Record<string, string>;
type GridColumn = { title: string; field: string; formatter: "plaintext" };
type RowHeader = { formatter: "rownum"; frozen: true; hozAlign: "right"; cssClass: string };
type GridOptions = {
  data: GridRow[];
  columns: GridColumn[];
  rowHeader: RowHeader;
  height: string;
  layout: "fitData";
};

// The row numbers of a sheet, as a spreadsheet shows them, frozen at the left.
const ROW_HEADER: RowHeader = {
  formatter: "rownum",
  frozen: true,
  hozAlign: "right",
  cssClass: "sheet-rownum",
};

/** The part of a Tabulator table the pane uses. */
export interface Grid {
  setFilter(filter: (row: GridRow) => boolean): void;
  clearFilter(): void;
  redraw(force: boolean): void;
  on(event: "tableBuilt", callback: () => void): void;
  destroy(): void;
}

type GridClass = {
  new (holder: HTMLElement, options: GridOptions): Grid;
  registerModule(modules: unknown[]): void;
};

/** The part of the Tabulator module the pane uses. */
export type TabulatorKit = {
  Tabulator: GridClass;
  FilterModule: unknown;
  ResizeColumnsModule: unknown;
  FormatModule: unknown;
  FrozenColumnsModule: unknown;
};

function isKit(module: unknown): module is TabulatorKit {
  if (typeof module !== "object" || module === null) return false;
  const parts = ["Tabulator", "FilterModule", "FormatModule", "FrozenColumnsModule"];
  return parts.every((part) => part in module);
}

const registered = new WeakSet<object>();

// The modules of D11, once per loaded module. The format module draws the
// `plaintext` cells and the row numbers. Never the download module.
function gridClass(kit: TabulatorKit): GridClass {
  if (!registered.has(kit)) {
    kit.Tabulator.registerModule([
      kit.FilterModule,
      kit.ResizeColumnsModule,
      kit.FormatModule,
      kit.FrozenColumnsModule,
    ]);
    registered.add(kit);
  }
  return kit.Tabulator;
}

/** The sheet name of a column: A to Z, then AA, AB and on. */
export function columnName(index: number): string {
  let n = index + 1;
  let name = "";
  while (n > 0) {
    const rest = (n - 1) % 26;
    name = String.fromCodePoint(65 + rest) + name;
    n = Math.floor((n - 1) / 26);
  }
  return name;
}

/** A grid of the sheet's first `MAX_SHEET_ROWS` rows, drawn into `holder`. */
export function drawGrid(kit: TabulatorKit, holder: HTMLElement, sheet: Sheet): Grid {
  const rows = sheet.rows.slice(0, MAX_SHEET_ROWS);
  const width = rows.reduce((max, row) => Math.max(max, row.length), 0);
  const fields = Array.from({ length: width }, (_, at) => `c${at}`);
  const columns = fields.map((field, at): GridColumn => ({
    title: columnName(at),
    field,
    formatter: "plaintext",
  }));
  const data = rows.map((row) => Object.fromEntries(fields.map((f, at) => [f, row[at] ?? ""])));
  const Grid = gridClass(kit);
  const options: GridOptions = {
    data,
    columns,
    rowHeader: ROW_HEADER,
    height: "100%",
    layout: "fitData",
  };
  return new Grid(holder, options);
}

/** Keep the rows with a cell that holds `term`, any case; an empty term keeps all. */
export function applyFilter(grid: Grid, term: string) {
  const needle = term.trim().toLowerCase();
  if (needle === "") {
    grid.clearFilter();
    return;
  }
  grid.setFilter((row) => Object.values(row).some((cell) => cell.toLowerCase().includes(needle)));
}

function inlineText(items: Inline[] | undefined): string {
  return (items ?? []).map((one) => one.text ?? inlineText(one.content)).join("");
}

function blockText(items: Block[] | undefined): string {
  return (items ?? [])
    .map((block) => block.text ?? (inlineText(block.content) || blockText(block.blocks)))
    .join("\n");
}

function tableRows(table: Table | undefined): string[][] {
  return (table?.grid ?? []).map((row) => row.map((slot) => blockText(slot.cell?.blocks)));
}

/** The sheets of a read xlsx or ods: a heading names the table after it. */
export function sheetsOf(model: DocModel): Sheet[] {
  const sheets: Sheet[] = [];
  let name: string | null = null;
  for (const block of model.blocks) {
    if (block.kind === "heading") name = inlineText(block.content);
    if (block.kind !== "table") continue;
    sheets.push({ name: name ?? `Sheet ${sheets.length + 1}`, rows: tableRows(block.table) });
    name = null;
  }
  return sheets;
}

// The separator selector of a csv grid: its first value is the guess.
function separatorSelect(document: Document, text: string, change: (s: Separator) => void) {
  const select = document.createElement("select");
  select.className = "text-field sheet-separator";
  select.setAttribute("aria-label", "Separator");
  select.setAttribute("title", "Separator");
  select.append(
    ...SEPARATORS.map(({ value, label }) => {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      return option;
    }),
  );
  select.value = guessSeparator(text);
  select.addEventListener("change", () => {
    const picked = SEPARATORS.find(({ value }) => value === select.value);
    if (picked) change(picked.value);
  });
  return select;
}

const styled = new WeakMap<Document, Promise<void>>();

// Tabulator measures its columns and rows when it draws: drawn before its
// stylesheet applies, every column is as wide as the pane and every row
// is cut. The grid waits for the stylesheet, or for its failure.
function gridStyle(document: Document): Promise<void> {
  const known = styled.get(document);
  if (known) return known;
  const link = document.createElement("link");
  const ready = new Promise<void>((resolve) => {
    link.addEventListener("load", () => {
      resolve();
    });
    link.addEventListener("error", () => {
      resolve();
    });
  });
  link.setAttribute("rel", "stylesheet");
  link.setAttribute("href", TABULATOR_CSS_PATH);
  document.head.append(link);
  styled.set(document, ready);
  return ready;
}

/** The controls and grid of a pane that shows sheets. */
type SheetView = { bar: HTMLElement; holder: HTMLElement; show: (sheets: Sheet[]) => void };

function rowsNote(sheet: Sheet) {
  const count = sheet.rows.length;
  if (count <= MAX_SHEET_ROWS) return "";
  return (
    `Showing the first ${MAX_SHEET_ROWS.toLocaleString("en")} of ` +
    `${count.toLocaleString("en")} rows.`
  );
}

// A grid built in a hidden pane measures its columns as 0 wide; it measures
// them again when the pane is shown. Tabulator's resize module redraws the
// rows only, so it does not do this.
function redrawWhenShown(holder: HTMLElement, grid: () => Grid | null) {
  if (typeof ResizeObserver === "undefined") return;
  let shown = false;
  new ResizeObserver(([entry]) => {
    const visible = (entry?.contentRect.width ?? 0) > 0;
    if (visible && !shown) grid()?.redraw(true);
    shown = visible;
  }).observe(holder);
}

function sheetView(document: Document, kit: TabulatorKit): SheetView & { close: () => void } {
  const bar = document.createElement("span");
  bar.className = "sheet-controls";
  const select = document.createElement("select");
  select.className = "text-field sheet-select";
  select.setAttribute("aria-label", "Sheet");
  const filter = document.createElement("input");
  filter.className = "text-field sheet-filter";
  filter.setAttribute("type", "search");
  filter.setAttribute("placeholder", "Filter rows");
  filter.setAttribute("aria-label", "Filter rows");
  const note = document.createElement("span");
  note.className = "sheet-note";
  bar.append(select, filter, note);
  const holder = document.createElement("div");
  holder.className = "sheet-grid";
  holder.style.height = "100%";
  let sheets: Sheet[] = [];
  let grid: Grid | null = null;
  // Tabulator throws on a redraw before it has built the table.
  let built: Grid | null = null;
  const draw = (at: number) => {
    const sheet = sheets[at];
    if (!sheet) return;
    grid?.destroy();
    const drawn = drawGrid(kit, holder, sheet);
    grid = drawn;
    built = null;
    drawn.on("tableBuilt", () => {
      if (grid === drawn) built = drawn;
    });
    note.textContent = rowsNote(sheet);
    filter.value = "";
  };
  select.addEventListener("change", () => {
    draw(Number(select.value));
  });
  filter.addEventListener("input", () => {
    if (grid) applyFilter(grid, filter.value);
  });
  let closed = false;
  const show = (all: Sheet[]) => {
    sheets = all;
    // A file of one sheet has no name for it, so there is nothing to choose.
    select.hidden = all.length < 2;
    select.replaceChildren(
      ...all.map((sheet, at) => {
        const option = document.createElement("option");
        option.value = String(at);
        option.textContent = sheet.name;
        return option;
      }),
    );
    void gridStyle(document).then(() => {
      if (!closed) draw(0);
      return null;
    });
  };
  const close = () => {
    closed = true;
    grid?.destroy();
    grid = null;
  };
  redrawWhenShown(holder, () => built);
  return { bar, holder, show, close };
}

/** The steps of a sheet pane: the file's bytes, their model, and the grid library. */
export type SheetDeps = {
  fetchBytes: (url: string) => Promise<Uint8Array | "too large">;
  read: (bytes: Uint8Array, format: AnydocFormat) => Promise<AnydocResult>;
  load: (path: string) => Promise<VendorLoad>;
};

// An xlsx or ods file is at most 2 MiB (ADR-0091 D3), under one answer of the
// route, so one request reads it whole. A larger file is refused with 413.
async function fetchSheetBytes(url: string): Promise<Uint8Array | "too large"> {
  const reply = await fetch(url);
  if (reply.status === 413) return "too large";
  if (!reply.ok) throw new Error(`the file bytes route answered ${reply.status}`);
  return new Uint8Array(await reply.arrayBuffer());
}

export const liveSheetDeps: SheetDeps = {
  fetchBytes: fetchSheetBytes,
  read: readAnydoc,
  load: loadVendor,
};

function formatOf(path: string): AnydocFormat {
  return path.toLowerCase().endsWith(".ods") ? "ods" : "xlsx";
}

async function loadKit(load: SheetDeps["load"]): Promise<TabulatorKit | string> {
  const outcome = await load(TABULATOR_PATH);
  if (!outcome.ok) return outcome.text;
  return isKit(outcome.module) ? outcome.module : SHEET_ERROR_TEXT;
}

type Shown = { meta: HTMLElement; toolbar: HTMLElement; body: HTMLElement; path: string };

async function showFile(document: Document, deps: SheetDeps, url: string, at: Shown) {
  const bytes = await deps.fetchBytes(url);
  if (bytes === "too large") {
    at.meta.textContent = SHEET_TOO_LARGE_TEXT;
    return;
  }
  const [result, kit] = await Promise.all([
    deps.read(bytes, formatOf(at.path)),
    loadKit(deps.load),
  ]);
  if (typeof kit === "string") {
    at.meta.textContent = kit;
    return;
  }
  if (!result.ok) throw new Error(`the sheet reader failed: ${result.code}`);
  const view = sheetView(document, kit);
  at.toolbar.insertBefore(view.bar, at.meta);
  at.body.append(view.holder);
  view.show(sheetsOf(result.document));
}

export function buildSheet(
  host: ViewerReadOnlyDeps,
  rec: BytesRecord,
  deps: SheetDeps = liveSheetDeps,
) {
  const { document, setPathLabel, detachBtnHtml } = host;
  const el = document.createElement("div");
  el.className = "viewer sheet-viewer";
  el.dataset["tabId"] = rec.id;
  el.style.display = "none";
  setMarkup(
    el,
    `
    <div class="viewer-toolbar">
      <span class="viewer-path"></span>
      <span class="sheet-meta"></span>
      <span class="spacer"></span>
      ${detachBtnHtml(rec)}
    </div>
    <div class="viewer-body sheet-body"></div>`,
  );
  setPathLabel(el, rec);
  required(host.viewers, "the element that holds the panes").append(el);
  el.querySelector('[data-act="detach"]')?.addEventListener("click", () => {
    host.detachClick(rec);
  });
  const meta = need(el, ".sheet-meta");
  const at = { meta, toolbar: need(el, ".viewer-toolbar"), body: need(el, ".sheet-body") };
  const url = fileBytesUrl(rec.project, rec.path, rec.checkout);
  const loaded = showLoading(document, at.body);
  void showFile(document, deps, url, { ...at, path: rec.path }).then(loaded, (err: unknown) => {
    loaded();
    devError("[workbench] sheet failed to load", err);
    meta.textContent = SHEET_ERROR_TEXT;
  });
  rec.el = el;
}

/** What the text and grid switch of a csv or tsv editor pane works on. */
export type SheetSwitchHost = {
  document: Document;
  toolbar: HTMLElement;
  /** The element that holds the editor; hidden while the grid shows. */
  body: HTMLElement;
  /** The editor's current text; the switch only reads it. */
  text: () => string;
  path: string;
};

function setCaption(button: HTMLElement, grid: boolean) {
  button.textContent = grid ? "Text" : "Grid";
  button.setAttribute("title", grid ? "Show the text" : "Show as a grid");
}

/**
 * A button that shows the editor's csv or tsv text as a grid and back. The
 * editor and its text stay as they are; the grid is drawn again each time.
 */
export function mountSheetSwitch(host: SheetSwitchHost, load = loadVendor): HTMLElement {
  const { document, body } = host;
  const button = document.createElement("button");
  button.className = "vbtn sheet-switch";
  setCaption(button, false);
  host.toolbar.append(button);
  let open: { pane: HTMLElement; close: () => void } | null = null;
  const toText = (shown: { pane: HTMLElement; close: () => void }) => {
    shown.close();
    shown.pane.remove();
    body.style.display = "";
    open = null;
    setCaption(button, false);
  };
  const toGrid = async () => {
    const kit = await loadKit(load);
    if (typeof kit === "string") {
      devError("[workbench] the grid library did not load", kit);
      return;
    }
    const view = sheetView(document, kit);
    const pane = document.createElement("div");
    pane.className = "viewer-body sheet-body";
    pane.append(view.bar, view.holder);
    body.after(pane);
    body.style.display = "none";
    open = { pane, close: view.close };
    setCaption(button, true);
    const text = host.text();
    const showWith = (separator: Separator) => {
      view.show([{ name: host.path, rows: parseDelimited(text, separator) }]);
    };
    if (host.path.toLowerCase().endsWith(".tsv")) {
      showWith("\t");
      return;
    }
    view.bar.prepend(separatorSelect(document, text, showWith));
    showWith(guessSeparator(text));
  };
  // A second click while the library loads would build a second grid.
  let loading = false;
  button.addEventListener("click", () => {
    if (open) {
      toText(open);
      return;
    }
    if (loading) return;
    loading = true;
    toGrid()
      .catch((err: unknown) => {
        devError("[workbench] the grid of the text failed", err);
      })
      .finally(() => {
        loading = false;
      });
  });
  return button;
}
