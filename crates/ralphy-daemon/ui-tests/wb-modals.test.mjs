// The modals: every `.modal-scrim` in index.html takes its behavior from ONE
// binding, `scrim()` of the Alpine store `modals` (wb-modals-store.ts). These
// tests read each scrim's real `x-bind` expression out of the document and
// evaluate it against a real page, so a scrim that stops using the binding, or
// a binding that stops closing, fails here. Alpine itself does not run in this
// harness: a test calls each function of the binding as Alpine would, with the
// scrim's scope as `this`, and calls `x-effect` after a flag changes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { assembledPage, loadComponent, loadShell, UI, valueIn as evalIn, withoutComments } from "./harness.mjs";

// The whole page with its parts put in: the order of the scrims is document
// order. Comments dropped first: their prose quotes tags.
const HTML = withoutComments(assembledPage("index.html"));

// The index of the `</div>` that closes the `<div` at `from`.
const divEnd = (from) => {
  const tags = /<div\b|<\/div>/gu;
  tags.lastIndex = from;
  let depth = 0;
  for (let t = tags.exec(HTML); t; t = tags.exec(HTML)) {
    depth += t[0] === "</div>" ? -1 : 1;
    if (depth === 0) return t.index;
  }
  return HTML.length;
};

// Each element that holds a component nested in `shell()`, and the text it spans.
// One element may hold several scrims (wbReleaseDialogs holds two).
const COMPONENTS = Array.from(HTML.matchAll(/<div [^<>]*x-data="(wb\w+)"/gu), (m) => ({
  name: m[1],
  start: m.index,
  end: divEnd(m.index),
}));

// The scrims in document order, which is the order Alpine registers their
// window listeners in. `component` is the innermost Alpine component whose
// `x-data` element holds the scrim, or null for a scrim of `shell()`.
const SCRIMS = Array.from(
  HTML.matchAll(/<div class="modal-scrim" x-cloak x-bind="(\$store\.modals\.scrim\('([^']+)'[^"]*)">/gu),
  (m) => ({
    expr: m[1],
    path: m[2],
    component: COMPONENTS.filter((c) => c.start < m.index && m.index < c.end).at(-1)?.name || null,
  }),
);

// One page: `shell()` and each component nested in it, as the browser builds
// them. `scopeOf(s)` is the scope a scrim's markup sees.
const page = () => {
  const loaded = loadShell();
  const scopes = new Map();
  for (const s of SCRIMS) {
    if (s.component && !scopes.has(s.component)) {
      scopes.set(s.component, loadComponent(s.component, { from: loaded }).scope);
    }
  }
  return { state: loaded.state, scopeOf: (s) => (s.component ? scopes.get(s.component) : loaded.state) };
};

const setPath = (state, path, value) => {
  const keys = path.split(".");
  const last = keys.pop();
  keys.reduce((o, k) => o[k], state)[last] = value;
};

// A scrim element: it finds no dialog and no controls, so the focus step does
// nothing. The focus test below builds a richer one.
const bareScrim = () => ({ querySelector: () => null, querySelectorAll: () => [], contains: () => false });

// The `this` Alpine gives a function of the binding: the scrim's scope, with
// the `$el` magic.
const withEl = (scope, $el) => new Proxy(scope, { get: (t, k) => (k === "$el" ? $el : Reflect.get(t, k)) });

const bindAll = ({ scopeOf }) =>
  SCRIMS.map((s) => {
    const scope = scopeOf(s);
    const b = evalIn(scope, s.expr);
    const el = bareScrim();
    return {
      ...s,
      b,
      show: () => b["x-show"].call(scope),
      escape: (e) => b["@keydown.escape.window"].call(scope, e),
      effect: () => b["x-effect"].call(withEl(scope, el)),
    };
  });

// What the browser does with one Escape keydown: every scrim's window listener
// hears the SAME event, and the microtasks (Alpine's effects) run after each
// listener returns, before the next one.
const pressEscape = (bound) => {
  const event = { key: "Escape" };
  for (const s of bound) {
    s.escape(event);
    for (const t of bound) t.effect();
  }
};

test("Escape closes every modal, Settings and Security included", () => {
  const paths = SCRIMS.map((s) => s.path);
  for (const flag of ["settingsOpen", "securityOpen"]) assert.ok(paths.includes(flag), flag);
  // A flag is set on the scope that holds it, so each scrim must be read in its
  // own component: the component that holds the scrim owns its flag.
  for (const s of SCRIMS.filter((x) => x.component)) {
    const { data } = loadComponent(s.component);
    assert.ok(s.path.split(".")[0] in data, s.path + " belongs to " + s.component);
  }
  for (const scrim of SCRIMS) {
    const { path } = scrim;
    const view = page();
    const { state } = view;
    const bound = bindAll(view);
    const mine = bound.find((s) => s.path === path);
    setPath(view.scopeOf(scrim), path, true);
    for (const s of bound) s.effect();
    assert.equal(mine.show(), true, `${path}: the binding shows the open modal`);
    pressEscape(bound);
    assert.equal(mine.show(), false, `${path}: Escape must close it`);
    assert.deepEqual(state.$store.modals._modalStack, [], `${path}: the closed modal leaves the stack`);
  }
});

