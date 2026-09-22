// Text has an encoding at the file verbs (ADR-0036 amendment 2026-09-22), as
// the SHELL folds it: `fetchContent` hands the pane `{content, encoding, bom}`
// or `{refused}` and closes the tab only for `not found`/transport; the tab's
// encoding rides the save and the re-read; a refused write puts the dirty
// mark back. Driven with the harness's empty document and a scripted
// `WBDaemon`; the viewer is a recorder, since the pane's DOM is not the fold.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";

// app.js names its siblings bare (`WBDaemon`, `WBViewer`), which the browser
// resolves through `window` and Node through `globalThis`: mirror the fakes
// there for this file, and take them down after it so no other file inherits
// a daemon that answers.
const GLOBALS = ["WB", "WBDaemon", "WBViewer"];
after(() => GLOBALS.forEach((k) => delete globalThis[k]));

// A shell whose daemon answers `file.read` with `reply` and records every
// `file.write`, and whose viewer records every `open`.
function shellWith(reply) {
  // The write seam subscribes to `workbench:action` at load; record it so a
  // test can fire the pane's `save` the way `WB.emit` would.
  const listeners = [];
  const { state, window, document } = loadShell({
    document: {
      addEventListener: (type, fn) => type === "workbench:action" && listeners.push(fn),
    },
  });
  window.WBView = { patch() {}, read: () => null };
  window.WBMode.isDaemon = () => true;
  const observed = [];
  const written = [];
  window.WBDaemon = {
    observe: (verb, payload) => {
      observed.push({ verb, payload });
      return Promise.resolve(typeof reply === "function" ? reply(payload) : reply);
    },
    write: (verb, payload) => {
      written.push({ verb, payload });
      return Promise.resolve(written.reply ?? { status: "ok" });
    },
    withCheckout: (p, c) => (c ? { ...p, checkout: String(c) } : { ...p }),
    readImage: () => Promise.resolve(null),
  };
  const opened = [];
  const viewer = {
    open: (d) => opened.push(d),
    close() {},
    setActive() {},
    externalChange() {},
    encodingOf: (id) => viewer.encodings?.[id] ?? null,
    saveDone: (id) => (viewer.lastAck = { id, ok: true }),
    saveFailed: (id, reason, reply) => (viewer.lastAck = { id, ok: false, reason, reply }),
    jumpTo() {},
    find() {},
  };
  window.WBViewer = viewer;
  // `WB.emit` dispatches a CustomEvent on the document, which the stub sinks;
  // Node has no CustomEvent on an eval'd `document`, so the seam event is a
  // recorder here.
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push({ action, ...detail }) };
  GLOBALS.forEach((k) => (globalThis[k] = window[k]));
  state.$nextTick = (fn) => fn();
  state.openSlug = "owner/repo";
  state.checkouts = {};
  state.syncViewer = () => {};
  const fire = (detail) => listeners.forEach((fn) => fn({ detail }));
  return { state, window, document, observed, written, opened, viewer, fire, emitted };
}

const tick = () => new Promise((r) => setImmediate(r));

test("a decoded read opens the pane with its encoding and bom", async () => {
  const { state, opened } = shellWith({ status: "ok", content: "§", encoding: "windows-1252", bom: false });
  state.openTab({ project: "owner/repo", path: "a.md", title: "a.md", ftype: "markdown" });
  await tick();
  assert.equal(opened.length, 1);
  assert.equal(opened[0].content, "§");
  assert.equal(opened[0].encoding, "windows-1252");
  assert.equal(opened[0].bom, false);
  assert.equal(opened[0].refused, undefined);
  assert.ok(state.tabs.some((t) => t.path === "a.md"), "the tab is there");
});

