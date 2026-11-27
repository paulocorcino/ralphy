// Unit tests for assets/ui/wb-move-dialog.ts, the move destination picker's
// Alpine component (#364). It is built with `loadComponent`, so every test
// here also fails when the component reads a shell() name it does not list in
// `uses`, or assigns a shell() field (ADR-0073 D4). The scrim is driven in
// wb-modals.test.mjs.
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent, partMarkup } from "./harness.mjs";

// The part that holds the move dialog's markup.
const PART = "wb-move-dialog.part.html";

// The component names `WBDaemon` bare, which Node resolves through
// `globalThis`: the fake is set there, and taken down after this file.
after(() => delete globalThis.WBDaemon);

// `state` is the scope the picker's code sees; `shell` is the shell() object
// around it, where a test sets the selected checkout. `listing` answers each
// `tree.list` by the directory it names.
function picker(listing = {}) {
  const sent = [];
  const { scope: state, shell, window } = loadComponent("wbMoveDialog", {
    window: { dispatchEvent: (e) => (sent.push(e), true) },
  });
  const calls = [];
  window.WBDaemon = {
    observe: (verb, payload) => {
      calls.push({ verb, payload });
      const entries = listing[payload.path];
      return Promise.resolve(entries ? { status: "ok", entries } : { status: "error", reason: "not found" });
    },
    withCheckout: (payload, checkout) => (checkout ? { ...payload, checkout: String(checkout) } : { ...payload }),
  };
  globalThis.WBDaemon = window.WBDaemon;
  state.$store.projects.setOpen("o/r");
  return { state, shell, calls, sent };
}

const tick = () => new Promise((r) => { setImmediate(r); });

test("wbMoveDialog lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbMoveDialog");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

// pin: the files open the picker by its event — node runs no Alpine over the markup.
test("the picker opens with workbench:move-open", () => {
  assert.match(partMarkup(PART), /<div class="move-dialog" x-data="wbMoveDialog" @workbench:move-open\.window="openMove\(\$event\.detail\.from\)">/u);
});

test("the picker opens on the parent of the row, in the tree's checkout, and offers only folders it may take", async () => {
  const { state, shell, calls } = picker({
    src: [
      { name: "inner", dir: true },
      { name: "a.txt", dir: false },
      { name: ".git", dir: true },
      { name: ".Ralphy", dir: true },
    ],
  });
  shell.$store.checkouts.checkouts = { "o/r": "wt-a" };
  state.openMove("src/inner");
  await tick();
  assert.deepEqual(calls.at(-1), { verb: "tree.list", payload: { repo: "o/r", path: "src", checkout: "wt-a" } });
  assert.equal(state.movePick.open, true);
  assert.equal(state.movePick.dir, "src");
  // The folder itself, files and the protected directories are not rows.
  assert.deepEqual(state.movePick.entries, []);
  assert.equal(state.movePickBlocked(), true, "src is where it already is");
});

test("Move here sends workbench:move-confirmed with the source and the destination, and closes", async () => {
  const { state, sent } = picker({ "": [{ name: "dst", dir: true }], dst: [] });
  state.openMove("a.txt");
  await tick();
  state.movePickConfirm();
  assert.equal(state.movePick.error, "it is already in that folder");
  assert.equal(sent.length, 0, "a no-op is refused here");
  state.movePickInto("dst");
  await tick();
  state.movePickConfirm();
  assert.equal(state.movePick.open, false);
  assert.deepEqual(
    sent.filter((e) => e.type === "workbench:move-confirmed").map((e) => e.detail),
    [{ from: "a.txt", to: "dst/a.txt" }],
  );
});

test("No folders here shows for an empty list that is not busy and has no error", () => {
  const { state } = picker({});
  state.movePick = { open: true, from: "a", isFolder: false, dir: "", entries: [], busy: false, error: "" };
  assert.equal(state.moveFolderIsEmpty(), true);
  state.movePick.busy = true;
  assert.equal(state.moveFolderIsEmpty(), false);
  state.movePick.busy = false;
  state.movePick.error = "no";
  assert.equal(state.moveFolderIsEmpty(), false);
  state.movePick.error = "";
  state.movePick.entries = [{ name: "x", dir: true }];
  assert.equal(state.moveFolderIsEmpty(), false);
});

test("a refused listing is a reason, not an empty folder", async () => {
  const { state } = picker({});
  state.openMove("a/b.txt");
  await tick();
  assert.deepEqual(state.movePick.entries, []);
  assert.equal(state.movePick.error, "Could not list the folder: it does not exist.");
  assert.equal(state.movePickBlocked(), true);
});
