// The list of page shortcuts and its one listener (wb-commands.ts): the keys
// the list refuses, and when a key is taken. No browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { COMMANDS, bindCommand, commandKeys, dispatchKey, listProblems } from "../assets/ui/wb-commands.ts";

const cmd = (id, keys) => ({ id, group: "Page", label: id, keys, scope: "page" });

test("the list holds no refused key and no key twice", () => {
  assert.deepEqual(listProblems(COMMANDS), []);
});

test("the list refuses a typed letter, a Ctrl key other than Ctrl+Shift+F, Alt+Shift+Tab and a key used twice", () => {
  const problems = listProblems([
    cmd("a", [{ kind: "typed", key: "a" }]),
    cmd("b", [{ kind: "modShift", key: "p" }]),
    cmd("c", [{ kind: "chord", code: "Tab" }]),
    cmd("d", [{ kind: "chord", code: "KeyQ" }]),
    cmd("e", [{ kind: "chord", code: "KeyQ" }]),
    cmd("ok", [
      { kind: "typed", key: "?" },
      { kind: "modShift", key: "f" },
      { kind: "chord", code: "KeyP" },
    ]),
  ]);
  assert.equal(problems.length, 4, problems.join("\n"));
  for (const id of ["a:", "b:", "c:", "e:"]) {
    assert.ok(
      problems.some((p) => p.startsWith(id)),
      `${id} is refused`,
    );
  }
});

function chord(code, mods = {}) {
  const e = {
    key: "",
    code,
    altKey: true,
    shiftKey: true,
    ctrlKey: false,
    metaKey: false,
    taken: false,
    ...mods,
  };
  e.preventDefault = () => (e.taken = true);
  return e;
}
const free = () => false;
const pageBlocked = (fromTerminal) => !fromTerminal;

test("a key is taken only when its command can run and does something", () => {
  const runs = [];
  bindCommand("consoles.row", {
    can: (code) => code !== "Digit9",
    run: (code) => (runs.push(code), code !== "Digit8"),
  });
  const ran = chord("Digit1");
  assert.equal(dispatchKey(ran, "page", free), true);
  assert.equal(ran.taken, true);
  const cannot = chord("Digit9");
  assert.equal(
    dispatchKey(cannot, "page", free),
    false,
    "a command that cannot run leaves the key",
  );
  assert.equal(cannot.taken, false);
  const nothing = chord("Digit8");
  assert.equal(
    dispatchKey(nothing, "page", free),
    false,
    "a command that did nothing leaves the key",
  );
  assert.equal(nothing.taken, false);
  assert.deepEqual(runs, ["Digit1", "Digit8"]);
});

test("a page command never runs from a terminal, and a blocked page runs none", () => {
  const runs = [];
  bindCommand("search.open", { can: () => true, run: () => void runs.push("search") });
  bindCommand("consoles.run", { can: () => true, run: () => void runs.push("run") });
  const slash = () => ({ ...chord(""), key: "/", altKey: false, shiftKey: false });
  assert.equal(dispatchKey(slash(), "terminal", free), false, "the search is a page command");
  assert.equal(
    dispatchKey(chord("KeyR"), "terminal", free),
    true,
    "the consoles menu passes a terminal",
  );
  assert.equal(dispatchKey(slash(), "page", pageBlocked), false);
  assert.equal(
    dispatchKey(chord("KeyR"), "page", () => true),
    false,
  );
  assert.equal(dispatchKey(slash(), "page", free), true);
  assert.deepEqual(runs, ["run", "search"]);
});

test("a key a terminal ran is not run again by the document listener", () => {
  let runs = 0;
  bindCommand("consoles.run", { can: () => true, run: () => void runs++ });
  const e = chord("KeyR");
  assert.equal(dispatchKey(e, "terminal", free), true);
  assert.equal(dispatchKey(e, "page", free), true);
  assert.equal(runs, 1);
});

test("a key with another modifier is no chord", () => {
  let runs = 0;
  bindCommand("consoles.run", { can: () => true, run: () => void runs++ });
  for (const mods of [
    { ctrlKey: true },
    { metaKey: true },
    { shiftKey: false },
    { altKey: false },
  ]) {
    assert.equal(dispatchKey(chord("KeyR", mods), "page", free), false, JSON.stringify(mods));
  }
  assert.equal(runs, 0);
});

test("a command's keys are written for the system, where the page shows them", () => {
  assert.equal(commandKeys("files.search", false), "Ctrl+Shift+F");
  assert.equal(commandKeys("files.search", true), "⌘⇧F");
  assert.equal(commandKeys("no.such", false), "");
});
