// Unit tests for assets/ui/wb-viewer-font.ts, the font pane: the sample text
// at three sizes once the browser loads the face, the message when it refuses
// the file, and the reading of a file larger than one answer.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildFont,
  FONT_ERROR_TEXT,
  FONT_SAMPLE_TEXT,
  liveFontDeps,
} from "../assets/ui/wb-viewer-font.ts";
import { VIEWERS, viewerKindOf } from "../assets/ui/wb-viewers.ts";

// An element that records its attributes, children and listeners, and finds
// the children a selector names through stubs.
function fakeEl(tag) {
  const listeners = new Map();
  const stubs = new Map();
  const el = {
    tag,
    attrs: {},
    style: {},
    dataset: {},
    className: "",
    textContent: "",
    children: [],
    setAttribute(name, value) {
      el.attrs[name] = value;
    },
    append(...kids) {
      el.children.push(...kids);
    },
    addEventListener(name, fn) {
      listeners.set(name, fn);
    },
    fire(name) {
      listeners.get(name)?.({});
    },
    querySelector(sel) {
      return stubs.get(sel) ?? null;
    },
    stub(sel) {
      const child = fakeEl(sel);
      stubs.set(sel, child);
      return child;
    },
  };
  return el;
}

// The pane of one file, with the two steps of the font pane supplied by the test.
function pane(deps) {
  const mount = fakeEl("viewers");
  const stubs = {};
  const document = {
    createElement: (tag) => {
      const el = fakeEl(tag);
      if (tag === "div") {
        for (const sel of [".font-meta", ".font-body", '[data-act="detach"]', ".viewer-path"]) {
          stubs[sel] = el.stub(sel);
        }
      }
      return el;
    },
  };
  const host = {
    document,
    viewers: mount,
    setPathLabel() {},
    detachBtnHtml: () => "",
    detachClick() {},
  };
  const rec = {
    id: "t",
    uid: 7,
    project: "me/proj",
    path: "fonts/a b.woff2",
    checkout: "feat x",
    content: "",
    refused: null,
  };
  buildFont(host, rec, deps);
  return { rec, stubs, body: stubs[".font-body"], meta: stubs[".font-meta"] };
}

// The steps of the font pane once the bytes arrive: the fetch and the face.
function steps({ addFace = () => Promise.resolve() } = {}) {
  const asked = [];
  return {
    asked,
    deps: {
      fetchBytes: (url) => {
        asked.push(url);
        return Promise.resolve(new Uint8Array([0, 1, 0, 0]));
      },
      addFace,
    },
  };
}

// Lets every promise the pane chains settle: they resolve in microtasks.
function settle() {
  return new Promise((resolve) => {
    setImmediate(resolve);
  });
}

test("a loaded font draws the sample at three sizes in the family it loaded", async () => {
  const faces = [];
  const { deps } = steps({
    addFace: (family, bytes) => {
      faces.push({ family, length: bytes.length });
      return Promise.resolve();
    },
  });
  const { body, meta } = pane(deps);
  await settle();
  assert.deepEqual(faces, [{ family: "ralphy-font-7", length: 4 }]);
  const samples = body.children;
  assert.equal(samples.length, 3);
  assert.equal(new Set(samples.map((p) => p.style.fontSize)).size, 3, "three sizes");
  for (const p of samples) {
    assert.equal(p.style.fontFamily, "ralphy-font-7");
    assert.equal(p.textContent, FONT_SAMPLE_TEXT);
  }
  assert.equal(meta.textContent, "");
});

test("a refused font (FontFace.load rejects) shows the message and no sample", async () => {
  const { deps } = steps({ addFace: () => Promise.reject(new Error("invalid font data")) });
  const { body, meta } = pane(deps);
  await settle();
  assert.equal(meta.textContent, FONT_ERROR_TEXT);
  assert.equal(body.children.length, 0);
});

test("a font larger than one answer is read again from the next byte", async () => {
  const asked = [];
  const answers = [
    { status: 206, total: 5, part: [1, 2, 3] },
    { status: 206, total: 5, part: [4, 5] },
  ];
  const realFetch = globalThis.fetch;
  globalThis.fetch = (_url, init) => {
    asked.push(init.headers.Range);
    const next = answers.shift();
    return Promise.resolve({
      ok: true,
      status: next.status,
      headers: { get: () => `bytes 0-4/${next.total}` },
      arrayBuffer: () => Promise.resolve(new Uint8Array(next.part).buffer),
    });
  };
  try {
    const bytes = await liveFontDeps({ fonts: { add() {} } }).fetchBytes("/api/file/bytes");
    assert.deepEqual([...bytes], [1, 2, 3, 4, 5]);
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(asked, ["bytes=0-", "bytes=3-"]);
});

test("the table opens the four font extensions with the font builder", () => {
  for (const ext of ["ttf", "otf", "woff", "woff2"]) {
    assert.equal(viewerKindOf(ext), "font", ext);
  }
  assert.equal(VIEWERS.font.build, buildFont);
});
