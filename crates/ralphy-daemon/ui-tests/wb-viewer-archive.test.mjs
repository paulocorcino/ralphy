// Unit tests for assets/ui/wb-viewer-archive.ts: the archive pane draws the
// `file.entries` reply as text rows.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { buildArchive } from "../assets/ui/wb-viewer-archive.ts";
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

const textOf = (row) => row.children.map((c) => c.textContent);

test("a listing draws rows, one per entry, with path and size", async () => {
  const reply = fixture("file.entries");
  const rows = await draw(reply);
  assert.equal(rows.length, reply.entries.length);
  assert.equal(textOf(rows[0])[0], "docs/readme.txt");
  assert.match(textOf(rows[0])[1], /940 bytes/u);
});

test("a name that looks like markup shows as text and adds no element", async () => {
  const name = "<img src=x onerror=alert(1)>";
  const rows = await draw({
    status: "ok",
    truncated: false,
    entries: [{ path: name, size: 1, folder: false, modified: null }],
  });
  assert.equal(rows.length, 1);
  assert.equal(textOf(rows[0])[0], name);
  assert.deepEqual(
    rows[0].children.map((c) => c.tag),
    ["span", "span"],
  );
});

test("a partial listing shows the partial note", async () => {
  const reply = { ...fixture("file.entries"), truncated: true };
  const rows = await draw(reply);
  assert.equal(rows.length, reply.entries.length + 1);
  assert.match(rows.at(-1).textContent, /partial/u);
  const whole = await draw(fixture("file.entries"));
  assert.equal(whole.length, reply.entries.length);
});

test("a zip, a tar and a tgz open in the archive viewer", () => {
  for (const name of ["a.zip", "b.TAR", "c.tgz"]) {
    assert.equal(classify(name), "archive", name);
  }
});
