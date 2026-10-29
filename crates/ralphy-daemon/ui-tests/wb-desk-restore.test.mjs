// Unit tests for `mountDetached` in assets/ui/wb-desk-restore.ts: the popup's
// side of a detach, which puts each member on its plane shifted by the fence
// origin. `createDeskRestore` is built with the few `deps` the method reads.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createDeskRestore } from "../assets/ui/wb-desk-restore.ts";

// The restore with fakes for what a member is mounted by. Every mount is
// recorded as `[route, ...arguments]`, in order, with `extent` for the call
// that ends the mount.
function restoreWith() {
  const mounts = [];
  const restore = createDeskRestore({
    window: { WBNotes: { mountDetached: (record) => mounts.push(["note", record]) } },
    popups: { commitDetached() {}, newPopupEntry() {} },
    spawnWindow: (...args) => mounts.push(["window", ...args]),
    spawnPlaceholder: (record) => mounts.push(["placeholder", record]),
    applyExtent: () => mounts.push(["extent"]),
  });
  return { restore, mounts };
}

test("each member is mounted shifted by the fence origin plus 12, by its own route", () => {
  const { restore, mounts } = restoreWith();
  const fence = { rect: { left: 100, top: 50, width: 800 } };
  restore.mountDetached(fence, [
    { id: "n1", kind: "note", rect: { left: 130, top: 70, width: 200 } },
    { id: "w1", kind: "agent", agent: "claude", repo: "o/r", session: 0, rect: { left: 100, top: 50, width: 400 } },
    { id: "w2", kind: "console", repo: "o/r", rect: { left: 0, top: 0 } },
  ]);
  assert.deepEqual(mounts, [
    ["note", { id: "n1", kind: "note", rect: { left: 42, top: 32, width: 200 } }],
    [
      "window",
      { id: 0, repo: "o/r" },
      "claude",
      "o/r",
      { id: "w1", kind: "agent", agent: "claude", repo: "o/r", session: 0, rect: { left: 12, top: 12, width: 400 } },
    ],
    ["placeholder", { id: "w2", kind: "console", repo: "o/r", rect: { left: -88, top: -38 } }],
    ["extent"],
  ]);
});

test("a console with no agent is spawned as a plain console", () => {
  const { restore, mounts } = restoreWith();
  restore.mountDetached({ rect: { left: 0, top: 0 } }, [{ id: "w1", repo: "o/r", session: 3, rect: {} }]);
  assert.equal(mounts[0][2], "console");
});

test("a fence with no rect, a member with no rect and no members all mount from the origin 0", () => {
  const { restore, mounts } = restoreWith();
  restore.mountDetached(null, [{ id: "n1", kind: "note" }]);
  assert.deepEqual(mounts[0][1].rect, { left: 12, top: 12 });
  restore.mountDetached(undefined, null);
  assert.deepEqual(mounts.slice(-1), [["extent"]], "the extent is applied even with nothing to mount");
});