// Three rows because the window asks the scrims in document order: Plan and
// Settings are asked before the confirm, Move after it.
test("one Escape closes only the top modal", async () => {
  for (const under of ["planModal.open", "settingsOpen", "movePick.open"]) {
    const view = page();
    const { state } = view;
    const bound = bindAll(view);
    // A dialog that is its own component keeps its flag in its own scope.
    const scope = view.scopeOf(SCRIMS.find((s) => s.path === under));
    const read = () => under.split(".").reduce((o, k) => o[k], scope);
    setPath(scope, under, true);
    for (const s of bound) s.effect();
    const answer = state.$store.modals.askConfirm({ title: "Discard" });
    for (const s of bound) s.effect();

    pressEscape(bound);
    // The flag first: on a failure the promise never settles, and awaiting it
    // would stall the run instead of failing this test.
    assert.equal(state.$store.modals.confirmModal.open, false, `${under}: the confirm on top is cancelled`);
    assert.equal(await answer, false);
    assert.equal(read(), true, `${under}: the modal under it stays open`);

    pressEscape(bound);
    assert.equal(read(), false, `${under}: the next Escape closes it`);
  }
});

// A click beside a modal is often a slip, and closing on it throws away what
// the modal holds. So no scrim listens to the pointer: the Alpine binding has no
// click key, and the dialogs the console and notes modules build by hand add no
// listener to their scrim.
// pin: no scrim listens to the pointer —
// the scrims built by hand are only source text, and node runs no browser events.
test("a click outside a modal does not close it", () => {
  for (const s of bindAll(page())) {
    const pointer = Object.keys(s.b).filter((k) => /^(@|x-on:)(click|mousedown|pointerdown)/u.test(k));
    assert.deepEqual(pointer, [], `${s.path}: the scrim must not close on a click`);
  }
  for (const file of ["wb-console.ts", "wb-notes.ts"]) {
    const src = readFileSync(join(UI, file), "utf8");
    assert.doesNotMatch(src, /\bscrim\.(addEventListener\(|on\w+\s*=)/u, `${file}: a scrim with a listener`);
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

  // The run modal is the component `wbRuns`: its scope holds the flag.
  const { scope: state } = loadComponent("wbRuns", {
    window: { requestAnimationFrame: (fn) => fn() },
    document: { activeElement: opener },
  });
  const b = state.$store.modals.scrim("runOpen", () => state.closeRunModal());
  const effect = () => b["x-effect"].call(withEl(state, scrimEl));

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

test("Tab and Shift+Tab stay inside the open modal", () => {
  const { scope: state } = loadComponent("wbRuns", { window: { requestAnimationFrame: () => {} } });
  // The page's stub document, which the store reads.
  const doc = globalThis.document;
  const el = (name) => {
    const e = { name, disabled: false, getClientRects: () => [{}], classList: { contains: () => false } };
    e.focus = () => (doc.activeElement = e);
    return e;
  };
  const [field, cancel, ok] = [el("field"), el("cancel"), el("ok")];
  const behind = el("rail button");
  const inside = new Set([field, cancel, ok]);
  const dialog = { contains: (x) => inside.has(x), querySelectorAll: () => [field, cancel, ok] };
  const scrimEl = { querySelector: () => dialog };
  const b = state.$store.modals.scrim("runOpen", () => state.closeRunModal());
  const tab = (shiftKey = false) => {
    const e = { key: "Tab", shiftKey, prevented: false, preventDefault: () => (e.prevented = true) };
    b["@keydown.tab.window"].call(withEl(state, scrimEl), e);
    return e;
  };
  state.runOpen = true;
  b["x-effect"].call(withEl(state, scrimEl));

  doc.activeElement = ok;
  assert.equal(tab().prevented, true);
  assert.equal(doc.activeElement, field, "Tab on the last control goes to the first");
  assert.equal(tab(true).prevented, true);
  assert.equal(doc.activeElement, ok, "Shift+Tab on the first control goes to the last");

  doc.activeElement = cancel;
  assert.equal(tab().prevented, false, "between controls the browser moves the focus");

  doc.activeElement = behind;
  tab();
  assert.equal(doc.activeElement, field, "focus behind the scrim comes back to the modal");

  // A confirm asked over this modal is now the top: this one keeps out of it.
  state.$store.modals.modalOpened("$store.modals.confirmModal.open", scrimEl);
  doc.activeElement = ok;
  assert.equal(tab().prevented, false);
  assert.equal(doc.activeElement, ok);
});

// pin: every dialog has aria-modal and sits in a scrim —
// node runs no Alpine over the markup, so only its text shows the attributes.
test("every dialog in the document is marked modal", () => {
  const dialogs = Array.from(HTML.matchAll(/<div\b(?:[^>"]|"[^"]*")*>/gu), (m) => m[0]).filter((t) =>
    /\brole="(?:alert)?dialog"/u.test(t),
  );
  assert.ok(dialogs.length > 0, "the page has dialogs");
  assert.equal(SCRIMS.length, dialogs.length, "each dialog sits in a scrim of the modal stack");
  for (const tag of dialogs) {
    assert.match(tag, /\baria-modal="true"/u, tag.replaceAll(/\s+/gu, " "));
  }
});

// The page is built from parts, and each part goes back to the place its
// marker holds: every component a part declares is in the page.
// pin: every component a part declares is in the assembled page —
// the page is markup, and node runs no Alpine over it.
test("a page put together from its parts", () => {
  const names = new Set(COMPONENTS.map((c) => c.name));
  const parts = readdirSync(UI).filter((f) => f.endsWith(".part.html"));
  let declared = 0;
  for (const part of parts) {
    const markup = readFileSync(join(UI, part), "utf8");
    for (const [, name] of markup.matchAll(/<div [^<>]*x-data="(wb\w+)"/gu)) {
      assert.ok(names.has(name), `${part}: ${name} is in the page`);
      declared++;
    }
  }
  assert.ok(declared > 0, "the parts declare components");
  assert.doesNotMatch(assembledPage("index.html"), /<!-- part:/u, "a marker is left in the page");
});
