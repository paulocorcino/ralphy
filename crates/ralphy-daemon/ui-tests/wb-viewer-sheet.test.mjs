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

function fakeDocument() {
  const made = [];
  return {
    made,
    head: element("head"),
    createElement: (tag) => element(tag, made),
  };
}

const csv = (text) =>
  text
    .split("\n")
    .filter((line) => line !== "")
    .map((line) => line.split(","));

// A Tabulator stand-in that records each grid it draws.
function fakeKit() {
  const grids = [];
  class Tabulator {
    constructor(holder, options) {
      this.options = options;
      this.destroyed = false;
      grids.push(this);
    }
    static registerModule() {}
    setFilter() {}
    clearFilter() {}
    destroy() {
      this.destroyed = true;
    }
  }
  return {
    grids,
    module: {
      Tabulator,
      FilterModule: {},
      ResizeColumnsModule: {},
      ImportModule: { importers: { csv } },
    },
  };
}

function settle() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

const words = (text) => ({ kind: "text", text, style: {} });
const cell = (text) => ({
  kind: "origin",
  cell: { blocks: [{ kind: "paragraph", content: [words(text)] }], colSpan: 1, rowSpan: 1 },
});
const heading = (text) => ({ kind: "heading", level: 1, content: [words(text)] });
const table = (rows) => ({
  kind: "table",
  table: { grid: rows.map((row) => row.map((text) => cell(text))), headerRows: 0, kind: "data" },
});

function sheetPane(path, deps) {
  const document = fakeDocument();
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

test("the table built by the viewer has no download function", async () => {
  const saved = ["document", "window", "getComputedStyle", "HTMLElement"].map((name) => [
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

test("an xlsx with three sheets offers three sheets in the selector", async () => {
  const { module, grids } = fakeKit();
  const model = {
    blocks: [
      heading("North"),
      table([["1"]]),
      heading("South"),
      table([["2"]]),
      heading("East"),
      table([["3", "4"]]),
    ],
    notes: [],
    assets: [],
  };
  const { document } = sheetPane("book.xlsx", {
    fetchBytes: () => Promise.resolve(new Uint8Array([1])),
    read: () => Promise.resolve({ ok: true, document: model }),
    load: () => Promise.resolve({ ok: true, module }),
  });
  await settle();
  const select = document.made.find((el) => el.tag === "select");
  assert.deepEqual(
    select.children.map((option) => option.textContent),
    ["North", "South", "East"],
  );
  assert.equal(grids.length, 1);
  select.value = "2";
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

test("two clicks while the grid library loads build one grid", async () => {
  const { module, grids } = fakeKit();
  const holder = element("viewer");
  const body = element("editor");
  holder.append(body);
  const button = mountSheetSwitch(
    { document: fakeDocument(), toolbar: element("toolbar"), body, text: () => "a,b", path: "a.csv" },
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
    read: () => {
      started.push("read");
      return Promise.resolve({ ok: false, code: "failed" });
    },
    load: () => {
      started.push("load");
      return Promise.resolve({ ok: false, text: "" });
    },
  });
  await settle();
  assert.equal(pane.stubs[".sheet-meta"].textContent, SHEET_TOO_LARGE_TEXT);
  assert.deepEqual(started, []);
});
