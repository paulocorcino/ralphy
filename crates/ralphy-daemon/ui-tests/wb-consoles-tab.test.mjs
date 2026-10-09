// Unit tests for assets/ui/wb-consoles-tab.ts, the two Alpine components of
// the Consoles tab: `wbConsoleMenus` (the toolbar menus and their keys) and
// `wbColumns` (the columns beside a maximized console, the footer pills, and
// the console module's events). Each is built with `loadComponent`, so every
// test here also fails when a component reads a shell() name it does not list
// in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// One page with both components and a window and a document that deliver the
// events they are sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds each component.
function page() {
  const listeners = { window: {}, document: {} };
  const on = (where) => (type, fn) => (listeners[where][type] ||= []).push(fn);
  const send = (where) => (e) => {
    for (const fn of listeners[where][e.type] || []) fn(e);
    return true;
  };
  const loaded = loadShell({
    window: { addEventListener: on("window"), dispatchEvent: send("window") },
    document: { addEventListener: on("document"), dispatchEvent: send("document") },
  });
  const magics = { $nextTick: () => {} };
  const menus = loadComponent("wbConsoleMenus", { from: loaded, magics });
  const columns = loadComponent("wbColumns", { from: loaded, magics });
  menus.scope.init();
  columns.scope.init();
  const fire = (where, type, detail) => send(where)({ type, detail });
  const key = (code, mods = { altKey: true, shiftKey: true }) => {
    const e = { type: "keydown", code, ctrlKey: false, metaKey: false, prevented: false, ...mods };
    e.preventDefault = () => (e.prevented = true);
    send("document")(e);
    return e;
  };
  return { shell: loaded.state, menus: menus.scope, columns: columns.scope, fire, key, listeners };
}

// ADR-0063 §3: a NEW console opens in the checkout selected at the moment of
// the click, and in the primary when none is — the console keeps it afterwards
// (the title comes from the daemon's announcement, never from this selection).
// A console is born in the primary whatever the Files chip shows (ADR-0063,
// amendment 2026-09-16 b): the selection is what the panels LOOK at; only the
// console's own title switcher moves it.
test("newConsole opens the agent in the primary even under a selected checkout", () => {
  const { scope: state, shell } = loadComponent("wbConsoleMenus");
  const calls = [];
  const real = globalThis.WBConsole;
  // The component reaches `WBConsole` as a bare global; the real one needs a DOM.
  globalThis.WBConsole = { open: (o) => calls.push(o), count: () => 0 };
  try {
    shell.active = "consoles";
    state.$store.projects.setOpen("o/r");
    shell.checkouts = { "o/r": "wt-a" };
    state.newConsole("claude");
    shell.checkouts = {};
    state.newConsole("codex");
  } finally {
    globalThis.WBConsole = real;
  }
  assert.deepEqual(calls, [
    { repo: "o/r", agent: "claude", checkout: null },
    { repo: "o/r", agent: "codex", checkout: null },
  ]);
});

// ADR-0051 §5: the same chord walks the columns while two or more consoles
// are open, and the fences otherwise. The listener itself is a sink in the
// harness; the decision lives in `arrowStep`. The listener is tested below.
function arrowShell() {
  const { scope: state, shell } = loadComponent("wbColumns");
  const calls = [];
  const box = { focused: null };
  const realConsole = globalThis.WBConsole;
  globalThis.WBConsole = {
    stepFence: (s) => (calls.push(["fence", s]), { id: "f" }),
    focusedId: () => box.focused,
    focusColumn: (id) => calls.push(["col", id]),
    columnMeasure: () => ({ viewport: 2000 }),
    PHONE_MAX_WIDTH: 560,
  };
  const done = () => {
    globalThis.WBConsole = realConsole;
  };
  return { state, shell, calls, box, done };
}

