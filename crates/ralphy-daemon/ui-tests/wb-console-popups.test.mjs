// Unit tests for assets/ui/wb-console-popups.ts — the popup registry: the
// fences this tab detached and their popup entries. `createPopupRegistry` is
// driven with a fake store and a fake heartbeat, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createPopupRegistry } from "../assets/ui/wb-console-popups.ts";

// A registry over a store that records each write, and a heartbeat that
// records each start and stop.
function fakeRegistry() {
  const writes = [];
  const beats = [];
  const popups = createPopupRegistry({
    link: { writeRegistry: (ids, members) => writes.push({ ids, members }) },
    startBeat: () => beats.push("start"),
    stopBeat: () => beats.push("stop"),
  });
  return { popups, writes, beats };
}

test("commitDetached stores the ids with their members and runs the heartbeat only while a fence is detached", () => {
  const { popups, writes, beats } = fakeRegistry();
  popups.put("f1", popups.newPopupEntry(["w1", "w2"]));
  popups.commitDetached(["f1"]);
  assert.equal(popups.isDetached("f1"), true);
  assert.deepEqual(writes.at(-1), { ids: ["f1"], members: { f1: ["w1", "w2"] } });
  assert.deepEqual(beats, ["start"]);

  popups.remove("f1");
  popups.commitDetached([]);
  assert.equal(popups.isDetached("f1"), false);
  assert.deepEqual(writes.at(-1), { ids: [], members: {} });
  assert.deepEqual(beats, ["start", "stop"]);
});

test("an entry added or removed does not change the detached ids: only commitDetached does", () => {
  const { popups, writes } = fakeRegistry();
  popups.put("f1", popups.newPopupEntry());
  assert.equal(popups.has("f1"), true);
  assert.equal(popups.size(), 1);
  assert.equal(popups.isDetached("f1"), false);
  assert.deepEqual(writes, []);

  popups.commitDetached(["f1"]);
  popups.remove("f1");
  assert.equal(popups.has("f1"), false);
  assert.equal(popups.size(), 0);
  assert.equal(popups.entry("f1"), undefined);
  assert.equal(popups.isDetached("f1"), true);
});

test("the ids and the entries a caller reads are copies of the registry's own", () => {
  const { popups } = fakeRegistry();
  const entry = popups.newPopupEntry();
  popups.put("f1", entry);
  popups.commitDetached(["f1"]);

  popups.detachedIds().push("f2");
  assert.equal(popups.isDetached("f2"), false);
  assert.deepEqual(popups.detachedIds(), ["f1"]);

  // A walk that removes the entry it stands on still sees every entry.
  const seen = [];
  popups.put("f2", popups.newPopupEntry());
  for (const [id] of popups.entries()) {
    seen.push(id);
    popups.remove(id);
  }
  assert.deepEqual(seen, ["f1", "f2"]);
  assert.equal(popups.size(), 0);
  // The entry is the same object: the console changes it in place.
  assert.equal(entry.adopted, false);
});

test("detachedMembers reads the popup's members once it answered, an empty answer included", () => {
  const { popups } = fakeRegistry();
  const entry = popups.newPopupEntry(["w1"]);
  popups.put("f1", entry);
  popups.commitDetached(["f1"]);
  // Not answered yet: the restored ids stand in.
  assert.deepEqual(popups.detachedMembers(), { f1: ["w1"] });

  entry.members = [{ id: "w2" }];
  assert.deepEqual(popups.detachedMembers(), { f1: ["w2"] });

  // The operator closed every console in the popup: empty means empty.
  entry.members = [];
  entry.adopted = true;
  assert.deepEqual(popups.detachedMembers(), { f1: [] });
});
