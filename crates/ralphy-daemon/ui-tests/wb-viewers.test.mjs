// Unit tests for assets/ui/wb-viewers.ts, the table of the file viewers by
// kind, and for the image viewer's address on the file bytes route
// (assets/ui/wb-file-bytes.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { VIEWERS } from "../assets/ui/wb-viewers.ts";
import { classify } from "../assets/ui/wb-file-paths.ts";
import { fetchContent } from "../assets/ui/wb-tabs-store-open.ts";
import { createViewer } from "../assets/ui/wb-file-viewer.ts";
import { createVendorLoader, VENDOR_FAILURE_TEXT } from "../assets/ui/wb-vendor-load.ts";
import { idLookups } from "./fake-document.mjs";

// An element that remembers one child per selector, so a test reads back the
// `<img>` the pane wrote.
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
    classList: { add() {}, remove() {}, toggle: () => false, contains: () => false },
    append(...kids) {
      el.children.push(...kids);
    },
    querySelector(sel) {
      if (!found.has(sel)) found.set(sel, fakeEl(sel));
      return found.get(sel);
    },
    querySelectorAll: () => [],
    addEventListener() {},
    setAttribute() {},
  };
  return el;
}

function fakeViewer() {
  const mount = fakeEl("viewers");
  const window = { WB: { emit() {} }, addEventListener() {}, matchMedia: () => ({ matches: false }) };
  const document = {
    ...idLookups((id) => (id === "viewers" ? mount : null)),
    createElement: (tag) => fakeEl(tag),
    addEventListener() {},
    dispatchEvent() {},
  };
  const viewer = createViewer(window, document, { messages: { flash() {} } });
  return { viewer, mount };
}

test("an image tab's image address is the file bytes route with repo, path and checkout", () => {
  const { viewer, mount } = fakeViewer();
  viewer.open({
    id: "t1",
    project: "me/proj",
    path: "docs/a b.png",
    ftype: "image",
    checkout: "feat x",
    content: "",
  });
  const img = mount.children[0].querySelector(".img-canvas");
  const url = new URL(img.src, "http://daemon");
  assert.equal(url.pathname, "/api/file/bytes");
  assert.equal(url.searchParams.get("repo"), "me/proj");
  assert.equal(url.searchParams.get("path"), "docs/a b.png");
  assert.equal(url.searchParams.get("checkout"), "feat x");
});

test("a new kind in the table opens with its own builder and reads no verb", async () => {
  const built = [];
  VIEWERS.testkind = {
    extensions: ["tkx"],
    build: (_host, rec) => {
      built.push(rec.path);
    },
  };
  const verbs = [];
  globalThis.WBDaemon = {
    observe: (verb) => {
      verbs.push(verb);
      return Promise.resolve({ status: "ok", content: "" });
    },
    readImage: () => {
      verbs.push("file.image");
      return Promise.resolve(null);
    },
    withCheckout: (p) => p,
  };
  try {
    assert.equal(classify("x/data.TKX"), "testkind");
    const store = { closeTab() {} };
    const body = await fetchContent(store, {
      project: "p",
      path: "x/data.tkx",
      ftype: "testkind",
      checkout: null,
    });
    assert.notEqual(body, null);
    assert.deepEqual(verbs, []);
    const { viewer } = fakeViewer();
    viewer.open({ id: "t2", project: "p", path: "x/data.tkx", ftype: "testkind", ...body });
    assert.deepEqual(built, ["x/data.tkx"]);
  } finally {
    delete VIEWERS.testkind;
    delete globalThis.WBDaemon;
  }
});

test("an extension outside the table and the code kinds is still refused as binary", () => {
  for (const name of ["a.zip", "b.exe", "c.rar", "d.MKV"]) {
    assert.equal(classify(name), "binary", name);
  }
  assert.equal(classify("main.rs"), "code");
});

test("an avif file classifies as image", () => {
  assert.equal(classify("photo.avif"), "image");
  assert.equal(classify("PHOTO.AVIF"), "image");
});

test("loadVendor imports a path once: a second load of it returns the same promise", async () => {
  const asked = [];
  const load = createVendorLoader((path) => {
    asked.push(path);
    return Promise.resolve({ path });
  });
  const first = load("./vendor/lib/a.mjs");
  assert.equal(load("./vendor/lib/a.mjs"), first);
  assert.deepEqual(await first, { ok: true, module: { path: "./vendor/lib/a.mjs" } });
  assert.equal(load("./vendor/lib/a.mjs"), first, "a finished load is shared too");
  load("./vendor/lib/b.mjs");
  assert.deepEqual(asked, ["./vendor/lib/a.mjs", "./vendor/lib/b.mjs"]);
});

test("loadVendor turns a failed import into the viewer's failure text, not a throw", async () => {
  let calls = 0;
  const load = createVendorLoader(() => {
    calls += 1;
    return Promise.reject(new Error("404"));
  });
  const outcome = await load("./vendor/lib/missing.mjs");
  assert.deepEqual(outcome, { ok: false, text: VENDOR_FAILURE_TEXT });
  await load("./vendor/lib/missing.mjs");
  assert.equal(calls, 2, "after a failure, the next open tries again");
});
