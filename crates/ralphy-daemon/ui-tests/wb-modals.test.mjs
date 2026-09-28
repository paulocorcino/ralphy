// The modals: every `.modal-scrim` in index.html takes its behavior from ONE
// binding, `scrim()` in app.js. These tests read each scrim's real `x-bind`
// expression out of the document and evaluate it against a real `shell()`, so a
// scrim that stops using the binding, or a binding that stops closing, fails
// here. Alpine itself does not run in this harness: a test calls the binding's
// `x-effect` where Alpine would, after a flag changes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadShell, UI } from "./harness.mjs";

// Comments dropped first: their prose quotes tags.
const HTML = readFileSync(join(UI, "index.html"), "utf8").replace(/<!--[\s\S]*?-->/g, "");

// The scrims in document order, which is the order Alpine registers their
// window listeners in.
const SCRIMS = Array.from(
  HTML.matchAll(/<div class="modal-scrim" x-cloak x-bind="(scrim\('([^']+)'[^"]*)">/g),
  (m) => ({ expr: m[1], path: m[2] }),
);

// Evaluate a markup expression the way Alpine does: names resolve on the
// component. `with` needs sloppy mode, which `new Function` gives.
const evalIn = (state, expr) => new Function("scope", `with (scope) { return ${expr}; }`)(state);

const setPath = (state, path, value) => {
  const keys = path.split(".");
  const last = keys.pop();
  keys.reduce((o, k) => o[k], state)[last] = value;
};

// A scrim element: it finds no dialog and no controls, so the focus step does
// nothing. The focus test below builds a richer one.
const bareScrim = () => ({ querySelector: () => null, querySelectorAll: () => [], contains: () => false });

const bindAll = (state) =>
  SCRIMS.map((s) => {
    const b = evalIn(state, s.expr);
    const el = bareScrim();
    return { ...s, b, effect: () => b["x-effect"].call({ $el: el }) };
  });

// What the browser does with one Escape keydown: every scrim's window listener
// hears the SAME event, and the microtasks (Alpine's effects) run after each
// listener returns, before the next one.
const pressEscape = (bound) => {
  const event = { key: "Escape" };
  for (const s of bound) {
    s.b["@keydown.escape.window"](event);
    for (const t of bound) t.effect();
  }
};

test("Escape closes every modal, Settings and Security included", () => {
  const paths = SCRIMS.map((s) => s.path);
  assert.deepEqual(paths, [
    "settingsOpen",
    "securityOpen",
    "whatsNewOpen",
    "aboutOpen",
    "branchOpen",
    "confirmModal.open",
    "promptModal.open",
    "movePick.open",
    "planModal.open",
    "runOpen",
  ]);
  for (const { path } of SCRIMS) {
    const { state } = loadShell();
    const bound = bindAll(state);
    const mine = bound.find((s) => s.path === path);
    setPath(state, path, true);
    for (const s of bound) s.effect();
    assert.equal(mine.b["x-show"](), true, `${path}: the binding shows the open modal`);
    pressEscape(bound);
    assert.equal(mine.b["x-show"](), false, `${path}: Escape must close it`);
    assert.deepEqual(state._modalStack, [], `${path}: the closed modal leaves the stack`);
  }
});

// Two rows because the window asks the scrims in document order: Plan is asked
// after the confirm, Settings before it.
test("one Escape closes only the top modal", async () => {
  for (const under of ["planModal.open", "settingsOpen"]) {
    const { state } = loadShell();
    const bound = bindAll(state);
    const read = () => under.split(".").reduce((o, k) => o[k], state);
    setPath(state, under, true);
    for (const s of bound) s.effect();
    const answer = state.askConfirm({ title: "Discard" });
    for (const s of bound) s.effect();

    pressEscape(bound);
    // The flag first: on a failure the promise never settles, and awaiting it
    // would stall the run instead of failing this test.
    assert.equal(state.confirmModal.open, false, `${under}: the confirm on top is cancelled`);
    assert.equal(await answer, false);
    assert.equal(read(), true, `${under}: the modal under it stays open`);

    pressEscape(bound);
    assert.equal(read(), false, `${under}: the next Escape closes it`);
  }
});

test("a modal focuses its first control on open and gives focus back on close", () => {
  const focused = [];
  const el = (name, extra = {}) => ({
    name,
    disabled: false,
    isConnected: true,
    classList: { contains: (c) => (extra.classes || []).includes(c) },
    getClientRects: () => (extra.hidden ? [] : [{}]),
    focus: () => focused.push(name),
    ...extra,
  });
  const opener = el("opener");
  const controls = [
    el("close", { classes: ["modal-x"] }),
    el("hidden", { hidden: true }),
    el("disabled", { disabled: true }),
    el("field"),
    el("button"),
  ];
  let focusInside = false;
  const dialog = { contains: () => focusInside, querySelectorAll: () => controls };
  const scrimEl = { querySelector: () => dialog };

  const { state } = loadShell({
    window: { requestAnimationFrame: (fn) => fn() },
    document: { activeElement: opener },
  });
  const b = state.scrim("runOpen", () => state.closeRunModal());
  const effect = () => b["x-effect"].call({ $el: scrimEl });

  state.runOpen = true;
  effect();
  assert.deepEqual(focused, ["field"], "the first visible, enabled control that is not the ✕");

  state.closeRunModal();
  effect();
  assert.deepEqual(focused, ["field", "opener"], "focus returns to the element that opened it");

  // A modal that focuses its own field on open keeps that focus.
  focused.length = 0;
  focusInside = true;
  state.runOpen = true;
  effect();
  assert.deepEqual(focused, []);
});

test("every dialog in the document is marked modal", () => {
  const dialogs = Array.from(HTML.matchAll(/<div\b(?:[^>"]|"[^"]*")*>/g), (m) => m[0]).filter((t) =>
    /\brole="(?:alert)?dialog"/.test(t),
  );
  assert.equal(dialogs.length, 10);
  for (const tag of dialogs) {
    assert.match(tag, /\baria-modal="true"/, tag.replace(/\s+/g, " "));
  }
});
