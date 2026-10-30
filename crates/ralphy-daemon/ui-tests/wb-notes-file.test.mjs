// Unit tests for assets/ui/wb-notes-file.ts — the file acts of a note card:
// point the card at another file, and rename its file. `createNoteFile` is
// driven with fake `deps` and a fake card, with no page.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createNoteFile } from "../assets/ui/wb-notes-file.ts";

// A card whose rename field holds `value`, opened as a pointing or a rename.
function card(id, value, pointing) {
  const field = { value, hidden: false };
  const path = { hidden: true };
  return {
    dataset: { noteId: id },
    _notePointing: pointing,
    classList: { remove() {} },
    querySelector: (sel) => {
      if (sel === ".note-rename") return field;
      return sel === ".note-path" ? path : null;
    },
  };
}

// The file acts over the desk `records`, with a daemon that answers `reply`.
function noteFile(records, reply = { status: "ok" }) {
  const calls = [];
  const deps = {
    window: {
      WBDaemon: {
        write: async (verb, payload) => {
          calls.push(["write", verb, payload]);
          return reply;
        },
      },
    },
    consoleHost: { notes: () => records },
    messages: null,
    recordOf: (id) => records.find((r) => r.id === id) ?? null,
    patch: (id, fields) => calls.push(["patch", id, fields]),
    render() {},
    paintPath: (el, path) => calls.push(["paintPath", path]),
    paintState: (el, text) => calls.push(["paintState", text]),
    loadInto: (el, record) => calls.push(["loadInto", record.path]),
  };
  return { file: createNoteFile(deps), calls };
}

const tick = () => new Promise((r) => setImmediate(r));

test("pointing a card at a file another card of the same checkout shows is refused", () => {
  const records = [
    { id: "a", repo: "r", checkout: null, path: "a.note" },
    { id: "b", repo: "r", checkout: null, path: "b.note" },
  ];
  const { file, calls } = noteFile(records);
  file.commitRename(card("a", " b.note ", true));
  assert.deepEqual(calls, [["paintState", "Another note already uses that file."]]);
});

test("pointing a card at a file another checkout shows re-aims the card and reads the file", () => {
  const records = [
    { id: "a", repo: "r", checkout: null, path: "a.note" },
    { id: "b", repo: "r", checkout: "wt", path: "b.note" },
  ];
  const { file, calls } = noteFile(records);
  file.commitRename(card("a", "b.note", true));
  assert.deepEqual(calls, [
    ["patch", "a", { path: "b.note" }],
    ["paintPath", "b.note"],
    ["loadInto", "b.note"],
  ]);
});

test("a rename keeps the directory, adds the extension, and follows the file with the record", async () => {
  const records = [{ id: "a", repo: "r", checkout: null, path: "notes/a.note" }];
  const { file, calls } = noteFile(records);
  file.commitRename(card("a", "plan", false));
  await tick();
  assert.deepEqual(calls, [
    ["write", "file.rename", { repo: "r", path: "notes/a.note", to: "notes/plan.note" }],
    ["patch", "a", { path: "notes/plan.note" }],
    ["paintPath", "notes/plan.note"],
    ["paintState", "Renamed"],
  ]);
});

test("a rename to the same name, or to nothing, writes nothing", async () => {
  const records = [{ id: "a", repo: "r", checkout: null, path: "notes/a.note" }];
  const { file, calls } = noteFile(records);
  file.commitRename(card("a", "a.note", false));
  file.commitRename(card("a", "   ", false));
  file.commitRename(card("zz", "x", false));
  await tick();
  assert.deepEqual(calls, []);
});

test("a refused rename says why and leaves the record alone", async () => {
  const records = [{ id: "a", repo: "r", checkout: null, path: "a.note" }];
  const { file, calls } = noteFile(records, { status: "error", message: "the file exists" });
  file.commitRename(card("a", "b", false));
  await tick();
  assert.deepEqual(calls.slice(1), [["paintState", "Could not rename: the file exists."]]);
  assert.deepEqual(calls[0][2].to, "b.note");
});