test("a binary refusal keeps the tab and opens a refused pane", async () => {
  const { state, opened, emitted } = shellWith({ status: "error", reason: "binary" });
  state.openTab({ project: "owner/repo", path: "x.bin.txt", title: "x.bin.txt", ftype: "code" });
  await tick();
  assert.equal(opened.length, 1, "a pane opened");
  assert.equal(opened[0].refused, "binary");
  assert.equal(opened[0].content, undefined);
  assert.ok(state.tabs.some((t) => t.path === "x.bin.txt"), "the tab stayed");
  assert.deepEqual(emitted.map((e) => [e.action, e.reason]), [["open-refused", "binary"]]);
});

test("not found still closes the tab, and opens nothing", async () => {
  const { state, opened } = shellWith({ status: "error", reason: "not found" });
  state.openTab({ project: "owner/repo", path: "gone.txt", title: "gone.txt", ftype: "code" });
  await tick();
  assert.equal(opened.length, 0);
  assert.ok(!state.tabs.some((t) => t.path === "gone.txt"), "the tab closed");
});

test("a re-attached popup's bytes come back with their encoding", async () => {
  const { state, opened, observed } = shellWith({ status: "ok", content: "never read" });
  state.openTab({
    project: "owner/repo", path: "b.txt", title: "b.txt", ftype: "code",
    content: "edited", encoding: "UTF-16LE", bom: true, checkout: null,
  });
  await tick();
  assert.equal(observed.length, 0, "bytes passed in are not re-read");
  assert.equal(opened[0].content, "edited");
  assert.equal(opened[0].encoding, "UTF-16LE");
  assert.equal(opened[0].bom, true);
});

test("a nudge re-reads a tab with its own encoding as the hint", async () => {
  const { state, observed, viewer } = shellWith({ status: "ok", content: "x", encoding: "Shift_JIS", bom: false });
  state.openTab({ project: "owner/repo", path: "docs/j.txt", title: "j.txt", ftype: "code" });
  await tick();
  viewer.encodings = { "file:owner/repo:docs/j.txt": { encoding: "Shift_JIS", bom: false } };
  observed.length = 0;
  await state.refreshOpenViewers("docs");
  assert.equal(observed.length, 1);
  assert.equal(observed[0].payload.encoding, "Shift_JIS", "the hint is the tab's encoding");
});

test("a save carries the pane's encoding and bom, and the ack clears the pane", async () => {
  const { state, written, viewer, fire } = shellWith({ status: "ok", content: "x" });
  state.openTab({ project: "owner/repo", path: "c.txt", title: "c.txt", ftype: "code", checkout: null });
  await tick();
  fire({ action: "save", project: "owner/repo", path: "c.txt", content: "é", checkout: null, encoding: "windows-1252", bom: false });
  await tick();
  assert.equal(written.length, 1);
  assert.equal(written[0].verb, "file.write");
  assert.equal(written[0].payload.encoding, "windows-1252");
  assert.equal(written[0].payload.bom, undefined, "a false bom is not sent");
  assert.deepEqual(viewer.lastAck, { id: "file:owner/repo:c.txt", ok: true });

  written.reply = { status: "error", reason: "unencodable", char_index: 0 };
  fire({ action: "save", project: "owner/repo", path: "c.txt", content: "→", checkout: null, encoding: "windows-1252", bom: true });
  await tick();
  assert.equal(written[1].payload.bom, true);
  assert.equal(viewer.lastAck.ok, false);
  assert.equal(viewer.lastAck.reason, "unencodable");
  assert.equal(viewer.lastAck.reply.char_index, 0);
});

// --- the pane's side (wb-viewer.js) ---------------------------------------
// The record behind a pane is the fold: what it carries into a save and a
// detach descriptor, and what a refusal does to it. The DOM here is a fake
// that accepts any structure and answers any selector with a fresh node.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const VIEWER_SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-viewer.js"),
  "utf8",
);

