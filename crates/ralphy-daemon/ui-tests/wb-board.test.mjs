// Unit tests for assets/ui/wb-board.ts, the Alpine component of the board:
// the issues of the open project and their reads, the filters, the issue
// view, the label menu and the ready plan. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// `init()` starts the backstop on the global timer; here it never ticks.
// Returns the intervals it asked for.
function initQuietly(scope) {
  const asked = [];
  const realSetInterval = globalThis.setInterval;
  globalThis.setInterval = (fn, ms) => (asked.push({ fn, ms }), 0);
  try {
    scope.init();
  } finally {
    globalThis.setInterval = realSetInterval;
  }
  return asked;
}

// One page with the shell and the board, and a window that delivers the
// events it is sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds the component.
function page(opts = {}) {
  const listeners = {};
  const loaded = loadShell({
    ...opts,
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const board = loadComponent("wbBoard", { from: loaded, magics: { $nextTick: (fn) => fn() } });
  const timers = initQuietly(board.scope);
  const fire = (type, detail) => loaded.window.dispatchEvent({ type, detail });
  return { loaded, shell: loaded.state, board: board.scope, window: loaded.window, fire, timers };
}

// A board whose `WBDaemon.observe` answers each call with the next reply.
function observedBoard(replies) {
  const loaded = loadShell();
  loaded.window.WBDaemon.observe = async () => replies.shift() ?? null;
  loaded.state.$store.projects.setOpen("o/r");
  loaded.state.$store.modals.flash = () => {};
  const { scope } = loadComponent("wbBoard", { from: loaded });
  scope.loadPlan = () => {};
  return scope;
}

// ---- the component and its markup ---------------------------------------------

test("wbBoard lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbBoard");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbBoard reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbBoard");
  const markup = componentMarkup(HTML, "wbBoard");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variables of the board, the issue view and the label menu.
  const loopVars = new Set(["col", "s", "iss", "l", "b", "c", "idx"]);
  // A template literal's text is not code: only its `${…}` parts are read.
  const code = (value) => value.replace(/`([^`]*)`/g, (_, t) => Array.from(t.matchAll(/\$\{([^}]*)\}/g), (m) => m[1]).join(", "));
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component.
    if (attr === "x-data") continue;
    for (const n of bindingNames(code(value))) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 60, `the markup was read: ${read} names`);
});

test("nothing outside the board element names its state", () => {
  const start = HTML.indexOf('x-data="wbBoard"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["kanbanSel", "openIssue", "selectedIssue", "kanbanColumns", "boardError", "planModal", "openPlanModal", "discardPlan", "toggleLabel", "labelMenuOpen", "labelsLocked", "issueRunning"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the board", () => {
  const { board, fire } = page();
  const calls = [];
  board.maybeRefreshBoard = (why) => calls.push(`refresh:${why}`);
  board.loadBoard = () => calls.push("loadBoard");
  board.openIssue = (n) => calls.push(`open:${n}`);
  fire("workbench:board-refresh", { trigger: "visible" });
  assert.deepEqual(calls, ["refresh:visible"]);
  // The board opened: the selection goes, and the tracker is read.
  board.kanbanSel = 4;
  fire("workbench:board-opened", null);
  assert.equal(board.kanbanSel, null);
  assert.deepEqual(calls, ["refresh:visible", "loadBoard"]);
  fire("workbench:issue-open", { issue: 12 });
  assert.deepEqual(calls, ["refresh:visible", "loadBoard", "open:12"]);
});

test("the Spend view takes the titles the board already holds, and loads nothing", () => {
  const { loaded, shell, board } = page();
  const { scope: spend } = loadComponent("wbSpend", { from: loaded });
  shell.$store.projects.setOpen("o/r");
  board.loadBoard = () => assert.fail("the Spend view must not load the board");
  // The board answers during the send, with the open project's issues only.
  board.boardIssues = { "o/r": [{ number: 251, title: "the retry" }], "o/x": [{ number: 9, title: "other" }] };
  loaded.window.WBSpend = { state: (view) => view };
  assert.deepEqual(spend.spendView().issues, [{ number: 251, title: "the retry" }]);
  // CONTROL: a board that holds nothing for the project gives no titles.
  board.boardIssues = {};
  assert.deepEqual(spend.spendView().issues, []);
});

test("opening the board reads it and drops the selection; closing it reads nothing", () => {
  const { shell, board } = page();
  let loads = 0;
  board.loadBoard = () => loads++;
  board.kanbanSel = 5;
  shell.toggleKanban();
  assert.equal(shell.kanbanOpen, true);
  assert.deepEqual([board.kanbanSel, loads], [null, 1]);
  board.kanbanSel = 6;
  shell.toggleKanban();
  assert.equal(shell.kanbanOpen, false);
  assert.deepEqual([board.kanbanSel, loads], [6, 1]);
});

test("a trail node of the Runs panel opens the board on that issue", () => {
  const { loaded, shell, board } = page();
  const runs = loadComponent("wbRuns", { from: loaded, magics: { $nextTick: (fn) => fn() } }).scope;
  runs.init();
  board.loadBoard = () => {};
  const opened = [];
  board.loadIssueDetail = (n) => opened.push(n);
  runs.focusIssue(17);
  assert.equal(shell.kanbanOpen, true);
  assert.equal(board.kanbanSel, 17);
  assert.deepEqual(opened, [17]);
});

test("the board part drops the selection of the project that was open", () => {
  const { shell, board, fire } = page();
  let loads = 0;
  board.loadBoard = () => loads++;
  shell.$store.projects.setOpen("owner/b");
  board.kanbanSel = 7;
  fire("workbench:project-changed", { slug: "owner/b", previous: "owner/a" });
  assert.equal(board.kanbanSel, null);
  // The board is read only while it is open.
  assert.equal(loads, 0);
  shell.kanbanOpen = true;
  fire("workbench:project-changed", { slug: "owner/b", previous: "owner/a" });
  assert.equal(loads, 1);
});

test("init anchors the refresh clock and starts the 30 s backstop", () => {
  const before = Date.now();
  const { board, timers } = page();
  assert.ok(board._boardLoadedAt >= before, "the clock is anchored at load");
  assert.deepEqual(timers.map((t) => t.ms), [30000]);
  const ticks = [];
  board.maybeRefreshBoard = (why) => ticks.push(why);
  timers[0].fn();
  assert.deepEqual(ticks, ["backstop"]);
});

// ---- the folds -----------------------------------------------------------------

test("githubUrl resolves the OPEN project's remote, and refuses when it cannot", () => {
  // The pure URL half moved to `wb-project.ts` and is tested there. This is the
  // half that STAYED — finding the open project among `projects` by composite
  // ref — and it lost its coverage in the move: a lookup regression would hand
  // back a link to another repo's issue with the whole suite green.
  const { scope: own } = loadComponent("wbBoard");
  own.$store.projects.setProjects([
    { slug: "owner/a", remoteUrl: "https://github.com/owner/a.git" },
    { slug: "owner/b", remoteUrl: "https://github.com/owner/b.git" },
  ]);

  own.$store.projects.setOpen(own.$store.projects.repoRef(own.$store.projects.projects[1]));
  assert.equal(own.githubUrl(42), "https://github.com/owner/b/issues/42");
  own.$store.projects.setOpen(own.$store.projects.repoRef(own.$store.projects.projects[0]));
  assert.equal(own.githubUrl(42), "https://github.com/owner/a/issues/42");

  // NEGATIVE CONTROL: no project matches, so there is nothing honest to link to
  // — and emphatically not the first project in the list.
  own.$store.projects.setOpen("owner/never-registered");
  assert.equal(own.githubUrl(42), null);
  own.$store.projects.setOpen(null);
  assert.equal(own.githubUrl(42), null);
});

test("issueBlockers resolves each blocker it can see and admits the ones it cannot", () => {
  const { scope: own } = loadComponent("wbBoard");
  own.$store.projects.setOpen("owner/repo");
  // `projectIssues()` reads the BOARD's fold, not the run's issue list.
  own.boardIssues = {
    "owner/repo": [
      { number: 10, title: "the open one", state: "open" },
      { number: 11, title: "the closed one", state: "closed" },
    ],
  };
  assert.deepEqual(own.issueBlockers({ blockedBy: [10, 11, 99] }), [
    { number: 10, open: true, known: true, title: "the open one" },
    { number: 11, open: false, known: true, title: "the closed one" },
    // An issue the board has not loaded: `known: false` is the honest state,
    // and it is NOT the same as a closed blocker even though both read
    // `open: false`. A consumer that conflated them would treat an unknown
    // blocker as satisfied.
    { number: 99, open: false, known: false, title: "" },
  ]);
  assert.deepEqual(own.issueBlockers({ blockedBy: [] }), []);
  assert.deepEqual(own.issueBlockers({}), []);
  assert.deepEqual(own.issueBlockers(null), []);
});

test("boardRowToIssue accepts both the snake and camel spellings of a blocker list", () => {
  // The board fold and the run snapshot disagree on case, and this is the one
  // place that reconciles them — so both must survive a move.
  const { scope: own } = loadComponent("wbBoard");
  assert.deepEqual(own.boardRowToIssue({ number: 7, blocked_by: [1] }).blockedBy, [1]);
  assert.deepEqual(own.boardRowToIssue({ number: 7, blockedBy: [2] }).blockedBy, [2]);
  const bare = own.boardRowToIssue({ number: 7 });
  assert.deepEqual(bare, {
    number: 7,
    title: "",
    state: "open",
    reason: null,
    labels: [],
    assignees: [],
    blockedBy: [],
    created: "",
    updated: "",
    body: "",
    comments: [],
  });
  // `reason` distinguishes absent from null-on-purpose, and `??` is load-bearing
  // here: a `state_reason` of `null` from the API must not fall through to a
  // later default.
  assert.equal(own.boardRowToIssue({ number: 7, state_reason: "completed" }).reason, "completed");
});

test("kanbanColumnTitle resolves a column id to its human title", () => {
  // Against the real column table, not against "is a non-empty string" — the
  // previous form asserted `title.length >= id.length || title !== ""`, whose
  // right side was already asserted two lines above, so a fold returning one
  // constant for every column passed it.
  const { scope: s, window } = loadComponent("wbBoard");
  const columns = window.WBKanban.COLUMNS;
  assert.ok(columns.length >= 4, "the board has four columns (#301)");
  const open = { state: "open", labels: [] };
  const id = s.kanbanColumnOf(open);
  const expected = columns.find((c) => c.id === id);
  assert.ok(expected, `kanbanColumnOf returned ${id}, which is not a column`);
  assert.equal(s.kanbanColumnTitle(open), expected.title);
  // Two different columns must not share a title, or the readout cannot
  // distinguish them.
  const titles = new Set(columns.map((c) => c.title));
  assert.equal(titles.size, columns.length, "every column needs its own title");
});

test("a run state shows as words on the board", () => {
  const { scope: state } = loadComponent("wbBoard");
  assert.equal(state.runStateWord("sleep"), "usage limit — sleeping");
  assert.equal(state.issueRunningLabel({ state: "hitl", agent: "claude" }), "Running · waiting on human (claude)");
});

// --- ADR-0070 D3: a failed read keeps the last good value, marked not current

test("a failed board read after a good one keeps the cards, marked not current", async () => {
  const state = observedBoard([
    { status: "ok", board: { issues: [{ number: 7, title: "x", labels: [] }], labels: [] } },
    { status: "error", message: "gh not authed" },
  ]);
  state.kanbanSel = null;
  await state.loadBoard();
  assert.equal(state.boardIssues["o/r"].length, 1);
  await state.loadBoard();
  assert.equal(state.boardIssues["o/r"].length, 1, "the cards stay");
  assert.equal(state.boardRead["o/r"].current, false);
  assert.match(state.boardError["o/r"], /Not current/);
});

// A label name is repo data: one named like an Object member (`constructor`)
// still falls back to the seed color, not to the member.
test("a label missing from the live list takes the seed color, whatever its name", async () => {
  const state = observedBoard([
    { status: "ok", board: { issues: [], labels: [{ name: "bug", color: "d73a4a" }] } },
  ]);
  state.kanbanSel = null;
  await state.loadBoard();
  assert.equal(state.labelColor("bug"), "#d73a4a");
  for (const name of ["constructor", "toString", "__proto__"]) {
    assert.equal(state.labelColor(name), globalThis.window.WBKanban.labelColor(name), name);
  }
});

test("moving a board card is locked while the board is not current", async () => {
  const state = observedBoard([
    { status: "ok", board: { issues: [], labels: [] } },
    { status: "error", message: "gh not authed" },
  ]);
  state.kanbanSel = null;
  state.$store.runs.runsByProject["o/r"] = [];
  await state.loadBoard();
  assert.equal(state.labelsLocked(), false);
  await state.loadBoard();
  assert.equal(state.labelsLocked(), true);
  assert.match(state.labelLockReason(), /board shown is not current/);
});
