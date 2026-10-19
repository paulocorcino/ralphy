// Unit tests for assets/ui/wb-files-upload.ts: the upload dialog's Alpine
// component and the drop on the files tree. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses` (ADR-0073 D4).
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, UI } from "./harness.mjs";
import { askUpload, conflictFor, limitMb } from "../assets/ui/wb-files-upload.ts";

const HTML = readFileSync(join(UI, "index.html"), "utf8");
const MB = 1024 * 1024;

after(() => {
  delete globalThis.WBDaemon;
  delete globalThis.window;
});

// A file of `size` bytes; only its name and size are read before the upload.
const file = (name, size = 3) => ({ name, size, slice: () => new Blob([new Uint8Array(0)]) });

// The dialog over a folder that holds `names`, in a project whose limit is
// `limit` MB. `written` lists each part the store sends.
function dialog({ names = [], limit = 25 } = {}) {
  const { scope: state, window } = loadComponent("wbUploadDialog");
  const written = [];
  window.WBDaemon = {
    observe: async (verb) =>
      verb === "config.get"
        ? { status: "ok", config: { "files.upload_max_mb": limit } }
        : { status: "ok", entries: names.map((name) => ({ name, dir: false })) },
    write: async (verb, payload) => (written.push({ verb, payload }), { status: "ok", path: payload.path }),
  };
  globalThis.WBDaemon = window.WBDaemon;
  return { state, written };
}

const tick = () => new Promise((r) => setImmediate(r));

test("every name the markup of wbUploadDialog reads is its own", () => {
  const { scope: state, data } = loadComponent("wbUploadDialog");
  assert.deepEqual(data.uses, []);
  const markup = componentMarkup(HTML, "wbUploadDialog");
  let read = 0;
  for (const [, attr, value] of markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)) {
    if (attr === "x-data") continue;
    const raw = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
    for (const n of bindingNames(raw)) {
      if (n.startsWith("$") || n === "r") continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 8, `the markup was read: ${read} names`);
  assert.match(HTML, /x-data="wbUploadDialog" @workbench:upload-open\.window="openUpload\(\$event\.detail\)"/);
});

test("the dialog refuses a file over the limit and marks the names that exist", async () => {
  const { state } = dialog({ names: ["a.txt"], limit: 1 });
  await state.openUpload({ repo: "o/r", folder: "docs", files: [file("A.txt"), file("big.bin", MB + 1), file("new.md")] });
  assert.equal(state.upload.busy, false);
  assert.deepEqual(
    state.upload.rows.map((r) => [r.name, r.tooBig, r.exists]),
    [
      ["A.txt", false, true],
      ["big.bin", true, false],
      ["new.md", false, false],
    ],
  );
  assert.equal(state.uploadHasExisting(), true);
  assert.equal(state.uploadRowText(state.upload.rows[1]), "Over the upload limit");
});

test("one choice applies to every name that exists; skip leaves them out", async () => {
  const { state } = dialog({ names: ["a.txt"] });
  await state.openUpload({ repo: "o/r", folder: "", files: [file("a.txt"), file("b.txt")] });
  const plan = () => state.uploadRequests().map((r) => [r.file.name, r.onConflict, r.folder, r.repo]);
  assert.deepEqual(plan(), [
    ["a.txt", "keep_both", "", "o/r"],
    ["b.txt", "fail", "", "o/r"],
  ]);
  state.upload.choice = "replace";
  assert.deepEqual(plan()[0], ["a.txt", "replace", "", "o/r"]);
  state.upload.choice = "skip";
  assert.deepEqual(plan(), [["b.txt", "fail", "", "o/r"]]);
});

test("Upload gives the files to the uploads store and closes", async () => {
  const { state, written } = dialog();
  await state.openUpload({ repo: "o/r", folder: "x", files: [file("n.txt", 0)] });
  state.uploadConfirm();
  assert.equal(state.upload.open, false);
  await tick();
  assert.deepEqual(
    written.map((w) => [w.verb, w.payload.path, w.payload.on_conflict]),
    [["file.upload", "x/n.txt", "fail"]],
  );
  assert.equal(state.$store.uploads.items[0].state, "done");
});

test("the limit is read as the daemon reads it", () => {
  assert.equal(limitMb(undefined), 25);
  assert.equal(limitMb(0), 25);
  assert.equal(limitMb(3), 3);
  assert.equal(limitMb(5000), 1024);
  assert.equal(conflictFor({ exists: false }, "skip"), "fail");
  assert.equal(conflictFor({ exists: true }, "skip"), null);
});

// The tree's side of a drop: `createDir` of wb-files.ts, and the stores it reads.
function tree({ checkout = "" } = {}) {
  const flashed = [];
  const opened = [];
  globalThis.window = { dispatchEvent: (e) => (opened.push(e.detail), true) };
  const t = {
    createDir: (node) => (node ? (node.folder ? node.rel : node.rel.split("/").slice(0, -1).join("/")) : ""),
    $store: {
      modals: { flash: (m) => flashed.push(m) },
      projects: { openSlug: "o/r" },
      checkouts: { checkoutOf: () => checkout },
    },
  };
  return { t, flashed, opened };
}

test("a drop on a file opens the dialog for its folder", () => {
  const { t, opened, flashed } = tree();
  askUpload(t, { rel: "src/main.rs", folder: false }, [file("x.txt")]);
  assert.deepEqual(flashed, []);
  assert.deepEqual(
    opened.map((d) => [d.repo, d.folder, d.files.map((f) => f.name)]),
    [["o/r", "src", ["x.txt"]]],
  );
});

test("a dropped folder, a worktree and a protected folder are refused with a message", () => {
  const dir = [{ webkitGetAsEntry: () => ({ isDirectory: true }) }];
  let { t, opened, flashed } = tree();
  askUpload(t, null, [file("photos")], dir);
  assert.match(flashed[0], /not folders/);
  ({ t, opened, flashed } = tree({ checkout: "wt-a" }));
  askUpload(t, null, [file("x.txt")]);
  assert.match(flashed[0], /worktree/);
  ({ t, opened, flashed } = tree());
  askUpload(t, { rel: ".git/hooks", folder: true }, [file("x.txt")]);
  assert.match(flashed[0], /cannot write/);
  assert.deepEqual(opened, []);
});
