// Unit tests for assets/ui/wb-viewer-archive.ts: the archive pane draws the
// `file.entries` reply as a table: a header, then one row per entry.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { buildArchive, entryName } from "../assets/ui/wb-viewer-archive.ts";
import { classify } from "../assets/ui/wb-file-paths.ts";

function fixture(name) {
  return JSON.parse(readFileSync(join(import.meta.dirname, "fixtures", name + ".json"), "utf8"));
}

function fakeEl(tag) {
  const found = new Map();
  const el = {
    tag,
    style: {},
    dataset: {},
    className: "",
    attrs: {},
    innerHTML: "",
    textContent: "",
    children: [],
    append(...kids) {
      el.children.push(...kids);
    },
    replaceChildren() {
      el.children = [];
    },
    querySelector(sel) {
      if (!found.has(sel)) found.set(sel, fakeEl(sel));
      return found.get(sel);
    },
    setAttribute(name, value) {
      el.attrs[name] = value;
    },
    addEventListener() {},
  };
  return el;
}

// Builds the pane against a daemon that answers `reply`; returns what it drew.
async function draw(reply) {
  const mount = fakeEl("viewers");
  const document = { createElement: (tag) => fakeEl(tag) };
  globalThis.WBDaemon = {
    withCheckout: (p) => p,
    observe: () => Promise.resolve(reply),
  };
  try {
    const host = { document, viewers: mount, setPathLabel() {}, detachBtnHtml: () => "" };
    buildArchive(host, { id: "t", project: "p", path: "a.zip", checkout: null });
    await new Promise((resolve) => {
      setImmediate(resolve);
    });
    return mount.children[0].querySelector(".viewer-body").children;
  } finally {
    delete globalThis.WBDaemon;
  }
}

// The name a row shows (the text after its icon) and its size.
const nameOf = (row) => row.children[0].children[1].textContent;
const sizeOf = (row) => row.children[1].textContent;

test("a listing draws a header, then one row per entry with its name and size", async () => {
  const reply = fixture("file.entries");
  const [head, ...rows] = await draw(reply);
  assert.deepEqual(
    head.children.map((c) => c.textContent),
    ["Name", "Size"],
  );
  assert.equal(rows.length, reply.entries.length);
  assert.equal(nameOf(rows[0]), "readme.txt", "the name, not the whole path");
  assert.equal(sizeOf(rows[0]), "940 bytes");
  assert.equal(rows[0].attrs.title, "docs/readme.txt\n940 bytes", "the title holds the path");
});

test("a row is indented by the depth of its folder", () => {
  assert.deepEqual(entryName("a.txt"), { depth: 0, name: "a.txt" });
  assert.deepEqual(entryName("Cactus/royalties/Classes/"), { depth: 2, name: "Classes" });
  assert.deepEqual(entryName("Cactus/royalties/x.php"), { depth: 2, name: "x.php" });
});

test("a name that looks like markup shows as text and adds no element", async () => {
  const name = "<img src=x onerror=alert(1)>";
  const rows = await draw({
    status: "ok",
    truncated: false,
    entries: [{ path: name, size: 1, folder: false, modified: null }],
  });
  assert.equal(rows.length, 2, "the header and one row");
  assert.equal(nameOf(rows[1]), name);
  assert.deepEqual(
    rows[1].children[0].children.map((c) => c.tag),
    ["i", "span"],
  );
});

test("a partial listing shows the partial note", async () => {
  const reply = { ...fixture("file.entries"), truncated: true };
  const rows = await draw(reply);
  assert.equal(rows.length, reply.entries.length + 2);
  assert.match(rows.at(-1).textContent, /partial/u);
  const whole = await draw(fixture("file.entries"));
  assert.equal(whole.length, reply.entries.length + 1);
});

test("a zip, a tar and a tgz open in the archive viewer", () => {
  for (const name of ["a.zip", "b.TAR", "c.tgz"]) {
    assert.equal(classify(name), "archive", name);
  }
});
