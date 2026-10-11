// Unit tests for assets/ui/wb-release-store.ts, the Alpine store `release`:
// the release view the daemon computed, its dismissal and whether the last
// read is current. A store test builds it without Alpine (ADR-0073 D3, D6), as
// `main.ts` builds it for `Alpine.store`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { releaseStore } from "../assets/ui/wb-release-store.ts";
import { WBRelease } from "../assets/ui/wb-release.ts";

// The store alone, on a window stub that keeps its listeners.
function storeOnly() {
  const listeners = {};
  globalThis.window = {
    Alpine: { store: () => ({ fmtClock: () => "14:02" }) },
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
    dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
  };
  const store = releaseStore({ messages: { flash() {} } });
  return { store, window: globalThis.window };
}

// The release view is read again each time the tab comes back (ADR-0056 §7).
// These pin the two rules that make a repeated read safe: a failed read keeps
// what the page knew, and a newer release undoes the dismissal of an older one.
test("loadRelease keeps the last view when the read fails", async () => {
  const { store: state } = storeOnly();
  const known = { ...WBRelease.EMPTY, latest: "v0.1.0-rc.26", severity: "notable", gap: [{}] };
  state.release = known;
  WBRelease.read = async () => null;
  await state.loadRelease();
  assert.equal(state.release, known);
});

test("loadRelease shows a newer release again after the older one was dismissed", async () => {
  const { store: state } = storeOnly();
  const view = (latest) => ({ ...WBRelease.EMPTY, latest, severity: "notable", gap: [{}] });
  WBRelease.read = async () => view("v0.1.0-rc.26");
  await state.loadRelease();
  state.releaseSeen = true;

  await state.loadRelease();
  assert.equal(state.releaseSeen, true, "the same release stays dismissed");

  WBRelease.read = async () => view("v0.1.0-rc.27");
  await state.loadRelease();
  assert.equal(state.releaseSeen, false, "a newer release is news again");
  assert.equal(state.releaseUnread, true);
});

// The store reads the view when `main.ts` registers it, and again on each
// `workbench:release-reread` that the store `connection` sends.
test("init reads the release view, and reads it again when the page asks", () => {
  const { store, window } = storeOnly();
  let reads = 0;
  store.loadRelease = () => reads++;
  store.init();
  assert.equal(reads, 1, "read at registration");
  window.dispatchEvent({ type: "workbench:release-reread", detail: null });
  assert.equal(reads, 2, "read again on workbench:release-reread");
});
