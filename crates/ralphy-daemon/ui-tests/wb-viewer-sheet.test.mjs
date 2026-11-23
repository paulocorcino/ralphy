// Unit tests for assets/ui/wb-viewer-sheet.ts, the sheet pane: the grid has no
// download, an xlsx offers each of its sheets, a csv stays in the editor and
// switches to a grid and back, and a file over the cap starts no reader.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SHEET_TOO_LARGE_TEXT,
  buildSheet,
  drawGrid,
  mountSheetSwitch,
} from "../assets/ui/wb-viewer-sheet.ts";
import { viewerKindOf } from "../assets/ui/wb-viewers.ts";
import { SHEETJS_PATH } from "../assets/ui/wb-sheet-read.ts";

function element(tag, made) {
  const el = {
    tag,
    children: [],
    parent: null,
    attrs: {},
    dataset: {},
    style: {},
    listeners: {},
    textContent: "",
    value: "",
    innerHTML: "",
    stubs: {},
    append(...kids) {
      for (const kid of kids) kid.parent = el;
      el.children.push(...kids);
    },
    insertBefore(kid) {
      el.append(kid);
    },
    prepend(...kids) {
      for (const kid of kids) kid.parent = el;
      el.children.unshift(...kids);
    },
    replaceChildren(...kids) {
      el.children = [];
      el.append(...kids);
    },
    after(kid) {
      el.parent?.append(kid);
    },
    remove() {
      if (el.parent) el.parent.children = el.parent.children.filter((one) => one !== el);
      el.parent = null;
    },
    setAttribute(name, value) {
      el.attrs[name] = String(value);
    },
    addEventListener(type, fn) {
      (el.listeners[type] ??= []).push(fn);
    },
    fire(type) {
      for (const fn of el.listeners[type] ?? []) fn();
    },
    // The pane's own markup: one stub element per selector.
    querySelector(sel) {
      if (sel.includes("detach")) return null;
      return (el.stubs[sel] ??= element(sel, made));
    },
  };
  made?.push(el);
  return el;
}

// A head whose stylesheets load as soon as they are added, unless held.
function loadingHead(made) {
  const head = element("head", made);
  const append = head.append;
  head.held = false;
  head.append = (...kids) => {
    append(...kids);
    if (!head.held) queueMicrotask(() => kids.forEach((kid) => kid.fire("load")));
  };
  return head;
}

function fakeDocument() {
  const made = [];
  return {
    made,
    head: loadingHead(made),
    createElement: (tag) => element(tag, made),
  };
}

// A Tabulator stand-in that records each grid it draws.
function fakeKit() {
  const grids = [];
  const registered = [];
  const FormatModule = { name: "format" };
  const FrozenColumnsModule = { name: "frozen columns" };
  class Tabulator {
    constructor(holder, options) {
      this.options = options;
      this.destroyed = false;
      this.redraws = [];
      this.handlers = {};
      grids.push(this);
    }
    static registerModule(modules) {
      registered.push(...modules);
    }
    setFilter() {}
    redraw(force) {
      this.redraws.push(force);
    }
    on(event, callback) {
      this.handlers[event] = callback;
    }
    clearFilter() {}
    destroy() {
      this.destroyed = true;
    }
  }
  return {
    grids,
    registered,
    FormatModule,
    FrozenColumnsModule,
    module: {
      Tabulator,
      FilterModule: {},
      ResizeColumnsModule: {},
      FormatModule,
      FrozenColumnsModule,
    },
  };
}

function settle() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

// A SheetJS that reads the given sheets, whatever the bytes are.
function fakeSheetJs(sheets) {
  return {
    read: () => ({
      SheetNames: sheets.map((sheet) => sheet.name),
      Sheets: Object.fromEntries(sheets.map((sheet) => [sheet.name, sheet])),
    }),
    utils: { sheet_to_json: (sheet) => sheet.rows },
    CFB: {},
  };
}

// The steps of a sheet pane over the fake grid and the fake reader.
function sheetDeps(kit, sheets) {
  const xlsx = fakeSheetJs(sheets);
  return {
    fetchBytes: () => Promise.resolve(new Uint8Array([1])),
    load: (path) => Promise.resolve({ ok: true, module: path === SHEETJS_PATH ? xlsx : kit }),
  };
}