function fakeNode() {
  const node = {
    style: {},
    dataset: {},
    classList: { add() {}, remove() {}, toggle: () => false, contains: () => false },
    children: [],
    append(...kids) {
      node.children.push(...kids);
    },
    appendChild(k) {
      node.children.push(k);
    },
    remove() {},
    querySelector: () => fakeNode(),
    querySelectorAll: () => [],
    addEventListener() {},
    focus() {},
  };
  return node;
}

function loadViewer() {
  const emitted = [];
  const window = {
    WB: { emit: (action, detail) => emitted.push({ action, ...detail }) },
    WBMode: { isDaemon: () => true, isDemo: () => false },
    WBFleet: { refSlug: (p) => p },
    WBMonaco: { ready: () => new Promise(() => {}) }, // never boots: no editor
    getShell: () => ({ _flashAction() {}, closeTab() {} }),
    addEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    setTimeout: () => 0,
    clearTimeout() {},
  };
  const mount = fakeNode();
  const document = {
    getElementById: () => mount,
    createElement: () => fakeNode(),
    addEventListener() {},
  };
  new Function("window", "document", VIEWER_SRC)(window, document);
  // The bare names the module uses, mirrored the way the shell tests do it.
  for (const k of ["WB", "WBMonaco", "WBMode"]) globalThis[k] = window[k];
  return { viewer: window.WBViewer, emitted, window };
}
after(() => ["WBMonaco", "WBMode"].forEach((k) => delete globalThis[k]));

test("a pane carries its encoding into the save and the detach descriptor", () => {
  const { viewer, emitted } = loadViewer();
  viewer.open({ id: "t1", project: "o/r", path: "a.txt", ftype: "code", content: "x", encoding: "windows-1252", bom: false, checkout: null });
  assert.deepEqual(viewer.encodingOf("t1"), { encoding: "windows-1252", bom: false });
  const desc = viewer.descOf("t1");
  assert.equal(desc.encoding, "windows-1252");
  assert.equal(desc.bom, false);
  assert.equal(desc.content, "x");

  // An open with no encoding (a demo, a diff) is UTF-8 without a BOM.
  viewer.open({ id: "t2", project: "o/r", path: "b.txt", ftype: "code", content: "y" });
  assert.deepEqual(viewer.encodingOf("t2"), { encoding: "UTF-8", bom: false });

  // "Save with…" changes what the next save carries, and marks the pane dirty.
  viewer.setEncoding("t1", "UTF-16LE", true);
  assert.deepEqual(viewer.encodingOf("t1"), { encoding: "UTF-16LE", bom: true });
  assert.equal(emitted.length, 0, "no seam event yet");
});

test("the dirty mark waits for the daemon's ack and comes back on a refusal", () => {
  const { viewer } = loadViewer();
  viewer.open({ id: "t3", project: "o/r", path: "c.txt", ftype: "code", content: "x", encoding: "UTF-8" });
  const rec = () => viewer.descOf("t3");
  assert.ok(rec(), "open");
  viewer.saveFailed("t3", "unencodable", { char_index: 4 });
  // A failed save leaves the pane's encoding alone; the shell decides what
  // to do with it (the "save as UTF-8" dialog).
  assert.deepEqual(viewer.encodingOf("t3"), { encoding: "UTF-8", bom: false });
  viewer.saveDone("t3");
  viewer.saveFailed("nope", "refused"); // an unknown id is a no-op
});

test("a refused open builds a pane that saves nothing and ignores nudges", () => {
  const { viewer, emitted } = loadViewer();
  viewer.open({ id: "t4", project: "o/r", path: "d.bin", ftype: "code", refused: "binary" });
  const desc = viewer.descOf("t4");
  assert.equal(desc.content, undefined, "no bytes");
  viewer.externalChange("t4", "bytes arrived");
  assert.equal(viewer.descOf("t4").content, undefined, "a nudge does not fill a refused pane");
  assert.equal(emitted.length, 0);
  viewer.close("t4");
  assert.equal(viewer.descOf("t4"), null);
});