test("Alt+Shift+←/→ walks the columns while they are open and the fences otherwise", () => {
  const { state, shell, calls, box, done } = arrowShell();
  try {
    shell.active = "consoles";
    state.columns = [];
    assert.ok(state.arrowStep("x", 1));
    assert.deepEqual(calls, [["fence", 1]]);
    calls.length = 0;
    state.columns = [["a"], ["b"], ["c"]];
    box.focused = "c";
    assert.ok(state.arrowStep("x", 1));
    assert.deepEqual(calls, [["col", "a"]], "wraps right");
    calls.length = 0;
    box.focused = "a";
    assert.ok(state.arrowStep("x", -1));
    assert.deepEqual(calls, [["col", "c"]], "wraps left");
    assert.ok(!calls.some((c) => c[0] === "fence"), "no fence step while columns are open");
  } finally {
    done();
  }
});

// Rows (ADR-0051 §5): ↑/↓ walk the rows of one column, and one column of two
// rows is enough. With nothing open, ↑/↓ apply nothing, so the key is not
// swallowed.
test("Alt+Shift+↑/↓ walks the rows of a column, and applies nothing with no columns", () => {
  const { state, shell, calls, box, done } = arrowShell();
  try {
    shell.active = "consoles";
    state.columns = [];
    assert.equal(state.arrowStep("y", 1), false);
    assert.deepEqual(calls, [], "no fence step for ↑/↓");
    state.columns = [["a", "b"]];
    box.focused = "a";
    assert.ok(state.arrowStep("y", 1));
    assert.deepEqual(calls, [["col", "b"]]);
  } finally {
    done();
  }
});

test("the Note menu keeps a card on top, puts it back, and refuses a card in a popup", () => {
  const { scope: state, shell, window } = loadComponent("wbConsoleMenus");
  const calls = [];
  window.WBNotes = {
    putBack: () => calls.push("putBack"),
    keepOnTop: (id) => calls.push("keepOnTop:" + id),
    list: () => [{ id: "a", onTop: false, away: false }],
  };
  state.$nextTick = (fn) => fn();
  shell.active = "consoles";
  state.noteMenu = true;

  // Putting back keeps the menu open and redraws the rows.
  state.toggleOnTop({ id: "a", onTop: true, away: false });
  assert.deepEqual(calls, ["putBack"]);
  assert.equal(state.noteMenu, true);
  assert.deepEqual(state.noteItems, [{ id: "a", onTop: false, away: false }]);

  // Keeping on top closes the menu, so the card is in view.
  state.toggleOnTop({ id: "a", onTop: false, away: false });
  assert.deepEqual(calls, ["putBack", "keepOnTop:a"]);
  assert.equal(state.noteMenu, false);

  // A card in a detached popup: nothing happens.
  state.toggleOnTop({ id: "b", onTop: false, away: true });
  assert.equal(calls.length, 2);
});

test("Alt+Shift+R from another tab opens the Consoles tab and its menu", () => {
  const { scope: state, shell } = loadComponent("wbConsoleMenus");
  const calls = [];
  state.$nextTick = () => {};
  shell.activate = (tab) => {
    calls.push("activate:" + tab);
    shell.active = tab;
  };
  shell.active = "code";

  state.openConsoleRunMenu();
  // The menu sits in the Consoles tab's toolbar: on another tab it is hidden.
  assert.deepEqual(calls, ["activate:consoles"]);
  assert.equal(state.agentMenu, true);
  assert.equal(state.consoleRunOpen, true);
});

test("keeping a card on top from another tab opens the Consoles tab first", () => {
  const { scope: state, shell, window } = loadComponent("wbConsoleMenus");
  const calls = [];
  window.WBNotes = { keepOnTop: (id) => calls.push("keepOnTop:" + id) };
  const ticks = [];
  state.$nextTick = (fn) => ticks.push(fn);
  shell.activate = (tab) => {
    calls.push("activate:" + tab);
    shell.active = tab;
  };
  shell.active = "code";

  state.toggleOnTop({ id: "a", onTop: false, away: false });
  // The card is placed only after the tab is shown: a hidden tab measures 0×0.
  assert.deepEqual(calls, ["activate:consoles"]);
  ticks.forEach((fn) => fn());
  assert.deepEqual(calls, ["activate:consoles", "keepOnTop:a"]);
});