function sheetPane(path, deps, document = fakeDocument()) {
  const host = {
    document,
    viewers: element("viewers"),
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const rec = { id: "t", project: "me/proj", path, checkout: null, content: "", refused: null };
  buildSheet(host, rec, deps);
  return { document, pane: host.viewers.children[0] };
}

// An observer that never reports, for the real Tabulator build.
function QuietObserver() {
  return { observe() {}, unobserve() {}, disconnect() {} };
}

test("the table built by the viewer has no download function", async () => {
  const globals = ["document", "window", "getComputedStyle", "HTMLElement", "ResizeObserver"];
  const saved = [...globals, "addEventListener", "removeEventListener"].map((name) => [
    name,
    Object.getOwnPropertyDescriptor(globalThis, name),
  ]);
  // Tabulator touches the DOM as it loads and builds; a node that answers
  // every read lets the real build run in Node.
  const empty = new Set(["parentNode", "parentElement", "nextSibling", "previousSibling"]);
  const none = new Set(["then", Symbol.iterator, "firstChild", "lastChild", "offsetParent"]);
  const any = new Proxy(() => {}, {
    get: (_, key) => {
      if (key === Symbol.toPrimitive) return () => "";
      if (key === "length") return 0;
      if (empty.has(key)) return null;
      return none.has(key) ? undefined : any;
    },
    apply: () => any,
    set: () => true,
  });
  globalThis.document = any;
  globalThis.window = globalThis;
  globalThis.getComputedStyle = () => any;
  globalThis.HTMLElement = { [Symbol.hasInstance]: () => true };
  // The resize module watches the holder, or the window when there is no observer.
  globalThis.ResizeObserver = QuietObserver;
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  try {
    const kit = await import("../assets/ui/vendor/tabulator/tabulator_esm.min.mjs");
    const grid = drawGrid(kit, any, { name: "s", rows: [["a", "b"]] });
    await new Promise((resolve) => {
      grid.on("tableBuilt", resolve);
    });
    assert.equal(typeof grid.setFilter, "function", "the filter module is registered");
    assert.equal(grid.download, undefined);
    assert.equal(grid.downloadToTab, undefined);
  } finally {
    for (const [name, desc] of saved) {
      if (desc) Object.defineProperty(globalThis, name, desc);
      else delete globalThis[name];
    }
  }
});

test("the grid registers the format module its plaintext cells need", async () => {
  const kit = fakeKit();
  sheetPane("book.xlsx", sheetDeps(kit.module, [{ name: "Data", rows: [["<b>x</b>"]] }]));
  await settle();
  assert.equal(kit.grids[0].options.columns[0].formatter, "plaintext");
  assert.ok(kit.registered.includes(kit.FormatModule), "plaintext is drawn by the format module");
});

function oneSheet(kit) {
  return sheetPane("book.xlsx", sheetDeps(kit.module, [{ name: "Data", rows: [["a", "b"]] }]));
}

test("the grid measures its columns again when its hidden pane is shown", async () => {
  const kit = fakeKit();
  let observer = null;
  // Only for this test: other files pin that no observer exists.
  globalThis.ResizeObserver = function ResizeObserver(callback) {
    observer = { fire: (width) => callback([{ contentRect: { width } }]), observe() {} };
    return observer;
  };
  try {
    oneSheet(kit);
    await settle();
  } finally {
    delete globalThis.ResizeObserver;
  }
  observer.fire(800);
  assert.deepEqual(kit.grids[0].redraws, [], "no redraw before the table is built");
  observer.fire(0);
  kit.grids[0].handlers.tableBuilt();
  assert.deepEqual(kit.grids[0].redraws, [], "a hidden pane draws nothing");
  observer.fire(800);
  observer.fire(900);
  assert.deepEqual(kit.grids[0].redraws, [true], "shown once: one full redraw");
});

test("the grid waits for its stylesheet before it draws", async () => {
  const kit = fakeKit();
  const document = fakeDocument();
  document.head.held = true;
  sheetPane("book.xlsx", sheetDeps(kit.module, [{ name: "Data", rows: [["a"]] }]), document);
  await settle();
  assert.equal(kit.grids.length, 0, "no grid is measured without its styles");
  document.made.find((el) => el.tag === "link").fire("load");
  await settle();
  assert.equal(kit.grids.length, 1);
});

test("the grid numbers its rows in a frozen column", async () => {
  const kit = fakeKit();
  oneSheet(kit);
  await settle();
  const { rowHeader } = kit.grids[0].options;
  assert.equal(rowHeader.formatter, "rownum");
  assert.equal(rowHeader.frozen, true);
  assert.ok(kit.registered.includes(kit.FrozenColumnsModule), "a frozen column needs its module");
});

test("a file of one sheet hides the sheet selector", async () => {
  const kit = fakeKit();
  const { document } = oneSheet(kit);
  await settle();
  assert.equal(document.made.find((el) => el.tag === "select").hidden, true);
});

test("an xlsx with three sheets offers three sheets in the selector", async () => {
  const { module, grids } = fakeKit();
  const sheets = [
    { name: "North", rows: [["1"]] },
    { name: "South", rows: [["2"]] },
    { name: "East", rows: [["3", "4"]] },
  ];
  const { document } = sheetPane("book.xlsx", sheetDeps(module, sheets));
  await settle();
  const select = document.made.find((el) => el.tag === "select");
  assert.deepEqual(
    select.children.map((option) => option.textContent),
    ["North", "South", "East"],
  );
  assert.equal(grids.length, 1);
  select.value = "2";
  assert.notEqual(select.hidden, true, "three sheets can be chosen");
  select.fire("change");
  assert.deepEqual(grids[1].options.data, [{ c0: "3", c1: "4" }]);
  assert.equal(grids[0].destroyed, true);
});

test("a csv opens in the editor and the switch shows a grid and back, text unchanged", async () => {
  assert.equal(viewerKindOf("csv"), null, "a csv opens in the editor");
  assert.equal(viewerKindOf("tsv"), null, "a tsv opens in the editor");
  const { module, grids } = fakeKit();
  const document = fakeDocument();
  const holder = element("viewer");
  const body = element("editor");
  holder.append(body);
  const editor = { text: "name,count\nfig,3\n" };
  const before = editor.text;
  const button = mountSheetSwitch(
    { document, toolbar: element("toolbar"), body, text: () => editor.text, path: "a.csv" },
    () => Promise.resolve({ ok: true, module }),
  );
  button.fire("click");
  await settle();
  assert.equal(body.style.display, "none");
  assert.equal(holder.children.length, 2, "the grid pane sits beside the editor");
  assert.deepEqual(grids[0].options.data, [
    { c0: "name", c1: "count" },
    { c0: "fig", c1: "3" },
  ]);
  button.fire("click");
  assert.equal(body.style.display, "");
  assert.deepEqual(holder.children, [body]);
  assert.equal(grids[0].destroyed, true);
  assert.equal(editor.text, before);
});

test("a semicolon csv is read with its guessed separator, and the selector changes it", async () => {
  const { module, grids } = fakeKit();
  const document = fakeDocument();
  const body = element("editor");
  element("viewer").append(body);
  mountSheetSwitch(
    { document, toolbar: element("toolbar"), body, text: () => "a;b\n1,5;2\n", path: "a.csv" },
    () => Promise.resolve({ ok: true, module }),
  ).fire("click");
  await settle();
  const select = document.made.find((el) => el.attrs["aria-label"] === "Separator");
  assert.equal(select.value, ";", "the guess is the semicolon");
  assert.deepEqual(grids.at(-1).options.data, [
    { c0: "a", c1: "b" },
    { c0: "1,5", c1: "2" },
  ]);
  select.value = ",";
  select.fire("change");
  await settle();
  assert.deepEqual(grids.at(-1).options.data, [
    { c0: "a;b", c1: "" },
    { c0: "1", c1: "5;2" },
  ]);
});

test("a tsv grid has no separator selector", async () => {
  const { module, grids } = fakeKit();
  const document = fakeDocument();
  const body = element("editor");
  element("viewer").append(body);
  mountSheetSwitch(
    { document, toolbar: element("toolbar"), body, text: () => "a\tb\n", path: "a.tsv" },
    () => Promise.resolve({ ok: true, module }),
  ).fire("click");
  await settle();
  assert.equal(
    document.made.some((el) => el.attrs["aria-label"] === "Separator"),
    false,
  );
  assert.deepEqual(grids[0].options.data, [{ c0: "a", c1: "b" }]);
});

test("two clicks while the grid library loads build one grid", async () => {
  const { module, grids } = fakeKit();
  const holder = element("viewer");
  const body = element("editor");
  holder.append(body);
  const button = mountSheetSwitch(
    {
      document: fakeDocument(),
      toolbar: element("toolbar"),
      body,
      text: () => "a,b",
      path: "a.csv",
    },
    () => Promise.resolve({ ok: true, module }),
  );
  button.fire("click");
  button.fire("click");
  await settle();
  assert.equal(grids.length, 1);
  assert.equal(holder.children.length, 2);
});

test("a too large answer shows the message and starts no reader", async () => {
  const started = [];
  const { pane } = sheetPane("big.ods", {
    fetchBytes: () => Promise.resolve("too large"),
    load: () => {
      started.push("load");
      return Promise.resolve({ ok: false, text: "" });
    },
  });
  await settle();
  assert.equal(pane.stubs[".sheet-meta"].textContent, SHEET_TOO_LARGE_TEXT);
  assert.deepEqual(started, []);
});
