// Unit tests for assets/ui/wb-checkouts-store.ts, the Alpine store
// `checkouts`: the selected checkout of each project, the worktree listings
// and the branch refusal line. It is built here without Alpine (ADR-0073 D3,
// D6), as `main.ts` builds it for `Alpine.store`, on a page stub that gives it
// the `projects` store, the daemon door, the console module and a window that
// hears what it is sent.
import { test } from "node:test";
import assert from "node:assert/strict";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";
import { checkoutsStore } from "../assets/ui/wb-checkouts-store.ts";

// The open project is `o/r`. `desk` is what `WBConsole.checkouts()` answers
// once the desk lands; `landDesk()` lands it.
function page({ desk = {} } = {}) {
  const projects = projectsStore();
  projects.setOpen("o/r");
  const listeners = {};
  const events = [];
  const flashed = [];
  const hooks = {};
  let landDesk = () => {};
  const landed = new Promise((resolve) => (landDesk = resolve));
  globalThis.window = {
    Alpine: { store: (name) => (name === "projects" ? projects : undefined), nextTick: (fn) => fn() },
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
    dispatchEvent: (e) => {
      events.push({ type: e.type, detail: e.detail ?? null });
      for (const fn of listeners[e.type] || []) fn(e);
      return true;
    },
    WBDaemon: {
      observe: async () => null,
      onUnknownCheckout: (fn) => (hooks.unknownCheckout = fn),
    },
    WBConsole: {
      setCheckout() {},
      checkouts: () => desk,
      whenDeskLoaded: () => landed,
    },
  };
  const store = checkoutsStore({ messages: { flash: (msg) => flashed.push(msg) } });
  const sent = (type) => events.filter((e) => e.type === type).map((e) => e.detail);
  const fire = (type, detail) => globalThis.window.dispatchEvent({ type, detail });
  return { store, projects, events, sent, fire, flashed, hooks, landDesk: () => (landDesk(), landed) };
}

test("checkoutGone resets the checkout to the main tree, and says so", () => {
  const { store, flashed } = page();
  store.setCheckout("o/r", "wt-a");
  // CONTROL: a late reply about another worktree says nothing.
  store.checkoutGone("o/r", "wt-b");
  assert.equal(store.checkoutOf("o/r"), "wt-a");
  assert.deepEqual(flashed, []);
  store.checkoutGone("o/r", "wt-a");
  assert.equal(store.checkoutOf("o/r"), null);
  assert.deepEqual(flashed, ["Worktree wt-a no longer exists. Showing the primary tree."]);
});

test("a new checkout of the open project is sent to the files", () => {
  const { store, sent } = page();
  store.setCheckout("other/repo", "wt-a");
  assert.deepEqual(sent("workbench:checkout-changed"), [], "not the open project");
  store.setCheckout("o/r", "wt-a");
  assert.deepEqual(sent("workbench:checkout-changed"), [null]);
});

test("init takes the unknown-checkout hook and adopts the desk's checkouts once the desk lands", async () => {
  const { store, hooks, landDesk, sent } = page({ desk: { "o/r": "wt-a" } });
  store.init();
  assert.equal(store.checkoutOf("o/r"), null, "the desk has not landed yet");
  await landDesk();
  assert.equal(store.checkoutOf("o/r"), "wt-a");
  assert.deepEqual(sent("workbench:changes-read"), [{ ref: "o/r" }], "the open tree is read under the restored checkout");
  hooks.unknownCheckout("o/r", "wt-a");
  assert.equal(store.checkoutOf("o/r"), null);
});

test("a project change drops the branch refusal of the project that was open", () => {
  const { store, fire } = page();
  store.init();
  store.branchRefused("a branch refusal");
  assert.equal(store.branchError, "a branch refusal");
  fire("workbench:project-changed", { slug: "o/b", previous: "o/r" });
  assert.equal(store.branchError, "");
});

// The project rows and the Changes view read one count per project.
test("a change count read for one project keeps the counts of the others, and a failed read is null", () => {
  const { store } = page();
  assert.deepEqual(store.changesCount, {}, "no project was read");
  store.changeCountRead("o/r", 3);
  store.changeCountRead("o/b", 0);
  assert.deepEqual(store.changesCount, { "o/r": 3, "o/b": 0 });
  store.changeCountRead("o/r", null);
  assert.deepEqual(store.changesCount, { "o/r": null, "o/b": 0 });
});
