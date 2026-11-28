// Unit tests for assets/ui/wb-console.ts — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createSpawn } from "../assets/ui/wb-console-spawn.ts";
import { idLookups } from "./fake-document.mjs";
import { fakeClassList } from "./harness.mjs";
import { SRC, load } from "./wb-console-fixture.mjs";

test("sessionPresentation applies session-open environment and persists its owner metadata", () => {
  const got = load().sessionPresentation(
    "console",
    "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared",
    { daemonId: null, environment: null },
    {
      daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
      environment: "WSL: Ubuntu-22.04",
    },
  );
  assert.deepEqual(got, {
    daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAY",
    environment: "WSL: Ubuntu-22.04",
    name: null,
    checkout: null,
    // The TOOLTIP names the project, then the environment (ADR-0066 §5); the
    // routing head is a key and never shows.
    tooltip: "owner/shared\nWSL: Ubuntu-22.04",
  });
});

// ADR-0066 §5: the TOOLTIP holds, one per line: the project (never the
// routing head), the environment, and the vendor's session name when the
// launch had one.
test("sessionPresentation puts the project, the environment and the name in the tooltip", () => {
  // Through the REAL `WBFleet.refSlug`. A hand-written stub here
  // (`ref.split("/").slice(-2).join("/")`) passed while saying nothing about the
  // production fold, which strips a head only when it is a ULID — so a broken
  // `refSlug` would have sailed through this and every other test.
  const console_ = load();
  assert.deepEqual(
    console_.sessionPresentation(
      "claude",
      "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared",
      { daemonId: null, environment: null },
      {
        daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
        environment: "WSL: Ubuntu-22.04",
        name: "reviewer",
      },
    ),
    {
      daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
      environment: "WSL: Ubuntu-22.04",
      name: "reviewer",
      checkout: null,
      tooltip: "owner/shared\nWSL: Ubuntu-22.04\nreviewer",
    },
  );
});

// A remoteless repo is keyed `path-<hash>`: once the shell has shared the
// project names, the tooltip shows the folder and the console prefix is the
// folder name, never the key.
test("ingestProjects names a remoteless repo by its folder in the tooltip and the console prefix", () => {
  const c = load();
  const ref = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/path-8ee0b8b587ea7891";
  // Before the shell shares the names, the key is all the module has.
  assert.equal(c.consolePrefix(ref), "path-8ee0b8b587ea7891");
  c.ingestProjects([{ ref, name: "widget", title: "/home/me/widget" }]);
  assert.equal(c.consolePrefix(ref), "widget");
  const got = c.sessionPresentation("claude", ref, { daemonId: null, environment: null }, {
    daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAZ",
    environment: "WSL: Ubuntu-22.04",
  });
  assert.equal(got.tooltip, "/home/me/widget\nWSL: Ubuntu-22.04");
});

// NEGATIVE CONTROL: the name has no desk fallback — it dies with the child, so a
// restored window that has not re-opened its socket must show none, not a stale
// one. A `prior.name` must NOT resurrect it.
test("sessionPresentation never restores a session name from the desk", () => {
  const got = load().sessionPresentation(
    "claude",
    "owner/shared",
    { daemonId: "local", environment: "Windows", name: "reviewer" },
    null,
  );
  assert.equal(got.name, null);
  assert.equal(got.tooltip, "owner/shared\nWindows");
});

test("sessionPresentation keeps backward-compatible saved metadata before session-open", () => {
  assert.deepEqual(
    load().sessionPresentation(
      "claude",
      "owner/shared",
      { daemonId: "local", environment: "Windows" },
      null,
    ),
    {
      daemonId: "local",
      environment: "Windows",
      name: null,
      checkout: null,
      tooltip: "owner/shared\nWindows",
    },
  );
});

// ADR-0063 §3: the worktree the console lives in rides the presentation, and
// `renderTitle` draws it right after the label.
test("sessionPresentation carries the announced checkout", () => {
  assert.deepEqual(
    load().sessionPresentation(
      "claude",
      "owner/shared",
      { daemonId: "local", environment: "Windows" },
      { daemon_id: "local", environment: "Windows", checkout: "wt-a" },
    ),
    {
      daemonId: "local",
      environment: "Windows",
      name: null,
      checkout: "wt-a",
      tooltip: "owner/shared\nWindows",
    },
  );
});

// NEGATIVE CONTROL: the checkout has no desk fallback and never reads the
// picker's selection — only the daemon's announcement names the tree the child
// runs in, so a selection change cannot retitle a live console.
test("sessionPresentation never restores a checkout from the desk or the selection", () => {
  const got = load().sessionPresentation(
    "claude",
    "owner/shared",
    { daemonId: "local", environment: "Windows", checkout: "wt-a" },
    null,
  );
  assert.equal(got.checkout, null);
  assert.equal("title" in got, false, "the title is drawn from the window, not the presentation");
});

