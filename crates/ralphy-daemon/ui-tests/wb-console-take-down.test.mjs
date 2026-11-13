// Unit tests for assets/ui/wb-console-take-down.ts: a window leaves the stage
// whole, and a throw from the layout after it does not reach the caller, which
// spawns the window's replacement next (#904, #925).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { takeOffStage } from "../assets/ui/wb-console-take-down.ts";

function stage(applyExtent) {
  const win = {
    removed: false,
    remove() {
      this.removed = true;
    },
  };
  const untracked = [];
  const wins = new Set([win]);
  const deps = {
    wins,
    budget: { untrackDormancy: (w) => untracked.push(w) },
    applyExtent,
  };
  return { win, wins, untracked, deps };
}

test("takeOffStage removes, untracks and forgets the window, then lays out", () => {
  let laidOut = 0;
  const { win, wins, untracked, deps } = stage(() => {
    laidOut += 1;
  });
  takeOffStage(deps, win, "test");
  assert.equal(win.removed, true);
  assert.deepEqual(untracked, [win]);
  assert.equal(wins.has(win), false);
  assert.equal(laidOut, 1);
});

test("takeOffStage survives a layout throw and logs it", (t) => {
  const { win, wins, deps } = stage(() => {
    throw new Error("a sibling's wake failed");
  });
  const logged = t.mock.method(console, "error", () => {});
  assert.doesNotThrow(() => takeOffStage(deps, win, "placeholder relaunch"));
  assert.equal(win.removed, true);
  assert.equal(wins.has(win), false);
  assert.equal(logged.mock.callCount(), 1);
  assert.match(String(logged.mock.calls[0].arguments[0]), /placeholder relaunch/u);
});

// Both take-downs go through the one rule: a placeholder's Launch or Attach
// (#925) and a restart (#904).
test("the placeholder and the restart take a window down with takeOffStage", () => {
  const ui = new URL("../assets/ui/", import.meta.url);
  const placeholder = readFileSync(new URL("wb-console-placeholder.ts", ui), "utf8");
  const spawn = readFileSync(new URL("wb-console-spawn.ts", ui), "utf8");
  assert.match(placeholder, /const drop = [^]*?takeOffStage\(/u);
  assert.match(spawn, /const discard = [^]*?takeOffStage\(/u);
});
