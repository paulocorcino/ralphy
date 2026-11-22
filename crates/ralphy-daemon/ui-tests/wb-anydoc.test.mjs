// Unit tests for assets/ui/wb-anydoc-worker.ts and wb-anydoc.ts: the worker
// reads bytes with the vendored anydoc-wasm, and the page side keeps one
// worker with a promise per request.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { initSync } from "../assets/ui/vendor/anydoc/anydoc_wasm.js";
import { wireAnydocWorker } from "../assets/ui/wb-anydoc-worker.ts";
import { createAnydocReader } from "../assets/ui/wb-anydoc.ts";

const WASM = new URL("../assets/ui/vendor/anydoc/anydoc_wasm_bg.wasm", import.meta.url);

// A ZIP of stored entries: enough for anydoc to open an Office package.
function crc32(bytes) {
  let crc = ~0;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}

function head(sig, size) {
  const b = Buffer.alloc(size);
  b.writeUInt32LE(sig, 0);
  return b;
}

function zip(files) {
  const enc = new TextEncoder();
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, text] of Object.entries(files)) {
    const path = enc.encode(name);
    const data = enc.encode(text);
    const local = head(0x04034b50, 30);
    local.writeUInt16LE(20, 4);
    local.writeUInt32LE(crc32(data), 14);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(data.length, 22);
    local.writeUInt16LE(path.length, 26);
    const central = head(0x02014b50, 46);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt32LE(crc32(data), 16);
    central.writeUInt32LE(data.length, 20);
    central.writeUInt32LE(data.length, 24);
    central.writeUInt16LE(path.length, 28);
    central.writeUInt32LE(offset, 42);
    locals.push(local, path, data);
    centrals.push(central, path);
    offset += local.length + path.length + data.length;
  }
  const dir = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(centrals.length / 2, 8);
  end.writeUInt16LE(centrals.length / 2, 10);
  end.writeUInt32LE(dir.length, 12);
  end.writeUInt32LE(offset, 16);
  return new Uint8Array(Buffer.concat([...locals, dir, end]));
}

const NS = "http://schemas.openxmlformats.org";
const REL = `${NS}/officeDocument/2006/relationships`;

function sheetXml(rows) {
  const body = rows
    .map((cells, r) => {
      const xml = cells.map((cell, c) => {
        const ref = `${String.fromCodePoint(65 + c)}${r + 1}`;
        if (typeof cell === "number") return `<c r="${ref}"><v>${cell}</v></c>`;
        if (cell.formula) return `<c r="${ref}"><f>${cell.formula}</f><v>${cell.value}</v></c>`;
        return `<c r="${ref}" t="inlineStr"><is><t>${cell}</t></is></c>`;
      });
      return `<row r="${r + 1}">${xml.join("")}</row>`;
    })
    .join("");
  return `<worksheet xmlns="${NS}/spreadsheetml/2006/main"><sheetData>${body}</sheetData></worksheet>`;
}

// The shape of the spike's book.xlsx: three sheets, one with a formula cell.
function bookXlsx() {
  const names = ["People", "Totals", "Notes"];
  const sheets = [
    [
      ["Name", "Age"],
      ["Ana", 21],
    ],
    [
      ["Base", "Double"],
      [21, { formula: "A2*2", value: 42 }],
    ],
    [["Note"], ["first"], ["second"]],
  ];
  const ct = `${NS}/officeDocument/2006`;
  const files = {
    "[Content_Types].xml":
      `<Types xmlns="${NS}/package/2006/content-types">` +
      `<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>` +
      `<Default Extension="xml" ContentType="application/xml"/>` +
      `<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>` +
      names
        .map(
          (_, i) =>
            `<Override PartName="/xl/worksheets/sheet${i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`,
        )
        .join("") +
      `</Types>`,
    "_rels/.rels":
      `<Relationships xmlns="${NS}/package/2006/relationships">` +
      `<Relationship Id="rId1" Type="${ct}/relationships/officeDocument" Target="xl/workbook.xml"/>` +
      `</Relationships>`,
    "xl/workbook.xml":
      `<workbook xmlns="${NS}/spreadsheetml/2006/main" xmlns:r="${REL}"><sheets>` +
      names.map((n, i) => `<sheet name="${n}" sheetId="${i + 1}" r:id="rId${i + 1}"/>`).join("") +
      `</sheets></workbook>`,
    "xl/_rels/workbook.xml.rels":
      `<Relationships xmlns="${NS}/package/2006/relationships">` +
      names
        .map(
          (_, i) =>
            `<Relationship Id="rId${i + 1}" Type="${REL}/worksheet" Target="worksheets/sheet${i + 1}.xml"/>`,
        )
        .join("") +
      `</Relationships>`,
  };
  sheets.forEach((rows, i) => (files[`xl/worksheets/sheet${i + 1}.xml`] = sheetXml(rows)));
  return zip(files);
}