// The console module names the consoles whose records left the desk; the
// columns lose them first, then the stage does — every one of them, a console
// outside the columns too.
test("checkColumnDesk takes the consoles that left the desk out of the columns, then off the stage", () => {
  const { scope: state } = loadComponent("wbColumns");
  const order = [];
  const realConsole = globalThis.WBConsole;
  globalThis.WBConsole = {
    applyColumns: () => order.push("columns"),
    dropClosedElsewhere: (id) => order.push(`drop ${id}`),
  };
  state.columns = [["x"], ["y"], ["z"]];
  state.columnCap = () => 3;
  state.setColumns = (c) => (state.columns = c);
  state.paintColumns = () => {};
  try {
    state.checkColumnDesk(["x", "loose"]);
  } finally {
    globalThis.WBConsole = realConsole;
  }
  assert.deepEqual(state.columns, [["y"], ["z"]]);
  assert.deepEqual(order, ["columns", "drop x", "drop loose"]);
});

// ADR-0051 §5: when the shell's columns move the maximize, the move is written
// to the desk. The torn-off fence window paints without `persist`.
test("every shell path that paints the columns asks to write the maximize", () => {
  const { scope: state, window, document } = loadComponent("wbColumns");
  const calls = [];
  const paints = [];
  const realConsole = globalThis.WBConsole;
  globalThis.WBConsole = {
    ...window.WBConsole,
    applyColumns: (painted, opts) => {
      calls.push(opts);
      paints.push(painted.map((p) => p.id));
    },
    columnMeasure: () => ({ viewport: 1920 }),
    dropClosedElsewhere() {},
    focusColumn() {},
    focusedId: () => null,
  };
  const grid = () => [["a"], ["b"], ["c"]];
  state.columnCap = () => 3;
  const realAll = document.querySelectorAll;
  // The stage holds only "b": the first console left, and one is left.
  const lone = () => {
    document.querySelectorAll = () => [{ _deskId: "b", classList: { contains: () => false } }];
    state.paintColumns();
  };
  try {
    // Each path, and how many paints it makes: its own, then `paintColumns`.
    const paths = {
      restoreColumn: [2, () => state.restoreColumn("a")],
      swapColumn: [
        2,
        () => {
          state.columnFrom = "b";
          state.swapColumn("x");
        },
      ],
      columnsFromFence: [
        2,
        () =>
          state.columnsFromFence([
            { id: "p", rect: { left: 0, top: 0, width: 100, height: 100 } },
            { id: "q", rect: { left: 200, top: 0, width: 100, height: 100 } },
          ]),
      ],
      leaveColumns: [1, () => state.leaveColumns(["a"])],
      checkColumnDesk: [2, () => state.checkColumnDesk(["a"])],
      paintColumns: [1, () => state.paintColumns()],
      "paintColumns, a lone survivor": [1, lone],
    };
    for (const [name, [count, run]] of Object.entries(paths)) {
      state.columns = grid();
      calls.length = 0;
      document.querySelectorAll = realAll;
      run();
      assert.equal(calls.length, count, `${name} paints the columns`);
      for (const opts of calls) assert.equal(opts?.persist, true, `${name}: ${JSON.stringify(opts)}`);
    }
    // The lone-survivor case above took its own branch: it paints the survivor
    // and ends the columns.
    state.columns = grid();
    paints.length = 0;
    lone();
    assert.deepEqual(paints, [["b"]], "the lone survivor is painted");
    assert.deepEqual(state.columns, [], "the lone survivor ends the columns");
    // A first console off the stage for a relaunch comes back under the same
    // id: nothing is painted or stored until it is back.
    globalThis.WBConsole.isRelaunching = (id) => id === "a";
    state.columns = grid();
    calls.length = 0;
    lone();
    assert.deepEqual([calls.length, state.columns], [0, grid()], "a relaunch gap paints nothing");
    const stage = (...wins) => {
      document.querySelectorAll = () =>
        wins.map(([id, max]) => ({ _deskId: id, classList: { contains: (c) => max && c === "maximized" } }));
    };
    // The same with two consoles left: the grid path.
    stage(["b"], ["c"]);
    state.columns = grid();
    calls.length = 0;
    state.paintColumns();
    assert.deepEqual([calls.length, state.columns], [0, grid()], "a relaunch gap on a grid paints nothing");
    // A first console back on the stage paints, even while still marked.
    stage(["a", true], ["b"], ["c"]);
    calls.length = 0;
    state.paintColumns();
    assert.equal(calls.length, 1, "a first console back on the stage paints");
  } finally {
    document.querySelectorAll = realAll;
    globalThis.WBConsole = realConsole;
  }
});

