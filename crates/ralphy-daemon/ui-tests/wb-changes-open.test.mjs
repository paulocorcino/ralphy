// A Changes row opens by the kind of file it names, as the SHELL routes it:
// text becomes a diff tab; an image shows its working copy in the image pane
// (ADR-0049), because both diff sides read text and a binary side would close
// the tab again; any other binary is refused before a tab exists. Driven with
// the harness's empty document, a scripted `WBDaemon`, and the real
// `wb-changes.js`.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { loadShell } from "./harness.mjs";

const CHANGES_SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-changes.js"),
  "utf8",
);

// app.js names its siblings bare, which Node resolves through `globalThis`:
// mirror the fakes there, and take them down after this file.
const GLOBALS = ["WB", "WBDaemon", "WBViewer", "WBChanges"];
after(() => GLOBALS.forEach((k) => delete globalThis[k]));

function shell() {
  const { state, window } = loadShell();
  window.WBView = { patch() {}, read: () => null };
  const observed = [];
  const images = [];
  window.WBDaemon = {
    observe: (verb, payload) => {
      observed.push({ verb, payload });
      return Promise.resolve({ status: "ok", content: "text", blob: { status: "present", content: "text" } });
    },
    withCheckout: (p, c) => (c ? { ...p, checkout: String(c) } : { ...p }),
    readImage: (project, path, onRefused, checkout) => {
      images.push({ project, path, checkout });
      return Promise.resolve("data:image/png;base64,AA==");
    },
  };
  const opened = [];
  window.WBViewer = {
    open: (d) => opened.push(d),
    close() {},
    setActive() {},
    jumpTo() {},
    find() {},
  };
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push({ action, ...detail }) };
  const changesWindow = {};
  new Function("window", CHANGES_SRC)(changesWindow);
  window.WBChanges = changesWindow.WBChanges;
  GLOBALS.forEach((k) => (globalThis[k] = window[k]));
  const flashed = [];
  state._flashAction = (msg) => flashed.push(msg);
  state.$nextTick = (fn) => fn();
  state.openSlug = "owner/repo";
  state.checkouts = {};
  state.syncViewer = () => {};
  return { state, observed, images, opened, emitted, flashed };
}

const tick = () => new Promise((r) => setImmediate(r));
const P = "owner/repo";
const fileTabs = (state) => state.tabs.filter((t) => String(t.id).startsWith("file:"));
const diffTabs = (state) => state.tabs.filter((t) => String(t.id).startsWith("diff:"));

for (const status of ["modified", "untracked", "added"]) {
  test(`an image with status ${status} opens its working copy in the image pane`, async () => {
    const { state, observed, images, opened } = shell();
    state.openDiff(P, { path: "img/logo.png", originalPath: null, status });
    await tick();
    assert.equal(diffTabs(state).length, 0, "no diff tab");
    assert.deepEqual(
      fileTabs(state).map((t) => [t.id, t.kind]),
      [["file:owner/repo:img/logo.png", "image"]],
    );
    assert.deepEqual(images.map((i) => i.path), ["img/logo.png"]);
    assert.deepEqual(observed, [], "no text read");
    assert.equal(opened.length, 1);
    assert.equal(opened[0].ftype, "image");
  });
}

test("a second click on the same image activates its tab", async () => {
  const { state } = shell();
  const entry = { path: "a.png", originalPath: null, status: "modified" };
  state.openDiff(P, entry);
  await tick();
  state.activate("consoles");
  state.openDiff(P, entry);
  await tick();
  assert.equal(fileTabs(state).length, 1);
  assert.equal(state.active, "file:owner/repo:a.png");
});

test("a deleted image is refused and opens no tab", async () => {
  const { state, observed, images, emitted, flashed } = shell();
  const before = state.tabs.length;
  state.openDiff(P, { path: "gone.png", originalPath: null, status: "deleted" });
  await tick();
  assert.equal(state.tabs.length, before, "no tab");
  assert.deepEqual(observed, []);
  assert.deepEqual(images, []);
  assert.deepEqual(emitted.map((e) => [e.action, e.reason]), [["open-refused", "deleted"]]);
  assert.equal(flashed.length, 1);
});

test("another binary is refused before a tab exists", async () => {
  const { state, observed, emitted, flashed } = shell();
  const before = state.tabs.length;
  state.openDiff(P, { path: "doc.pdf", originalPath: null, status: "modified" });
  await tick();
  assert.equal(state.tabs.length, before, "no tab");
  assert.deepEqual(observed, []);
  assert.deepEqual(emitted.map((e) => [e.action, e.reason]), [["open-refused", "binary"]]);
  assert.deepEqual(flashed, ["Cannot open binary files."]);
});

test("a text file still opens a diff tab", async () => {
  const { state, observed, opened } = shell();
  state.openDiff(P, { path: "src/x.rs", originalPath: null, status: "modified" });
  await tick();
  assert.deepEqual(diffTabs(state).map((t) => t.id), ["diff:owner/repo:src/x.rs"]);
  assert.deepEqual(observed.map((o) => o.verb).sort(), ["blob.read", "file.read"]);
  assert.equal(opened[0].ftype, "diff");
});

test("an image under a selected worktree is pinned to it", async () => {
  const { state, images } = shell();
  state.checkouts = { [P]: "wt-a" };
  state.openDiff(P, { path: "a.png", originalPath: null, status: "modified" });
  await tick();
  assert.deepEqual(fileTabs(state).map((t) => [t.id, t.checkout]), [["file:owner/repo@wt-a:a.png", "wt-a"]]);
  assert.equal(images[0].checkout, "wt-a");
});