// The rows the switcher offers: `primary` first, then the listing's worktrees
// in order with their dirty flag; the console's own tree is the one marked
// current — `null` (a console on the primary) marks `primary`.
test("checkoutMenuRows puts primary first and marks the console's own tree", () => {
  const wb = load();
  const listing = {
    primary: "/p",
    worktrees: [
      { name: "wt-a", branch: "wt-a", dirty: true },
      { name: "wt-b", branch: "feat", dirty: false },
    ],
  };
  assert.deepEqual(
    wb.checkoutMenuRows(listing, "wt-b").map((r) => [r.name, r.branch, r.dirty, r.primary, r.current]),
    [
      ["primary", "", false, true, false],
      ["wt-a", "wt-a", true, false, false],
      ["wt-b", "feat", false, false, true],
    ],
  );
  assert.deepEqual(
    wb.checkoutMenuRows(listing, null).map((r) => [r.name, r.current]),
    [
      ["primary", true],
      ["wt-a", false],
      ["wt-b", false],
    ],
  );
  // A listing that never answered still offers the primary row — the menu
  // is only ever opened from a switchable title, so this is the guard, not
  // the expected path.
  assert.deepEqual(
    wb.checkoutMenuRows(null, null).map((r) => r.name),
    ["primary"],
  );
});

// With the shell's session rows the menu folds the agent's state per tree
// (ADR-0059 §5): `primary` is the sessions with no checkout, a worktree row
// its own; `waiting` outranks `working`; a tree with no session says nothing.
test("checkoutMenuRows carries the agent state per tree when given sessions", () => {
  // The fold is `WBProject.worktreeStates`, the real module the console imports.
  const wb = load();
  const listing = { primary: "/p", worktrees: [{ name: "wt-a" }, { name: "wt-b" }] };
  const sessions = [
    { id: 1, repo: "o/r", agent_state: { state: "working" } },
    { id: 2, repo: "o/r", checkout: "wt-a", agent_state: { state: "working" } },
    { id: 3, repo: "o/r", checkout: "wt-a", agent_state: { state: "waiting" } },
  ];
  assert.deepEqual(
    wb.checkoutMenuRows(listing, "wt-b", sessions).map((r) => [r.name, r.state, r.current]),
    [
      ["primary", "working", false],
      ["wt-a", "waiting", false],
      ["wt-b", null, true],
    ],
  );
  // No sessions given → no state field is populated.
  assert.deepEqual(
    wb.checkoutMenuRows(listing, null).map((r) => r.state),
    [null, null, null],
  );
});

// A desk with `n` fences, landed through the real GET.
async function plane(n) {
  const fences = Array.from({ length: n }, (_, i) => ({
    id: `f${i}`,
    name: `Fence ${i + 1}`,
    rect: { left: i * 10, top: 0, width: 5, height: 5 },
    locked: false,
    ts: i,
  }));
  const realFetch = globalThis.fetch;
  globalThis.fetch = () => Promise.resolve({ ok: true, json: () => Promise.resolve({ windows: [], fences }) }); // own: the fake changes inside the test
  try {
    const wb = load({}, {}, { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } });
    await wb.whenDeskLoaded();
    assert.equal(wb.fenceRecords().length, n, "the desk landed");
    return wb;
  } finally {
    globalThis.fetch = realFetch; // own: the fake changes inside the test
  }
}

test("the fence cap is a number the shell can state, and the plane is at it from the 12th fence", async () => {
  assert.equal(load().FENCE_MAX, 12);
  assert.equal((await plane(0)).atFenceCap(), false, "an empty plane");
  assert.equal((await plane(11)).atFenceCap(), false, "one below the cap");
  const full = await plane(12);
  assert.equal(full.atFenceCap(), true, "at the cap");
  assert.equal(full.createFence(), false, "a fence past the cap is refused");
});