// The two components share nothing but the event below; each owns its names.
for (const name of ["wbConsoleMenus", "wbColumns"]) {
  test(`${name} lists only shell() names in uses, and its members left shell()`, () => {
    const { data, shell } = loadComponent(name);
    for (const n of data.uses) assert.ok(n in shell, n);
    for (const n of Object.keys(data)) {
      if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
    }
  });

  test(`every name the markup of ${name} reads is its own or in its uses list`, () => {
    const { scope: state } = loadComponent(name);
    const markup = componentMarkup(HTML, name);
    const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
    // `x-data` names the component; `x-for` adds its loop variables.
    const loopVars = new Set();
    for (const [, attr, value] of attrs) {
      if (attr !== "x-for") continue;
      const [left] = value.split(/\s+in\s+/);
      for (const v of bindingNames(left)) loopVars.add(v);
    }
    let read = 0;
    for (const [, attr, value] of attrs) {
      // `x-ref` names an element, not a scope member.
      if (attr === "x-data" || attr === "x-ref") continue;
      const raw = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
      // A template literal reads only the names in its `${…}` holes.
      const expr = raw.startsWith("`") ? [...raw.matchAll(/\$\{([^}]*)\}/g)].map((m) => m[1]).join(";") : raw;
      for (const n of bindingNames(expr)) {
        // `window` is the page's global, not a name of any scope.
        if (loopVars.has(n) || n.startsWith("$") || n === "window") continue;
        assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
        read++;
      }
    }
    assert.ok(read > 10, `the markup was read: ${read} names`);
  });
}

