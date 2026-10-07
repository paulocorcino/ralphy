// The modals: every `.modal-scrim` in index.html takes its behavior from ONE
// binding, `scrim()` in app.ts. These tests read each scrim's real `x-bind`
// expression out of the document and evaluate it against a real `shell()`, so a
// scrim that stops using the binding, or a binding that stops closing, fails
// here. Alpine itself does not run in this harness: a test calls the binding's
// `x-effect` where Alpine would, after a flag changes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

// Comments dropped first: their prose quotes tags.
const HTML = withoutComments(readFileSync(join(UI, "index.html"), "utf8"));

// The index of the `</div>` that closes the `<div` at `from`.
const divEnd = (from) => {
  const tags = /<div\b|<\/div>/g;
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
const COMPONENTS = Array.from(HTML.matchAll(/<div [^<>]*x-data="(wb\w+)"/g), (m) => ({
  name: m[1],
  start: m.index,
  end: divEnd(m.index),
}));

// The scrims in document order, which is the order Alpine registers their
// window listeners in. `component` is the innermost Alpine component whose
// `x-data` element holds the scrim, or null for a scrim of `shell()`.
const SCRIMS = Array.from(
  HTML.matchAll(/<div class="modal-scrim" x-cloak x-bind="(scrim\('([^']+)'[^"]*)">/g),
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

const bindAll = ({ scopeOf }) =>
  SCRIMS.map((s) => {
    const b = evalIn(scopeOf(s), s.expr);
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
    "addProject.open",
    "addHost.open",
  ]);
  // A flag is set on the scope that holds it, so each scrim must be read in its
  // own component.
  assert.deepEqual(
    Object.fromEntries(SCRIMS.filter((s) => s.component).map((s) => [s.path, s.component])),
    {
      settingsOpen: "wbSettingsDialog",
      securityOpen: "wbSecurityDialog",
      whatsNewOpen: "wbReleaseDialogs",
      aboutOpen: "wbReleaseDialogs",
      "addProject.open": "wbAddProjectDialog",
      "addHost.open": "wbHostsDialog",
    },
  );
  for (const scrim of SCRIMS) {
    const { path } = scrim;
    const view = page();
    const { state } = view;
    const bound = bindAll(view);
    const mine = bound.find((s) => s.path === path);
    setPath(view.scopeOf(scrim), path, true);
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
    const view = page();
    const { state } = view;
    const bound = bindAll(view);
    // A dialog that is its own component keeps its flag in its own scope.
    const scope = view.scopeOf(SCRIMS.find((s) => s.path === under));
    const read = () => under.split(".").reduce((o, k) => o[k], scope);
    setPath(scope, under, true);
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

// A click beside a modal is often a slip, and closing on it throws away what
// the modal holds. So no scrim listens to the pointer: the Alpine binding has no
// click key, and the dialogs the console and notes modules build by hand add no
// listener to their scrim.
test("a click outside a modal does not close it", () => {
  for (const s of bindAll(page())) {
    const pointer = Object.keys(s.b).filter((k) => /^(@|x-on:)(click|mousedown|pointerdown)/.test(k));
    assert.deepEqual(pointer, [], `${s.path}: the scrim must not close on a click`);
  }
  for (const file of ["wb-console.ts", "wb-notes.ts"]) {
    const src = readFileSync(join(UI, file), "utf8");
    assert.doesNotMatch(src, /\bscrim\.(addEventListener\(|on\w+\s*=)/, `${file}: a scrim with a listener`);
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
  assert.equal(dialogs.length, 12);
  for (const tag of dialogs) {
    assert.match(tag, /\baria-modal="true"/, tag.replace(/\s+/g, " "));
  }
});