test("the font range holds xterm's default, so an unset preference changes nothing", () => {
  const c = load();
  assert.ok(c.FONT_MIN < c.FONT_DEFAULT && c.FONT_DEFAULT < c.FONT_MAX);
  // With no view store (the popup, and this harness) the size is the default.
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

const coarse = (q) => ({ matches: q.includes("coarse"), addEventListener() {} });

test("a touch surface with no stored preference starts at the same size", () => {
  const c = load({ matchMedia: coarse });
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

// --- restoreRect: a window nobody can measure is read from its inline rect ----
// `restoreDesk` runs on load whatever tab is showing, and the Consoles tab is
// `display:none` under any other — so a restored window measured 0×0 at 0,0
// there, and the `persistWin` at the end of its spawn stored the zeros over the
// record's real box. The next load rendered the zeros as the CSS floor (240×150
// at the origin) and stored THAT: the console had "moved to the corner"
// (2026-09-09). The inline rect is what `buildChrome` just wrote from the record,
// and it is the honest box while nothing can be measured.
function fakeWin({ maximized = false, classes = [], offsets, inline }) {
  return {
    classList: fakeClassList(...(maximized ? ["maximized"] : []), ...classes),
    offsetLeft: offsets.left,
    offsetTop: offsets.top,
    offsetWidth: offsets.width,
    offsetHeight: offsets.height,
    style: inline || {},
  };
}
const REAL = { left: 59, top: 2569, width: 1122, height: 634 };
const INLINE = { left: "59px", top: "2569px", width: "1122px", height: "634px" };
const HIDDEN = { left: 0, top: 0, width: 0, height: 0 };

// A column right of the leftmost is not `.maximized` (ADR-0051 §5), yet its
// painted box is view state: reading it would write the column onto the desk.
const RESTORE_RECT_ROWS = [
  {
    // The inline rect is stale on purpose: after a drag the DOM is the truth.
    name: "a measurable window is read from the DOM",
    win: { offsets: REAL, inline: { left: "1px", top: "1px", width: "1px", height: "1px" } },
    expected: REAL,
  },
  {
    name: "a hidden window keeps its box instead of storing zeros",
    win: { offsets: HIDDEN, inline: INLINE },
    expected: REAL,
  },
  {
    name: "a hidden window honours a real 0 in the inline rect",
    win: { offsets: HIDDEN, inline: { left: "0px", top: "0px", width: "560px", height: "340px" } },
    expected: { left: 0, top: 0, width: 560, height: 340 },
  },
  {
    name: "a maximized window still reads the pre-maximize inline rect",
    win: { maximized: true, offsets: { left: 0, top: 0, width: 1440, height: 900 }, inline: INLINE },
    expected: REAL,
  },
  {
    name: "a column reads the inline rect, not the painted column box",
    win: { classes: ["column"], offsets: { left: 960, top: 0, width: 960, height: 1000 }, inline: INLINE },
    expected: REAL,
  },
];

test("restoreRect gives the box of the record in every state of the window", () => {
  const { restoreRect } = load();
  for (const row of RESTORE_RECT_ROWS) {
    assert.deepEqual(restoreRect(fakeWin(row.win)), row.expected, row.name);
  }
});

test("markRelaunch marks the id, and a take-down that throws clears the mark", () => {
  const { markRelaunch, isRelaunching } = load();
  assert.equal(isRelaunching("w-a"), false);
  let seen = null;
  markRelaunch("w-a", () => (seen = isRelaunching("w-a")));
  assert.equal(seen, true, "marked before the take-down");
  assert.equal(isRelaunching("w-a"), true, "still marked until the respawn");
  assert.equal(isRelaunching("w-b"), false);
  assert.throws(() => markRelaunch("w-b", () => { throw new Error("gone"); }), /gone/u);
  assert.equal(isRelaunching("w-b"), false, "a take-down that throws clears the mark");
});

// A relaunch takes the window off the stage and spawns a new one under the
// same id; the shell's columns wait for an id marked in between. These paths
// need a real DOM, so the wiring is pinned on the source.
// pin: a restart and a placeholder's Launch mark the console as relaunching —
// both paths build a real window in the DOM, which the node harness has not.
test("a restart and a placeholder's Launch mark the console as relaunching", () => {
  assert.match(SRC, /markRelaunch\(carry\.id, discard\);/u);
  assert.match(SRC, /markRelaunch\(carry\.id, \(\) => \{\s*drop\(true\);\s*\}\);/u);
  assert.match(SRC, /\} finally \{\s*relaunching\.delete\(carry\.id\);/u);
});

// The layout after a take-down wakes the siblings the old window covered, and
// a sibling may throw there: the restart still builds its new window (#904).
const noop = () => {};
test("a restart survives a layout throw", (t) => {
  const { markRelaunch } = load();
  const button = () => ({ addEventListener: noop });
  const win = {
    _deskId: "w-a",
    _deskKind: "console",
    classList: fakeClassList(),
    querySelector: () => null,
    remove() {
      this.removed = true;
    },
  };
  let layoutThrows = false;
  const spawned = [];
  const wins = new Set();
  const spawn = createSpawn({
    document: {},
    wins,
    peerGroups: new Map(),
    wakePeerNow: () => null,
    readFleetNow: () => null,
    buildChrome: () => ({ win, body: {}, title: {}, restartBtn: button(), closeBtn: button() }),
    sessionPresentation: () => ({}),
    renderTitle: noop,
    closeCheckoutMenu: noop,
    restartWin: () => Promise.resolve(),
    ensureListing: noop,
    buildKeyBar: () => ({ ctrlBtn: null, shiftBtn: null, selBtn: null }),
    applyDormancy: noop,
    attachTerminal: () => ({ dispose: noop }),
    budget: { dormancyWatch: () => null, trackDormancy: noop, untrackDormancy: noop },
    askConfirm: () => Promise.resolve(false),
    toast: noop,
    createRecord: noop,
    atDeskCap: () => false,
    setWin: noop,
    forgetRecord: noop,
    currentDesk: () => [],
    reloadDesk: () => Promise.resolve(),
    recordLeft: noop,
    restoreRect: () => ({ left: 0, top: 0, width: 10, height: 10 }),
    applyExtent: () => {
      if (layoutThrows) throw new Error("a sibling's wake failed");
    },
    changed: noop,
    spawnPlaceholder: () => win,
    spawnOrMissing: (req, label, repo, carry) => {
      spawned.push(carry.id);
      return Promise.resolve(win);
    },
    markRelaunch,
  });
  const realWB = globalThis.WB;
  const logged = t.mock.method(console, "error", noop);
  globalThis.WB = { emit: noop };
  try {
    spawn.spawnWindow({ console: true, repo: "o/r" }, "console", "o/r");
    layoutThrows = true;
    assert.doesNotThrow(() => win._relaunchIn());
  } finally {
    globalThis.WB = realWB;
  }
  assert.equal(win.removed, true, "the old window left the page");
  assert.equal(wins.has(win), false, "the old window is no longer tracked");
  assert.deepEqual(spawned, ["w-a"], "the new window is spawned under the same id");
  assert.ok(
    logged.mock.calls.some((c) => c.arguments.some((a) => a instanceof Error && /sibling/u.test(a.message))),
    "the layout's error reaches the browser console",
  );
});

// A page with no tabs store (the detached fence page) shows each console;
// a console is not shown while a file tab is the active one. `placeTabs`
// reaches the dormancy of a window that came onto the stage.
function stagePage(extras) {
  const kids = [];
  const stage = {
    clientWidth: 1280,
    querySelectorAll: (sel) => (sel === ".session-window" ? kids : []),
    append: (w) => {
      w.parentElement = stage;
      kids.push(w);
    },
  };
  const pane = { clientWidth: 1280, querySelectorAll: () => [], append() {} };
  const elements = { stage, "console-tabs-pane": pane };
  const wb = load(extras, {
    ...idLookups((id) => elements[id] ?? null),
    createElement: () => ({ style: {}, addEventListener() {}, append() {}, remove() {} }),
  });
  const disposed = [];
  const win = {
    _deskId: "w1",
    _visible: true,
    _term: { sessionId: 7, dispose: (why) => disposed.push(why) },
    _dormantTimer: null,
    parentElement: null,
    dataset: {},
    style: { setProperty() {}, removeProperty() {} },
    classList: fakeClassList("session-window"),
    querySelector: () => null,
  };
  kids.push(win);
  return { wb, win, disposed };
}

test("a console on a page with no Alpine is shown and does not throw", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const { wb, win, disposed } = stagePage();
  wb.placeTabs([], null);
  t.mock.timers.tick(wb.dormantAfterMs() * 2);
  assert.deepEqual(
    {
      timer: win._dormantTimer,
      dormant: win._dormant,
      marked: win.classList.contains("dormant"),
      disposed,
    },
    { timer: null, dormant: undefined, marked: false, disposed: [] },
    "a shown console on screen stays awake, with its terminal kept",
  );
});

test("a console is not shown while another tab is active", (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const tabs = { active: "file:a.txt" };
  const { wb, win, disposed } = stagePage({ Alpine: { store: () => tabs } });
  wb.placeTabs([], null);
  assert.deepEqual(disposed, [], "it is not put to sleep before the grace period ends");
  assert.equal(win.classList.contains("dormant"), false);
  t.mock.timers.tick(wb.dormantAfterMs());
  assert.deepEqual(
    {
      timer: win._dormantTimer,
      dormant: win._dormant,
      marked: win.classList.contains("dormant"),
      disposed,
    },
    { timer: null, dormant: true, marked: true, disposed: ["dormant"] },
    "a console behind a file tab sleeps after the grace period, its timer cleared",
  );
});
