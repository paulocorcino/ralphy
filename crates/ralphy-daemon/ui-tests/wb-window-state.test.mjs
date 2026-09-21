// Unit tests for assets/ui/wb-window-state.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-window-state.js"),
  "utf8",
);

// The module's only load-time global is `window`, which it assigns onto.
function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBWindowState;
}

// The shapes a window is actually found in, named once so each table below
// reads as the situation rather than as a literal.
const live = (id, watching = false) => ({ _term: { sessionId: id, watching } });
const asleep = (id, watching = false) => ({
  _term: null,
  _dormant: true,
  _dormantSession: id,
  _dormantWatch: watching,
  _wantsSession: id,
});
const spawning = (id) => ({ _term: null, _wantsSession: id });

test("initWindow writes every declared field, so no window is born half-stated", () => {
  const { FIELDS, initWindow } = load();
  const win = {};
  initWindow(win);
  assert.deepEqual(Object.keys(win).sort(), Object.keys(FIELDS).sort());
  for (const [name, value] of Object.entries(FIELDS)) {
    assert.equal(win[name], value, `${name} must be born holding its declared default`);
  }
});

test("initWindow applies the seed over the defaults and leaves the rest declared", () => {
  const { initWindow } = load();
  const win = initWindow({}, { _deskId: "d1", _deskRepo: "owner/repo", _deskLocked: true });
  assert.equal(win._deskId, "d1");
  assert.equal(win._deskRepo, "owner/repo");
  assert.equal(win._deskLocked, true);
  // Untouched by the seed, and still declared rather than absent.
  assert.equal(win._wantsSession, null);
  assert.equal(win._dormant, false);
  assert.equal(win._visible, true);
});

// The whole point of declaring the inventory: a field nothing states is a field
// nothing can notice. `_sessionOwner` was written on every announcement and read
// nowhere for months because there was no place for it to look wrong.
test("initWindow refuses a field the inventory does not declare", () => {
  const { initWindow } = load();
  assert.throws(() => initWindow({}, { _sessionOwner: "01ARZ3NDEKTSV4RRFFQ69G5FAY" }), {
    message: /unknown window field _sessionOwner/,
  });
});

test("initWindow overwrites a recycled element rather than merging into it", () => {
  const { initWindow } = load();
  const win = { _deskId: "old", _wantsSession: 7, _dormant: true };
  initWindow(win, { _deskId: "new" });
  assert.equal(win._deskId, "new");
  assert.equal(win._wantsSession, null);
  assert.equal(win._dormant, false);
});

test("sessionIdOf answers one id from whichever source the window has", () => {
  const { sessionIdOf, initWindow } = load();
  const table = [
    ["a live terminal", live(4), 4],
    ["a sleeping console", asleep(9), 9],
    ["spawned, not yet announced", spawning(12), 12],
    ["a placeholder", initWindow({}), null],
    ["nothing at all", null, null],
  ];
  for (const [what, win, want] of table) {
    assert.equal(sessionIdOf(win), want, what);
  }
});

// Id `0` is a real session id, and every formula this replaced used `??` rather
// than `||` for exactly that reason. Pinned so a later "simplification" cannot
// quietly make session 0 unreachable.
test("sessionIdOf treats id 0 as an id, not as absence", () => {
  const { sessionIdOf } = load();
  assert.equal(sessionIdOf(live(0)), 0);
  assert.equal(sessionIdOf(asleep(0)), 0);
  assert.equal(sessionIdOf(spawning(0)), 0);
});

// The two camps that never consulted each other. A sleeping window carries BOTH
// ids, and a live handle outranks both — a window that woke onto a different
// session must not answer with the one it went to sleep holding.
test("sessionIdOf prefers the live handle over either carried id", () => {
  const { sessionIdOf } = load();
  assert.equal(sessionIdOf({ ...asleep(9), _term: { sessionId: 4 } }), 4);
  assert.equal(sessionIdOf({ _term: null, _dormantSession: 9, _wantsSession: 3 }), 9);
});

test("watchingOf answers false for a window with no session to watch", () => {
  const { watchingOf, initWindow } = load();
  assert.equal(watchingOf(live(4, true)), true);
  assert.equal(watchingOf(live(4, false)), false);
  assert.equal(watchingOf(asleep(9, true)), true);
  assert.equal(watchingOf(asleep(9, false)), false);
  assert.equal(watchingOf(spawning(12)), false);
  assert.equal(watchingOf(initWindow({})), false);
  assert.equal(watchingOf(null), false);
});

// A live handle is authoritative even when it disagrees with the flag carried
// across dormancy: waking onto a busy session parks the window as a watcher
// (ADR-0051 §9), and the stale flag must not outvote that.
test("watchingOf prefers the live handle over the carried flag", () => {
  const { watchingOf } = load();
  assert.equal(watchingOf({ ...asleep(9, false), _term: { sessionId: 9, watching: true } }), true);
  assert.equal(watchingOf({ ...asleep(9, true), _term: { sessionId: 9, watching: false } }), false);
});

test("windowCheckout lets the daemon's announcement beat the request beat the record", () => {
  const { windowCheckout, initWindow } = load();
  const announced = initWindow({}, {
    _sessionCheckout: "wt/announced",
    _deskCheckout: "wt/recorded",
  });
  assert.equal(windowCheckout(announced, "wt/asked"), "wt/announced");

  const unannounced = initWindow({}, { _deskCheckout: "wt/recorded" });
  assert.equal(windowCheckout(unannounced, "wt/asked"), "wt/asked");
  assert.equal(windowCheckout(unannounced, undefined), "wt/recorded");

  assert.equal(windowCheckout(initWindow({}), undefined), null);
});

// The primary worktree is `null`, not a name, and `null` from the daemon is an
// ANSWER: a console announced as running in the primary must not fall through
// to a stale recorded worktree.
test("windowCheckout does not treat the primary worktree as an unanswered question", () => {
  const { windowCheckout, initWindow } = load();
  const win = initWindow({}, { _sessionCheckout: null, _deskCheckout: "wt/stale" });
  // `_sessionCheckout` is null (its default) — indistinguishable from "never
  // announced", which is why the DESK mirror is overwritten on announcement too.
  assert.equal(windowCheckout(win, undefined), "wt/stale");
  // What the announcement actually does: it writes both, and then they agree.
  win._deskCheckout = null;
  assert.equal(windowCheckout(win, undefined), null);
});