// A worker scope that records each reply and resolves when one arrives.
function fakeScope() {
  let listener = null;
  const replies = [];
  let wake = null;
  return {
    replies,
    addEventListener(_type, fn) {
      listener = fn;
    },
    postMessage(reply) {
      replies.push(reply);
      wake?.();
    },
    ask(request) {
      const next = new Promise((resolve) => {
        wake = resolve;
      });
      listener({ data: request });
      return next.then(() => replies.at(-1));
    },
  };
}

function readyWorker() {
  const scope = fakeScope();
  wireAnydocWorker(scope, () => Promise.resolve(initSync({ module: readFileSync(WASM) })));
  return scope;
}

function textOf(block) {
  return (block.content ?? []).map((inline) => inline.text ?? "").join("");
}

function cellText(slot) {
  return (slot.cell?.blocks ?? []).map((block) => textOf(block)).join("");
}

test("the worker turns a book of three sheets into one table per sheet", async () => {
  const reply = await readyWorker().ask({ id: 7, bytes: bookXlsx(), format: "xlsx" });
  assert.equal(reply.id, 7);
  assert.equal(reply.ok, true, `the read failed: ${reply.code}`);
  const tables = reply.document.blocks.filter((block) => block.kind === "table");
  const headings = reply.document.blocks.filter((block) => block.kind === "heading");
  assert.equal(tables.length, 3);
  assert.deepEqual(
    headings.map((block) => textOf(block)),
    ["People", "Totals", "Notes"],
  );
  const totals = tables[1].table.grid;
  assert.equal(cellText(totals[1][1]), "42", "a formula cell holds its stored value");
  assert.equal(tables[2].table.grid.length, 3);
});

test("a ZIP that is not a document comes back as a failure value", async () => {
  const bytes = zip({ "readme.txt": "not a document" });
  const reply = await readyWorker().ask({ id: 3, bytes, format: "docx" });
  assert.equal(reply.id, 3);
  assert.equal(reply.ok, false);
  assert.ok(
    ["unsupported", "malformed", "missingPart"].includes(reply.code),
    `an anydoc error code, got ${reply.code}`,
  );
});

test("a worker whose wasm does not load answers failed, and tries again next time", async () => {
  const scope = fakeScope();
  let tries = 0;
  wireAnydocWorker(scope, () => {
    tries++;
    return Promise.reject(new Error("no wasm"));
  });
  assert.deepEqual(await scope.ask({ id: 1, bytes: new Uint8Array(), format: "odt" }), {
    id: 1,
    ok: false,
    code: "failed",
  });
  await scope.ask({ id: 2, bytes: new Uint8Array(), format: "odt" });
  assert.equal(tries, 2);
});

// A worker that records its requests and answers when the test says so.
function fakeWorkers() {
  const made = [];
  const spawn = () => {
    const listeners = new Map();
    const worker = {
      requests: [],
      ended: false,
      addEventListener(type, fn) {
        listeners.set(type, fn);
      },
      fire(type, event) {
        listeners.get(type)(event);
      },
      postMessage(request) {
        worker.requests.push(request);
      },
      terminate() {
        worker.ended = true;
      },
      reply(reply) {
        worker.fire("message", { data: reply });
      },
    };
    made.push(worker);
    return worker;
  };
  return { made, spawn };
}

const tick = (ms) =>
  new Promise((resolve) => {
    setTimeout(resolve, ms);
  });

test("the reader keeps one worker and answers each request by its id", async () => {
  const { made, spawn } = fakeWorkers();
  const read = createAnydocReader(spawn, 10_000);
  const first = read(new Uint8Array([1]), "docx");
  const second = read(new Uint8Array([2]), "ods");
  assert.equal(made.length, 1);
  const [a, b] = made[0].requests;
  assert.deepEqual([a.format, b.format], ["docx", "ods"]);
  made[0].reply({ id: b.id, ok: false, code: "encrypted" });
  made[0].reply({ id: a.id, ok: true, document: { blocks: [], notes: [], assets: [] } });
  assert.deepEqual(await second, { ok: false, code: "encrypted" });
  assert.equal((await first).ok, true);
});

test("the reader ends an idle worker and starts a new one for the next request", async () => {
  const { made, spawn } = fakeWorkers();
  const read = createAnydocReader(spawn, 5);
  const done = read(new Uint8Array(), "odt");
  made[0].reply({ id: made[0].requests[0].id, ok: false, code: "malformed" });
  await done;
  await tick(20);
  assert.equal(made[0].ended, true);
  void read(new Uint8Array(), "odt");
  assert.equal(made.length, 2);
  assert.equal(made[1].ended, false);
});

test("a crashed worker answers every waiting request with a failure value", async () => {
  const { made, spawn } = fakeWorkers();
  const read = createAnydocReader(spawn, 10_000);
  const waiting = [read(new Uint8Array(), "docx"), read(new Uint8Array(), "xlsx")];
  made[0].fire("error", new Event("error"));
  assert.deepEqual(await Promise.all(waiting), [
    { ok: false, code: "failed" },
    { ok: false, code: "failed" },
  ]);
  assert.equal(made[0].ended, true);
});
