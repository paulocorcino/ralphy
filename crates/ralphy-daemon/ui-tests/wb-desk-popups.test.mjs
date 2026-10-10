// Unit tests for assets/ui/wb-desk-popups.ts — the popup registry: the
// fences this tab detached and their popup entries. `createPopupRegistry` is
// driven with a fake store and a fake heartbeat, with no console and no
// browser.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createPopupRegistry, isPopupMember } from "../assets/ui/wb-desk-popups.ts";

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

// A popup and its opener post members to each other, and either one can be a
// page of another build: a member is read only in the shape this page knows.
test("a member from another window is read only in its own shape", () => {
  const win = { id: "w1", kind: null, agent: "claude", repo: "o/r", consoleName: null, session: 4, rect: { left: 1, top: 2, width: 3, height: 4 } };
  const note = { id: "n1", kind: "note", path: "notes/a.md", draft: "text", claim: null, rect: { left: 1 } };
  assert.equal(isPopupMember(win), true);
  assert.equal(isPopupMember(note), true);
  assert.equal(isPopupMember({ id: "w2" }), true, "every field but the id may be absent");
  for (const bad of [
    { ...win, id: 7 },
    { ...win, session: "4" },
    { ...win, agent: 1 },
    { ...win, rect: { left: "1" } },
    { ...note, path: null },
    { ...note, draft: 3 },
    [win],
  ]) {
    assert.equal(isPopupMember(bad), false, JSON.stringify(bad));
  }
});
