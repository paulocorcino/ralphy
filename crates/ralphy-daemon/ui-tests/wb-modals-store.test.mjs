// Unit tests for assets/ui/wb-modals-store.ts, the Alpine store `modals`: the
// modal stack, the confirm and prompt dialogs, and the flash line. A store
// test builds it without Alpine (ADR-0073 D3, D6), as `main.ts` builds it for
// `Alpine.store`; a test that needs the page's keys loads the page. The
// scrims of index.html drive the stack in wb-modals.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";
import { modalsStore } from "../assets/ui/wb-modals-store.ts";
import { WBReleaseDialogs } from "../assets/ui/wb-release-dialogs.ts";
import { WBSecurityDialog } from "../assets/ui/wb-security-dialog.ts";
import { WBSettingsDialog } from "../assets/ui/wb-settings-dialog.ts";

// The store alone. It calls the door for nothing: the door ends here.
const storeOnly = () => modalsStore({ messages: {} });

// The Security, Settings and What's new dialogs' open flags are their own
// (wb-security-dialog.ts, wb-settings-dialog.ts, wb-release-dialogs.ts), so the
// shortcuts ask the modal stack for them (ADR-0073 amendment of 2026-10-05).
// They block in the same cases as the flag did: while the dialog is open, under
// another modal too, and not once it closes.
test("the shortcuts are blocked while the Security, Settings or What's new dialog is on the modal stack", () => {
  const keydowns = [];
  const { state, window } = loadShell({
    document: { addEventListener: (type, fn) => type === "keydown" && keydowns.push(fn) },
  });
  // The Ctrl/Cmd+Shift+F listener: the one that opens the files search.
  const filesKey = keydowns.find((fn) => /shiftKey/.test(String(fn)) && /file-search-open/.test(String(fn)));
  assert.ok(filesKey, "app.ts registers the Ctrl+Shift+F listener at load");
  const searches = [];
  state.$store.connection.authChanged(true);
  state.$store.projects.setOpen("o/r");
  window.dispatchEvent = (e) => e.type === "workbench:file-search-open" && searches.push("files");
  window.getShell = () => state;
  const modals = state.$store.modals;
  const press = () =>
    filesKey({ key: "F", ctrlKey: true, metaKey: false, shiftKey: true, altKey: false, preventDefault() {} });
  const scrimEl = { querySelector: () => null };

  assert.equal(modals.consoleShortcutsBlocked(), false, "no modal: not blocked");
  press();
  let expected = ["files"];
  assert.deepEqual(searches, expected);

  for (const flag of [
    WBSecurityDialog.openFlag,
    WBSettingsDialog.openFlag,
    WBReleaseDialogs.whatsNewFlag,
  ]) {
    modals.modalOpened(flag, scrimEl);
    assert.equal(modals.consoleShortcutsBlocked(), true, `${flag} open: blocked`);
    assert.equal(modals.consoleShortcutsBlocked(true), true, "a terminal key too");
    press();
    assert.deepEqual(searches, expected, `${flag}: Ctrl+Shift+F is blocked`);

    modals.modalOpened("confirmModal.open", scrimEl);
    assert.equal(modals.consoleShortcutsBlocked(), true, `a confirm over ${flag}: still blocked`);
    press();
    assert.deepEqual(searches, expected);

    modals.modalClosed("confirmModal.open");
    modals.modalClosed(flag);
    assert.equal(modals.consoleShortcutsBlocked(), false, `${flag} closed: not blocked`);
    press();
    expected = [...expected, "files"];
    assert.deepEqual(searches, expected);
  }
});

// The run and the branch dialogs are asked through the modal stack too, as
// the other dialogs are (ADR-0073 D5): no reader outside a dialog reads its flag.
test("the shortcuts are blocked while the run or the branch dialog is on the modal stack", () => {
  const { state } = loadShell();
  const modals = state.$store.modals;
  const scrimEl = { querySelector: () => null };
  for (const flag of ["runOpen", "branchOpen"]) {
    modals.modalOpened(flag, scrimEl);
    assert.equal(modals.consoleShortcutsBlocked(), true, `${flag} open: blocked`);
    modals.modalClosed(flag);
    assert.equal(modals.consoleShortcutsBlocked(), false, `${flag} closed: not blocked`);
  }
});


// The flash line clears itself: a message that stayed would read as the
// answer to the next act.
test("a flash shows on the line and clears itself after 2.6 s", () => {
  const timers = [];
  const realSetTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (fn, ms) => (timers.push({ fn, ms }), timers.length);
  try {
    const store = storeOnly();
    store.flash("Run started.");
    assert.equal(store.runsActionMsg, "Run started.");
    assert.deepEqual(timers.map((t) => t.ms), [2600]);
    timers[0].fn();
    assert.equal(store.runsActionMsg, "");
  } finally {
    globalThis.setTimeout = realSetTimeout;
  }
});

test("askPrompt settles with the trimmed name the prompt submits", async () => {
  const { state: shell } = loadShell();
  const state = shell.$store.modals;
  const answer = state.askPrompt({ title: "New file in src" });
  assert.equal(state.promptModal.open, true);
  assert.equal(state.promptModal.title, "New file in src");
  // A name that cannot be one directory entry keeps the dialog open.
  state.promptModal.value = "a/b";
  state.promptSubmit();
  assert.equal(state.promptModal.open, true);
  assert.equal(state.promptModal.error, "name cannot contain / or \\");
  state.promptModal.value = "  notes.md ";
  state.promptSubmit();
  assert.equal(state.promptModal.open, false);
  assert.equal(await answer, "notes.md");
});


// The prompt's field writes through the store, so the store stays the one
// writer of its fields: typing keeps the text and clears the refusal.
test("typing in the prompt keeps the text and clears the refusal under it", () => {
  const store = storeOnly();
  store.askPrompt({ title: "New file in src" });
  store.promptTyped("a/b");
  store.promptSubmit();
  assert.equal(store.promptModal.error, "name cannot contain / or \\");
  store.promptTyped("a");
  assert.equal(store.promptModal.value, "a");
  assert.equal(store.promptModal.error, "", "the refusal goes once the operator types");
});