test("nothing outside the two components names their state", () => {
  let outside = HTML;
  for (const name of ["wbConsoleMenus", "wbColumns"]) {
    const start = outside.indexOf(`x-data="${name}"`);
    outside = outside.slice(0, start) + outside.slice(outside.indexOf("<!-- ===", start));
  }
  outside = withoutComments(outside);
  for (const name of ["agentMenu", "toggleAgentMenu", "consoleRunText", "fenceItems", "noteItems", "columnMenu", "columnView", "consoleCount", "stageW"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ONE dropdown at a time: a trigger in one component closes the menus of the
// other. The shell's account menu hears the same event (app.test.mjs).
test("a menu trigger sends workbench:menus-close, and both components close their menus", () => {
  const { menus, columns, listeners } = page();
  assert.equal(listeners.window["workbench:menus-close"].length, 2, "each component listens");
  columns.columnMenu = true;
  menus.toggleAgentMenu();
  assert.equal(menus.agentMenu, true);
  assert.equal(columns.columnMenu, false, "the toolbar's trigger closed the columns list");
  menus.consoleRunOpen = true;
  columns.closeMenus();
  assert.equal(menus.agentMenu, false, "the columns' trigger closed the toolbar's menu");
  assert.equal(menus.consoleRunOpen, false, "and its Run field");
});

// The keys moved out of `wire()` with the menus and the columns. They are
// document listeners, so a terminal that hands a key over still reaches them.
test("the Alt+Shift keys reach the menus and the columns through the listeners the components add", () => {
  const { shell, menus, columns, key } = page();
  const calls = [];
  menus.openConsoleItem = (row) => calls.push("row " + row.digit);
  menus.openConsoleRunMenu = () => calls.push("run");
  menus.jumpFenceAt = (n) => (calls.push("fence " + n), true);
  columns.arrowStep = (axis, step) => (calls.push(`arrow ${axis} ${step}`), true);
  assert.equal(key("Digit0").prevented, true);
  assert.equal(key("KeyR").prevented, true);
  assert.equal(key("F3").prevented, true);
  assert.equal(key("ArrowLeft").prevented, true);
  assert.equal(key("Digit0", { altKey: true, shiftKey: false }).prevented, false, "Alt alone is not the chord");
  assert.deepEqual(calls, ["row 0", "run", "fence 3", "arrow x -1"]);
  shell.authed = false;
  for (const code of ["Digit0", "KeyR", "F3", "ArrowLeft"]) {
    assert.equal(key(code).prevented, false, `${code} is blocked while locked`);
  }
  assert.equal(calls.length, 4);
});

test("the console module's events reach the columns: the count, the stage extent, the list, a restore", () => {
  const { columns, fire } = page();
  const calls = [];
  columns.paintColumns = () => calls.push("paint");
  columns.toggleColumnMenu = (id, rect) => calls.push(`list ${id} ${rect.right}`);
  columns.restoreColumn = (id) => calls.push(`restore ${id}`);
  columns.leaveColumns = (ids) => calls.push(`leave ${ids}`);
  columns.columnsFromFence = (items) => calls.push(`fence ${items.length}`);
  columns.restoreColumns = () => calls.push("restore all");
  fire("document", "workbench:consoles-changed", { count: 3 });
  fire("document", "workbench:stage-extent", { width: 800, height: 600 });
  fire("document", "workbench:column-open", { id: "a", rect: { right: 10 } });
  fire("document", "workbench:column-restore", { id: "b" });
  fire("document", "workbench:columns-stale");
  fire("document", "workbench:columns-leave", { ids: ["c"] });
  fire("document", "workbench:fence-columns", { items: [1, 2] });
  fire("document", "workbench:desk-restored");
  assert.equal(columns.consoleCount, 3);
  assert.deepEqual([columns.stageW, columns.stageH], [800, 600]);
  assert.deepEqual(calls, ["paint", "list a 10", "restore b", "paint", "leave c", "fence 2", "restore all"]);
});

test("the columns give the console module the hook for consoles another client removed", () => {
  const { scope, window } = loadComponent("wbColumns");
  let hook = null;
  window.WBConsole = { ...window.WBConsole, setDeskGoneHook: (fn) => (hook = fn) };
  scope.init();
  const seen = [];
  scope.checkColumnDesk = (ids) => seen.push(ids);
  hook(["x"]);
  assert.deepEqual(seen, [["x"]]);
});

test("the menu head and the columns list name a project without its path- key or daemon id", () => {
  const loaded = loadShell();
  const shell = loaded.state;
  const menus = loadComponent("wbConsoleMenus", { from: loaded }).scope;
  const columns = loadComponent("wbColumns", { from: loaded }).scope;
  shell.$store.projects.setProjects([
    { slug: "path-8ee0b8b587ea7891", name: "widget", path: "C:/Dev/widget" },
    { key: "01KYPEER/path-1234", slug: "path-1234", name: "gadget", path: "/home/me/gadget", daemon: "01KYPEER", env: "WSL: Ubuntu" },
  ]);
  const ref = (p) => shell.$store.projects.repoRef(p);
  const [local, peer] = shell.$store.projects.projects;
  shell.$store.projects.setOpen(ref(local));
  assert.equal(menus.consoleMenuRepoName(), "widget");
  assert.equal(columns.columnRepoLabel(ref(peer)), "gadget");
});
