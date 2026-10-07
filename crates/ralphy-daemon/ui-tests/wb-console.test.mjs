// Unit tests for assets/ui/wb-console.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { WBProject } from "../assets/ui/wb-project.ts";
import { WBFleet } from "../assets/ui/wb-fleet.ts";
import { WBColumns } from "../assets/ui/wb-columns.ts";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-console.js"), "utf8");
// The module reads `window.WBDeskSink.daemon()` at load, exactly as it does in
// the browser — so the harness runs the REAL sink source first, mirroring
// index.html's script order. A stub here would hide a broken script tag.
const SINK_SRC = readFileSync(join(UI, "wb-desk-sink.js"), "utf8");
// The desk's changes and view (`wb-desk-sync.js`): `wb-console.js` builds its
// state from `window.WBDeskSync` at load, so the real source runs first here.
const SYNC_SRC = readFileSync(join(UI, "wb-desk-sync.js"), "utf8");
// Same reasoning for the detach link: `wb-console.js` reads
// `window.WBDetachLink.link()` at load, so the harness runs the REAL source in
// the same script order both documents use. It touches neither `sessionStorage`
// nor `BroadcastChannel` at load, so neither needs to exist here.
const LINK_SRC = readFileSync(join(UI, "wb-detach-link.js"), "utf8");
// The plane geometry (ADR-0057). `wb-console.js` DESTRUCTURES this namespace at
// module scope, so a harness without it throws on the first line of the IIFE —
// which is the intended failure, and the reason it is the real source here too.
const GEOM_SRC = readFileSync(join(UI, "wb-geometry.js"), "utf8");
// `wb-fleet.ts` is on the window of both documents that carry the console
// (index.html, detached-fence.html) before the console boots, so the harness
// gives the REAL module rather than leaving the namespace absent. Leaving it
// out made `sessionPresentation` take its `window.WBFleet ? … : repo`
// fallback, and the test then pinned a title the product explicitly forbids —
// the peer ref printed whole, which is the defect wb-fleet was written to fix.
// The window field inventory and its accessors (`initWindow`, `sessionIdOf`,
// `watchingOf`, `checkoutOf`). DESTRUCTURED at module scope exactly like the
// geometry above, so the real source runs here for the same reason: a harness
// without it throws inside the IIFE, which is the intended failure.
const WINSTATE_SRC = readFileSync(join(UI, "wb-window-state.js"), "utf8");
// The console name (ADR-0066). Loaded before `wb-console.js` by both
// documents; the tooltip and every new console's name come from it.
const NAME_SRC = readFileSync(join(UI, "wb-console-name.js"), "utf8");

// `extras` is merged into the stub `window` BEFORE the module is evaluated, so a
// test can supply a sibling module (`WBFleet`) that index.html loads first. The
// default is no siblings: that is the honest shape for the boot order where a
// sibling has not loaded, and several tests pin the fallback it produces.
function load(extras = {}, docExtras = {}) {
  // The three globals the module touches at LOAD time: `window.addEventListener`
  // (the pagehide flush), `document.readyState`/`addEventListener` (the boot
  // hooks — "loading" parks them on a no-op listener instead of running them
  // against a DOM that does not exist) and `location.protocol`/`host`
  // (WS_ORIGIN). No `ResizeObserver` is injected ON PURPOSE: the surviving one
  // lives inside `attachTerminal`, which this harness never reaches, so a
  // module-scope observer re-added alongside a clamp fails LOUDLY here.
  const window = { addEventListener() {}, ...extras };
  const document = { readyState: "loading", addEventListener() {}, ...docExtras };
  const location = { protocol: "http:", host: "127.0.0.1:7431" };
  window.WBFleet = WBFleet;
  new Function("window", GEOM_SRC)(window);
  new Function("window", WINSTATE_SRC)(window);
  new Function("window", NAME_SRC)(window);
  new Function("window", SINK_SRC)(window);
  new Function("window", SYNC_SRC)(window);
  new Function("window", LINK_SRC)(window);
  // Node 22 ships a REAL `BroadcastChannel`, and `wb-console.js` subscribes at
  // module load — an open channel per `load()` holds the event loop open and
  // `node --test` never exits. Hidden for the load only; the channel's own
  // behaviour is covered against a fake in wb-detach-link.test.mjs.
  const realBC = globalThis.BroadcastChannel;
  delete globalThis.BroadcastChannel;
  try {
    new Function("window", "document", "location", SRC)(window, document, location);
  } finally {
    globalThis.BroadcastChannel = realBC;
  }
  return window.WBConsole;
}

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

test("reconcileDesk keeps same-slug sessions distinct by composite repo ref", () => {
  const peerA = "01ARZ3NDEKTSV4RRFFQ69G5FAY/owner/shared";
  const peerB = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/shared";
  const record = (id, repo) => ({
    id,
    repo,
    agent: "console",
    kind: "console",
    sessionId: 1,
  });
  const session = (repo, environment) => ({
    id: 1,
    repo,
    agent: "console",
    kind: "console",
    environment,
  });
  const out = load().reconcileDesk({
    layout: [record("a", peerA), record("b", peerB)],
    sessions: [session(peerB, "WSL: B"), session(peerA, "WSL: A")],
  });
  assert.deepEqual(
    out.map(({ record: saved, session: live, action }) => [
      saved.id,
      live.repo,
      live.environment,
      action,
    ]),
    [
      ["a", peerA, "WSL: A", "attach"],
      ["b", peerB, "WSL: B", "attach"],
    ],
  );
});

// #411: the launch request a desk record relaunches with. An agent record asks
// for its vendor AND the worktree it was recorded in; a record without one
// (a pre-#411 desk, or a console on the primary) asks for `null`, the primary;
// a shell record is the shell request and never carries a checkout.
test("relaunchRequest carries the record's checkout on an agent record only", () => {
  const wb = load();
  const agent = { id: "a", repo: "owner/repo", agent: "claude", kind: "agent" };
  assert.deepEqual(wb.relaunchRequest({ ...agent, checkout: "wt-a" }), {
    repo: "owner/repo",
    agent: "claude",
    checkout: "wt-a",
  });
  assert.deepEqual(wb.relaunchRequest(agent), {
    repo: "owner/repo",
    agent: "claude",
    checkout: null,
  });
  assert.deepEqual(
    wb.relaunchRequest({ id: "s", repo: "owner/repo", agent: "console", kind: "console", checkout: "wt-a" }),
    { console: true, repo: "owner/repo", command: undefined },
  );
  // "~" is the daemon's label for a repo-less console, never a slug to send back.
  assert.deepEqual(wb.relaunchRequest({ id: "h", repo: "~", agent: "console", kind: "console" }), {
    console: true,
    repo: undefined,
    command: undefined,
  });
  // A console record whose label is not the literal `console` is a
  // startup-command console: the label IS the command it relaunches with.
  assert.deepEqual(wb.relaunchRequest({ id: "m", repo: "owner/repo", agent: "htop", kind: "console" }), {
    console: true,
    repo: "owner/repo",
    command: "htop",
  });
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
  // The fold is `WBProject.worktreeStates`, the real module.
  const wb = load({ WBProject });
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

// ADR-0059 §5: a window's row on `/api/sessions` is the daemon's id AND the
// repo ref — a restarted daemon reuses ids and a peer's `1` is not ours.
test("sessionRowFor matches a window's session by id and repo ref", () => {
  const wb = load();
  const win = (sessionId, repo) => ({ _term: { sessionId }, _deskRepo: repo });
  const sessions = [
    { id: 1, repo: "owner/a", agent_state: { state: "working", since: "t" } },
    { id: 1, repo: "01PEER/owner/a", agent_state: { state: "waiting", since: "t" } },
    { id: 2, repo: "~" },
  ];
  assert.equal(wb.sessionRowFor(win(1, "owner/a"), sessions).agent_state.state, "working");
  assert.equal(wb.sessionRowFor(win(1, "01PEER/owner/a"), sessions).agent_state.state, "waiting");
  assert.equal(wb.sessionRowFor(win(2, "~"), sessions).id, 2);
  assert.equal(wb.sessionRowFor(win(3, "owner/a"), sessions), null);
  // A window still waiting for its socket has `_wantsSession` and no term.
  assert.equal(
    wb.sessionRowFor({ _wantsSession: 1, _deskRepo: "owner/a" }, sessions).agent_state.state,
    "working",
  );
  assert.equal(wb.sessionRowFor({ _deskRepo: "owner/a" }, sessions), null);
});

// ---- bringIntoView (issue #337) ---------------------------------------------
// The scroll offsets that CENTRE a target rect in the viewport, clamped to the
// extent. Always centres — it is not a scroll-into-view-if-needed (ADR-0051 §7's
// fence jump wants the same slide for a partially visible target).
const VIEW = { width: 1000, height: 700 };
const EXT = { width: 2000, height: 1500 };

const BRING = [
  {
    name: "a target already in view is still CENTRED",
    target: { left: 600, top: 400, width: 200, height: 100 },
    want: { left: 200, top: 100 },
  },
  {
    name: "off the right edge",
    target: { left: 1200, top: 400, width: 200, height: 100 },
    want: { left: 800, top: 100 },
  },
  {
    name: "off the left edge clamps at the pinned origin",
    target: { left: 0, top: 400, width: 200, height: 100 },
    want: { left: 0, top: 100 },
  },
  {
    name: "off the bottom edge",
    target: { left: 600, top: 900, width: 200, height: 100 },
    want: { left: 200, top: 600 },
  },
  {
    name: "off the top edge clamps at the pinned origin",
    target: { left: 600, top: 0, width: 200, height: 100 },
    want: { left: 200, top: 0 },
  },
  {
    name: "a target LARGER than the viewport centres on its own middle",
    target: { left: 200, top: 100, width: 1600, height: 1200 },
    want: { left: 500, top: 350 },
  },
  {
    name: "a target at the far corner never scrolls past `extent - viewport`",
    target: { left: 1800, top: 1350, width: 200, height: 150 },
    want: { left: 1000, top: 800 },
  },
  {
    // The ceiling itself, un-clamped: distinguishes "clamped at the boundary"
    // from "the arithmetic happened to land there".
    name: "a target whose centring lands EXACTLY on the ceiling is not clamped away",
    target: { left: 1400, top: 1000, width: 200, height: 100 },
    want: { left: 1000, top: 700 },
  },
  {
    // NEGATIVE CONTROL: drop the final `Math.max(0, …)` and this answers
    // {-200,-100}, which the DOM would silently swallow as 0,0 — the bug would
    // be invisible in the browser and only ever show up here. It discriminates
    // only because that floor is the axis's ONLY floor: flooring the ceiling
    // too would make both spellings answer 0 and this row unfalsifiable.
    name: "a viewport LARGER than the extent yields the origin, never a negative offset",
    target: { left: 600, top: 400, width: 200, height: 100 },
    extent: { width: 800, height: 600 },
    want: { left: 0, top: 0 },
  },
];

for (const row of BRING) {
  test(`bringIntoView: ${row.name}`, () => {
    const got = load().bringIntoView(row.target, VIEW, row.extent || EXT);
    assert.deepEqual(got, row.want);
  });
}

// ---- anchorIntoView ----------------------------------------------------------
// The fence jump's own anchoring: the target's TOP-LEFT corner one inset in from
// the viewport's, through the same clamp. A fence is a region worked inside, not
// a point of interest looked at, so centring it wastes the screen above and left
// of it. `bringIntoView` above is untouched and still serves the Go-to picker.
const ANCHOR = [
  {
    name: "the target's corner lands one inset in from the viewport's",
    target: { left: 600, top: 400, width: 200, height: 100 },
    want: { left: 576, top: 376 },
  },
  {
    // NEGATIVE CONTROL against the centring fold: `bringIntoView` answers
    // {200,100} for this same target, so a call routed to the wrong one is red.
    name: "the SIZE of the target changes nothing — only its corner is read",
    target: { left: 600, top: 400, width: 1600, height: 1200 },
    want: { left: 576, top: 376 },
  },
  {
    name: "a target near the origin clamps at the pinned origin rather than going negative",
    target: { left: 10, top: 4, width: 200, height: 100 },
    want: { left: 0, top: 0 },
  },
  {
    name: "a target at the far corner never scrolls past `extent - viewport`",
    target: { left: 1900, top: 1400, width: 200, height: 150 },
    want: { left: 1000, top: 800 },
  },
  {
    // The whole point of the plane's new headroom: with one viewport of room
    // past the content, the corner offset is INSIDE the ceiling and survives the
    // clamp. `stageExtent`'s table is what guarantees this extent is real.
    name: "with a viewport of headroom past it, a far fence really reaches the corner",
    target: { left: 2200, top: 1500, width: 720, height: 460 },
    extent: { width: 3920, height: 2660 },
    want: { left: 2176, top: 1476 },
  },
  {
    name: "the inset is overridable, and zero means flush against the corner",
    target: { left: 600, top: 400, width: 200, height: 100 },
    inset: 0,
    want: { left: 600, top: 400 },
  },
];

for (const row of ANCHOR) {
  test(`anchorIntoView: ${row.name}`, () => {
    const got = load().anchorIntoView(row.target, VIEW, row.extent || EXT, row.inset);
    assert.deepEqual(got, row.want);
  });
}

// ---- viewLanding (issue #339) ------------------------------------------------
// Where the viewport lands on load: the stored per-client offset when it still
// SHOWS work, otherwise the bounding box of the restored windows. Both legs end
// in the same clamp, so a stored offset from a bigger screen is pulled into the
// current extent rather than silently swallowed by the DOM.
const LAND_VIEW = { width: 1052, height: 854 };
const LAND_EXT = { width: 3200, height: 2080 };
const LAND_RECTS = [
  { left: 1600, top: 900, width: 600, height: 380 },
  { left: 2400, top: 1500, width: 600, height: 380 },
];
// The bbox landing these rects imply: bbox {1600,900,1400x980} centred in a
// 1052x854 viewport → 1600 + 700 - 526 = 1774, 900 + 490 - 427 = 963.
const BBOX_LANDING = { left: 1774, top: 963 };

const LAND = [
  {
    name: "an empty stage with nothing stored lands on the origin",
    stored: null,
    rects: [],
    want: { left: 0, top: 0 },
  },
  {
    name: "nothing stored lands on the bounding box of the restored windows",
    stored: null,
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
  {
    // DELIBERATELY not the bbox landing: a `viewLanding` that ignored `stored`
    // entirely would pass this row if the two coincided, which is the one
    // mutant the honour path exists to catch.
    name: "a stored offset that still shows a window is honoured verbatim",
    stored: { left: 1500, top: 850 },
    rects: LAND_RECTS,
    want: { left: 1500, top: 850 },
  },
  {
    // NEGATIVE CONTROL: 0,0 is a perfectly well-formed stored offset that shows
    // NO window here. Invert the intersection test (or drop it and trust any
    // stored pair) and this row answers {0,0} instead of the bbox landing.
    name: "a stored offset showing no window at all falls back to the bounding box",
    stored: { left: 0, top: 0 },
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
  {
    name: "a stored offset past the extent is clamped, not discarded",
    stored: { left: 9999, top: 9999 },
    rects: LAND_RECTS,
    want: { left: 2148, top: 1226 },
  },
  {
    name: "a corrupt stored offset is treated as absent",
    stored: { left: "x", top: null },
    rects: LAND_RECTS,
    want: BBOX_LANDING,
  },
];

for (const row of LAND) {
  test(`viewLanding: ${row.name}`, () => {
    const got = load().viewLanding(row.stored, row.rects, LAND_VIEW, LAND_EXT);
    assert.deepEqual(got, row.want);
  });
}

// ---- panNudge (issue #337) ---------------------------------------------------
// How far the plane scrolls per animation frame while a window is dragged
// against the viewport edge. `viewport` is a CLIENT rect with a NON-ZERO origin
// on purpose: a handler that mistakes client coordinates for viewport-relative
// ones reds every EDGE row here — the centre row answers {0,0} under that
// mutant too, so it is not the one doing the discriminating.
const PAN_VIEW = { left: 100, top: 50, right: 1100, bottom: 850 };
const BAND = 48;
const STEP = 24;

const NUDGE = [
  { name: "the middle of the viewport does not pan", pointer: { x: 600, y: 400 }, want: { dx: 0, dy: 0 } },
  {
    // NEGATIVE CONTROL: exactly at the band's outer lip. An off-by-one `<=`
    // starts the loop here, one pixel before the operator asked for it.
    name: "one pixel outside the band is still not panning",
    pointer: { x: 1052, y: 400 },
    want: { dx: 0, dy: 0 },
  },
  { name: "just inside the right band creeps", pointer: { x: 1054, y: 400 }, want: { dx: 1, dy: 0 } },
  { name: "half-way into the right band is half speed", pointer: { x: 1076, y: 400 }, want: { dx: 12, dy: 0 } },
  { name: "at the right edge is full speed", pointer: { x: 1100, y: 400 }, want: { dx: 24, dy: 0 } },
  {
    name: "PAST the right edge is capped, never faster",
    pointer: { x: 1400, y: 400 },
    want: { dx: 24, dy: 0 },
  },
  { name: "at the left edge pans back", pointer: { x: 100, y: 400 }, want: { dx: -24, dy: 0 } },
  { name: "at the top edge pans up", pointer: { x: 600, y: 50 }, want: { dx: 0, dy: -24 } },
  { name: "at the bottom edge pans down", pointer: { x: 600, y: 850 }, want: { dx: 0, dy: 24 } },
  {
    name: "a corner pans on both axes at once",
    pointer: { x: 1100, y: 850 },
    want: { dx: 24, dy: 24 },
  },
  {
    name: "the opposite corner pans back on both axes",
    pointer: { x: 100, y: 50 },
    want: { dx: -24, dy: -24 },
  },
  {
    // NEGATIVE CONTROL: a viewport narrower than two bands. The rule is written
    // as a DIFFERENCE of the two edge pressures so they cancel here; a naive
    // "in the right band → +step, else in the left band → -step" answers
    // {dx: 24} and the plane oscillates under the operator.
    name: "a viewport narrower than two bands cancels instead of oscillating",
    viewport: { left: 0, top: 0, right: 60, bottom: 850 },
    pointer: { x: 30, y: 400 },
    want: { dx: 0, dy: 0 },
  },
];

for (const row of NUDGE) {
  test(`panNudge: ${row.name}`, () => {
    const got = load().panNudge(row.pointer, row.viewport || PAN_VIEW, BAND, STEP);
    assert.deepEqual(got, row.want);
  });
}

// ---- autoPan: the loop around panNudge ---------------------------------------
// One loop serves the window drag and the fence drag. Frames are queued by a
// stub `requestAnimationFrame` and run by hand, so the test sees each tick.
function withFrames(body) {
  const frames = new Map();
  let next = 0;
  const saved = [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame];
  globalThis.requestAnimationFrame = (cb) => {
    next += 1;
    frames.set(next, cb);
    return next;
  };
  globalThis.cancelAnimationFrame = (id) => frames.delete(id);
  const runFrame = () => {
    const [id, cb] = frames.entries().next().value || [];
    if (!cb) return false;
    frames.delete(id);
    cb();
    return true;
  };
  try {
    body(frames, runFrame);
  } finally {
    [globalThis.requestAnimationFrame, globalThis.cancelAnimationFrame] = saved;
  }
}

function panHarness() {
  const ws = { scrollLeft: 0, scrollTop: 0, getBoundingClientRect: () => PAN_VIEW };
  const api = load({}, { getElementById: (id) => (id === "workspace" ? ws : null) });
  const node = { isConnected: true };
  const placed = [];
  const pan = api.autoPan(node, (p) => placed.push(p));
  return { ws, node, placed, pan };
}

test("autoPan: a pointer at the edge scrolls the plane and places again each frame", () => {
  withFrames((frames, runFrame) => {
    const { ws, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    assert.ok(runFrame());
    assert.ok(runFrame());
    assert.equal(ws.scrollLeft, 48);
    assert.equal(placed.length, 2);
    assert.equal(frames.size, 1, "the loop keeps running while the pointer is in the band");
  });
});

test("autoPan: stop ends the loop, so a released button pans nothing more", () => {
  withFrames((frames, runFrame) => {
    const { ws, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    pan.stop();
    assert.equal(frames.size, 0);
    assert.equal(runFrame(), false);
    assert.equal(ws.scrollLeft, 0);
  });
});

test("autoPan: a node that left the page ends the loop without placing it", () => {
  withFrames((frames, runFrame) => {
    const { ws, node, placed, pan } = panHarness();
    pan.follow({ x: 1100, y: 400 });
    node.isConnected = false;
    assert.ok(runFrame());
    assert.equal(frames.size, 0);
    assert.equal(ws.scrollLeft, 0);
    assert.equal(placed.length, 0);
  });
});

test("autoPan: a pointer away from the edges starts no loop", () => {
  withFrames((frames) => {
    const { pan } = panHarness();
    pan.follow({ x: 600, y: 400 });
    assert.equal(frames.size, 0);
  });
});

// The viewport and offset a new fence is placed against.
const FENCE_VIEW = { width: 1400, height: 900 };
const ORIGIN = { left: 0, top: 0 };

// Which slot a NEW fence takes. Indexing by `fences.length` reuses a slot after
// a removal, which is how an overlap ships before ADR-0051 §6 exists to enforce
// it away — every row below is that bug's oracle.
const SLOTS = [
  { name: "the first fence on an empty plane takes slot 0", rects: [], want: 0 },
  {
    name: "a second fence takes the next free slot",
    rects: [{ left: 40, top: 40, width: 720, height: 460 }],
    want: 1,
  },
  {
    // NEGATIVE CONTROL: `fences.length` answers 2 here and lands the new fence
    // exactly on the surviving slot-2 rect.
    name: "after the middle of three is removed, the FREED slot is reused, not the survivor's",
    rects: [
      { left: 40, top: 40, width: 720, height: 460 },
      { left: 40, top: 524, width: 720, height: 460 },
    ],
    want: 1,
  },
  {
    name: "a plane whose first four slots are full spills into the fifth",
    rects: [0, 1, 2, 3].map((i) => ({
      left: 40 + (i % 2) * 744,
      top: 40 + Math.floor(i / 2) * 484,
      width: 720,
      height: 460,
    })),
    want: 4,
  },
];

for (const row of SLOTS) {
  test(`nextFenceSlot: ${row.name}`, () => {
    assert.equal(load().nextFenceSlot(row.rects, ORIGIN, FENCE_VIEW), row.want);
  });
}

// The scan runs PAST the fence count. Bounding it at `taken.length` made a
// viewport covered by ONE big fence unbuildable: slots 0..1 both land inside it,
// the loop ran out, and the operator's New-fence click produced a refusal flash
// instead of a fence — with free plane sitting one row below. The row below is
// that bug's oracle: the old bound answered `1` (a slot INSIDE the blocker).
test("nextFenceSlot: a fence covering the whole viewport spills below it, not onto it", () => {
  const wb = load();
  const blocker = { left: 0, top: 0, width: 2000, height: 1200 };
  const slot = wb.nextFenceSlot([blocker], ORIGIN, FENCE_VIEW);
  assert.ok(slot > 0, `expected a slot below the blocker, got ${slot}`);
  const born = wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, slot);
  assert.ok(born.top >= blocker.top + blocker.height, "the new fence still lands on the blocker");
});

// …and `-1` is the honest "nowhere", so the caller can refuse rather than nudge
// the fence into a gap nobody chose. A single rect covering the whole scanned
// band is the only way to reach it.
test("nextFenceSlot: a plane with no free slot in the scanned band answers -1", () => {
  const wb = load();
  const wall = { left: 0, top: 0, width: 100000, height: 100000 };
  assert.equal(wb.nextFenceSlot([wall], ORIGIN, FENCE_VIEW), -1);
});

test("nextFenceSlot: the slot it picks never overlaps an existing fence", () => {
  const wb = load();
  // Three fences with a HOLE at slot 1 — the shape a removal leaves behind.
  const rects = [0, 2, 3].map((i) => wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, i));
  const slot = wb.nextFenceSlot(rects, ORIGIN, FENCE_VIEW);
  const born = wb.fenceSpawnRect(ORIGIN, FENCE_VIEW, slot);
  const overlaps = (a, b) =>
    a.left < b.left + b.width &&
    a.left + a.width > b.left &&
    a.top < b.top + b.height &&
    a.top + a.height > b.top;
  for (const r of rects) {
    assert.ok(!overlaps(born, r), `slot ${slot} lands on ${JSON.stringify(r)}`);
  }
});

// The repos a fence's members belong to, for the fence's own chrome. Sorted
// because DOM order is not stable, deduped because two consoles on one repo
// read as one place.
const REPOS = [
  { name: "no members read as no repos", members: [], want: "" },
  {
    // NEGATIVE CONTROL for the dedupe: a plain join answers "alpha · alpha".
    name: "two members on one repo read as that one repo",
    members: [{ repo: "alpha" }, { repo: "alpha" }],
    want: "alpha",
  },
  {
    // NEGATIVE CONTROL for the sort: a DOM-ordered join answers "beta · alpha".
    name: "two repos read alphabetically, whatever order the members arrive in",
    members: [{ repo: "beta" }, { repo: "alpha" }],
    want: "alpha · beta",
  },
  {
    // NEGATIVE CONTROL: `"~"` is the desk's spelling of "no repo" — the storage
    // token must not leak, the same rule `list()` applies.
    name: "a member with no repo reads as home",
    members: [{ repo: "~" }],
    want: "home",
  },
  {
    name: "home sorts among the named repos, deduped like any other",
    members: [{ repo: "zeta" }, { repo: "~" }, { repo: "~" }],
    want: "home · zeta",
  },
];

for (const row of REPOS) {
  test(`fenceRepos: ${row.name}`, () => {
    assert.equal(load().fenceRepos(row.members), row.want);
  });
}

// ---- the fence list is the map (issue #343) ---------------------------------
// One fold feeds the fence's own chrome AND the toolbar list, so the two can
// never disagree. One entry per fence, IN ORDER, whether or not it holds
// anything.
const SUM_A = { left: 0, top: 0, width: 200, height: 200 };
const box = (x, y) => ({ left: x - 10, top: y - 10, width: 20, height: 20 });

const SUMMARIES = [
  {
    name: "two members read their count and their repos, sorted and home-renamed",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "~", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, repos: "a · home", locked: false }],
  },
  {
    // Dedup is a REPOS rule, not a count rule: two consoles on one repo read as
    // one place but still as two consoles.
    name: "two members on one repo keep a count of two",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [
      { id: "w1", repo: "a", rect: box(50, 50) },
      { id: "w2", repo: "a", rect: box(150, 150) },
    ],
    want: [{ id: "a", name: "alpha", count: 2, repos: "a", locked: false }],
  },
  {
    name: "a locked fence says so, and a fence without the key reads unlocked",
    fences: [
      { id: "a", name: "alpha", rect: SUM_A, locked: true },
      { id: "b", name: "beta", rect: { left: 400, top: 400, width: 100, height: 100 } },
    ],
    windows: [{ id: "w1", repo: "a", rect: box(150, 150) }],
    want: [
      { id: "a", name: "alpha", count: 1, repos: "a", locked: true },
      { id: "b", name: "beta", count: 0, repos: "", locked: false },
    ],
  },
  {
    name: "an empty fence is still listed, at zero",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [],
    want: [{ id: "a", name: "alpha", count: 0, repos: "", locked: false }],
  },
  {
    // NEGATIVE CONTROL: the centre sits exactly on `left + width`. A CLOSED
    // containment counts it as a member and reds this row.
    name: "a member whose centre is on the far edge belongs to no fence",
    fences: [{ id: "a", name: "alpha", rect: SUM_A }],
    windows: [{ id: "w1", repo: "a", rect: box(200, 100) }],
    want: [{ id: "a", name: "alpha", count: 0, repos: "", locked: false }],
  },
  {
    // NEGATIVE CONTROL for "exactly one fence": hand-overlapped rects (which a
    // hand-edited desk.toml can carry) must not double-count the shared member.
    name: "two overlapping fences split a shared member into the FIRST one only",
    fences: [
      { id: "a", name: "alpha", rect: SUM_A },
      { id: "b", name: "beta", rect: { left: 100, top: 100, width: 200, height: 200 } },
    ],
    windows: [{ id: "w1", repo: "a", rect: box(150, 150) }],
    want: [
      { id: "a", name: "alpha", count: 1, repos: "a", locked: false },
      { id: "b", name: "beta", count: 0, repos: "", locked: false },
    ],
  },
];

for (const row of SUMMARIES) {
  test(`fenceSummaries: ${row.name}`, () => {
    const got = load().fenceSummaries(row.fences, row.windows);
    assert.deepEqual(got, row.want);
    // Asserted as a RELATION on every row, not only the overlap one: no window
    // may be counted twice, whatever the rects.
    assert.ok(
      got.reduce((n, s) => n + s.count, 0) <= row.windows.length,
      "a window is a member of at most ONE fence",
    );
  });
}

// Where a console born into a focused fence lands. The fence has a NON-ZERO
// origin as a built-in negative control: an implementation that cascades from
// 0,0 and forgets `fence.left`/`fence.top` reds every row.
const SPAWN_F = { left: 700, top: 300, width: 600, height: 460 };
const HEAD = 28;

const SPAWNS = [
  {
    name: "the first console takes the fence's full inner box",
    fence: SPAWN_F,
    index: 0,
    want: { left: 712, top: 340, width: 560, height: 340 },
  },
  {
    // NEGATIVE CONTROL for the room cap: a bare `k * step` answers
    // `left: 772, top: 412` here — 60 and 72 px of cascade in a fence with only
    // 16 and 68 px of slack, walking the window out of its own fence.
    name: "a later slot cascades only as far as the fence has room",
    fence: SPAWN_F,
    index: 3,
    want: { left: 728, top: 408, width: 560, height: 340 },
  },
  {
    name: "the cascade wraps at eight, so slot 8 is slot 0 again",
    fence: SPAWN_F,
    index: 8,
    want: { left: 712, top: 340, width: 560, height: 340 },
  },
  {
    // A fence smaller than `.session-window`'s CSS floor (240x150): the box is
    // BELOW it on both axes, which is exactly what the caller relaxes inline.
    name: "a small fence yields a box below the CSS floor rather than one that escapes",
    fence: { left: 0, top: 0, width: 200, height: 120 },
    index: 7,
    want: { left: 12, top: 40, width: 176, height: 68 },
  },
];

for (const row of SPAWNS) {
  test(`spawnRectIn: ${row.name}`, () => {
    assert.deepEqual(load().spawnRectIn(row.fence, row.index, HEAD), row.want);
  });
}

test("spawnRectIn: the box lies inside the fence at every cascade slot", () => {
  const wb = load();
  for (const row of SPAWNS) {
    for (let i = 0; i < 8; i++) {
      const b = wb.spawnRectIn(row.fence, i, HEAD);
      // Asserted as a RELATION: the right COUNT of wrong boxes must still red.
      const detail = `${row.name} slot ${i}: ${JSON.stringify(b)} escapes ${JSON.stringify(row.fence)}`;
      assert.ok(b.left >= row.fence.left, detail);
      assert.ok(b.top >= row.fence.top + HEAD, detail);
      assert.ok(b.left + b.width <= row.fence.left + row.fence.width, detail);
      assert.ok(b.top + b.height <= row.fence.top + row.fence.height, detail);
      assert.ok(b.width > 0 && b.height > 0, detail);
    }
  }
});

// ---- a console is never born held by a locked fence -------------------------
// The free cascade from a viewport at 0,0: slot k sits at 30+24k, 20+24k.
const FREE_VIEW = { left: 0, top: 0, width: 1000, height: 600 };
const FREE_BOX = { width: 560, height: 340 };

test("freeSpawnRect: with no fences, slot k is the plain viewport cascade", () => {
  const got = load().freeSpawnRect(FREE_VIEW, 2, []);
  assert.deepEqual(got, { rect: { left: 78, top: 68, ...FREE_BOX }, moved: false });
});

test("freeSpawnRect: an UNLOCKED fence under the slot does not move the console", () => {
  const fence = { id: "f", rect: { left: 0, top: 0, width: 800, height: 500 }, locked: false };
  const got = load().freeSpawnRect(FREE_VIEW, 0, [fence]);
  assert.deepEqual(got, { rect: { left: 30, top: 20, ...FREE_BOX }, moved: false });
});

test("freeSpawnRect: a slot a locked fence holds is skipped for the next free one", () => {
  // Slot 0's centre is (310, 190); slot 1's is (334, 214). The fence holds
  // only the first one.
  const fence = { id: "f", rect: { left: 300, top: 180, width: 20, height: 20 }, locked: true };
  const wb = load();
  const got = wb.freeSpawnRect(FREE_VIEW, 0, [fence]);
  assert.deepEqual(got, { rect: { left: 54, top: 44, ...FREE_BOX }, moved: false });
  assert.equal(wb.fenceHolds([fence], got.rect, false), false);
});

test("freeSpawnRect: a locked fence over every slot pushes the console past its right edge", () => {
  const big = { id: "big", rect: { left: 0, top: 0, width: 1200, height: 800 }, locked: true };
  // The box past `big` lands in a second locked fence: the walk passes that too.
  const next = { id: "next", rect: { left: 1200, top: 0, width: 700, height: 800 }, locked: true };
  const wb = load();
  const got = wb.freeSpawnRect(FREE_VIEW, 0, [big, next]);
  assert.equal(got.moved, true);
  assert.deepEqual(got.rect, { left: 1912, top: 20, ...FREE_BOX });
  assert.equal(wb.fenceHolds([big, next], got.rect, false), false);
});

test("freeSpawnRect: the cascade is anchored at the viewport offset", () => {
  const got = load().freeSpawnRect({ left: 3000, top: 900, width: 0, height: 0 }, 0, []);
  // An unmeasurable viewport takes the plain caps.
  assert.deepEqual(got.rect, { left: 3030, top: 920, ...FREE_BOX });
});

// ---- walking the fences from the keyboard -----------------------------------
// Alt+Shift+←/→ steps through the fences in the plane's own READING ORDER — top
// band first, left to right inside it — not in the order the desk array happens
// to carry them. Creation order on a plane means the walk teleports across the
// stage; reading order makes the shortcut a sweep.
//
// A 120 px band is what keeps a row a row: two fences placed side by side are
// never pixel-aligned on `top`, and a raw `top` sort would zig-zag between them.
const at = (id, left, top) => ({ id, rect: { left, top, width: 400, height: 300 } });

// Deliberately shuffled against reading order: `c` is first in the array and
// last on the plane, so every row below is red under a fold that walks the
// array.
const GRID = [
  at("c", 900, 700), //  bottom row, right
  at("b", 900, 40), //   top row, right
  at("d", 60, 700), //   bottom row, left
  at("a", 60, 40), //    top row, left
];

const CYCLE = [
  { name: "no fences at all: nothing to walk", fences: [], from: null, step: 1, want: null },
  {
    name: "forward with nothing in hand enters at the top-left fence",
    from: null,
    step: 1,
    want: "a",
  },
  {
    // The other end, so "enters at an end" cannot pass by answering `order[0]`
    // for both directions.
    name: "backward with nothing in hand enters at the bottom-right fence",
    from: null,
    step: -1,
    want: "c",
  },
  { name: "forward walks left to right inside the top band", from: "a", step: 1, want: "b" },
  {
    // The band's own boundary: the walk leaves the top row only after both of
    // its fences, which is what a raw `top` sort would get wrong.
    name: "forward crosses to the next band after the last fence of this one",
    from: "b",
    step: 1,
    want: "d",
  },
  { name: "forward walks left to right inside the bottom band too", from: "d", step: 1, want: "c" },
  { name: "forward wraps from the last fence to the first", from: "c", step: 1, want: "a" },
  { name: "backward wraps from the first fence to the last", from: "a", step: -1, want: "c" },
  { name: "backward retraces the same order", from: "d", step: -1, want: "b" },
  {
    // A focus can outlive the fence that carried it (another client removed it
    // between the jump and the key): entering from an unknown id is the
    // no-fence-in-hand case, not a crash and not a stall.
    name: "an id no fence carries re-enters at the end the step names",
    from: "gone",
    step: -1,
    want: "c",
  },
  {
    name: "one fence: the walk is a no-op that still answers that fence",
    fences: [at("solo", 40, 40)],
    from: "solo",
    step: 1,
    want: "solo",
  },
];

for (const row of CYCLE) {
  test(`fenceCycle: ${row.name}`, () => {
    const got = load().fenceCycle(row.fences || GRID, row.from, row.step);
    assert.equal(got, row.want);
  });
}

test("fenceCycle breaks a tie on id, so every client walks the same order", () => {
  const same = [at("z", 100, 100), at("a", 100, 100)];
  assert.equal(load().fenceCycle(same, null, 1), "a");
  assert.equal(load().fenceCycle(same, "a", 1), "z");
});

// ---- detachFold: the detach registry's transitions ---------------------------
// The fold is the whole decision surface for detaching a fence into its own
// window — every case below is a RELATION (what the registry becomes, which
// effect comes back), never a count of popups, because the caller is what turns
// an effect into a `window.open`.

test("detachFold: the cap is four — the fifth detach is refused, not opened", () => {
  const WB = load();
  assert.equal(WB.DETACH_MAX, 4);
  let reg = [];
  const opened = [];
  for (const id of ["f-a", "f-b", "f-c", "f-d"]) {
    const out = WB.detachFold(reg, { type: "detach", fenceId: id });
    opened.push(...out.effects.filter((e) => e.type === "open").map((e) => e.fenceId));
    reg = out.registry;
  }
  assert.deepStrictEqual(opened, ["f-a", "f-b", "f-c", "f-d"]);
  assert.equal(reg.length, 4);

  // THE NEGATIVE CONTROL. With the cap check deleted this line goes green with
  // an `open` and a 5-long registry, so both halves are asserted: the effect is
  // the literal `refuse`, AND the registry came back untouched.
  const fifth = WB.detachFold(reg, { type: "detach", fenceId: "f-e" });
  assert.deepStrictEqual(fifth.effects, [{ type: "refuse", fenceId: "f-e", reason: "cap" }]);
  assert.equal(fifth.registry.length, 4);
  assert.deepStrictEqual(fifth.registry, reg);
});

test("detachFold: one popup per fence — detaching a detached fence focuses it", () => {
  const WB = load();
  const first = WB.detachFold([], { type: "detach", fenceId: "f-a" });
  assert.deepStrictEqual(first.effects, [{ type: "open", fenceId: "f-a" }]);
  assert.equal(first.registry.length, 1);

  const again = WB.detachFold(first.registry, { type: "detach", fenceId: "f-a" });
  assert.equal(again.registry.length, 1, "no second entry for the same fence");
  assert.equal(again.effects.length, 1);
  assert.equal(again.effects[0].type, "focus");
  assert.equal(again.effects[0].fenceId, "f-a");
});

// The popup re-origins a detached fence's members to its own top-left, while
// the fence records keep the stage coordinates. A member of a locked fence at
// the stage origin lands inside that same fence rect, so the stage's answer is
// "held" and the popup's answer must be "free".
test("fenceHolds: a locked fence holds its console on the stage, never in the popup", () => {
  const WB = load();
  const fences = [{ id: "f-a", rect: { left: 0, top: 0, width: 800, height: 600 }, locked: true }];
  const rect = { left: 12, top: 12, width: 300, height: 200 };
  assert.equal(WB.fenceHolds(fences, rect, false), true, "the stage honours the fence lock");
  assert.equal(WB.fenceHolds(fences, rect, true), false, "the popup moves the console freely");
  const open = [{ ...fences[0], locked: false }];
  assert.equal(WB.fenceHolds(open, rect, false), false, "an unlocked fence holds nothing");
});

test("detachFold: re-attach empties the registry, and a SECOND one is a no-op", () => {
  const WB = load();
  const held = WB.detachFold([], { type: "detach", fenceId: "f-a" }).registry;

  const home = WB.detachFold(held, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(home.effects, [{ type: "close", fenceId: "f-a" }]);
  assert.deepStrictEqual(home.registry, []);

  // Both the popup's `beforeunload` AND the opener's `closed` poll report the
  // re-attach: the doubled signal must not spawn the consoles twice.
  const twice = WB.detachFold(home.registry, { type: "reattach", fenceId: "f-a" });
  assert.deepStrictEqual(twice.effects, []);
  assert.deepStrictEqual(twice.registry, []);
});

test("detachFold: re-attaching a fence that was never detached changes nothing", () => {
  const WB = load();
  const out = WB.detachFold(["f-a"], { type: "reattach", fenceId: "f-zzz" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

test("detachFold: focus reaches a detached fence only", () => {
  const WB = load();
  const member = WB.detachFold(["f-a"], { type: "focus", fenceId: "f-a" });
  assert.deepStrictEqual(member.effects, [{ type: "focus", fenceId: "f-a" }]);
  assert.deepStrictEqual(member.registry, ["f-a"]);

  const stranger = WB.detachFold(["f-a"], { type: "focus", fenceId: "f-b" });
  assert.deepStrictEqual(stranger.effects, [], "nothing to focus for an attached fence");
  assert.deepStrictEqual(stranger.registry, ["f-a"]);
});

test("detachFold: an unknown event is inert, so a stray message cannot detach", () => {
  const WB = load();
  const out = WB.detachFold(["f-a"], { type: "heartbeat", fenceId: "f-a" });
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.registry, ["f-a"]);
});

// ---- peerFold: the heartbeat and the peer-lost transition (issue #347) -------
// The window is passed in, so these run in microseconds rather than six real
// seconds — which is the whole reason the rule is a pure fold and not a timer.
const WINDOW_MS = 6000;
const SEEN = 100000;

test("peerFold: silence past the window loses the peer, exactly once", () => {
  const WB = load();
  const lost = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS + 1 }, WINDOW_MS);
  assert.deepStrictEqual(lost.effects, [{ type: "peer-lost" }]);
  assert.equal(lost.state.lost, true);
  // The effect turns into a `window.close()`, so a SECOND tick must be silent.
  const again = WB.peerFold(lost.state, { type: "tick", at: SEEN + WINDOW_MS + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(again.effects, []);
  assert.equal(again.state.lost, true);
});

// THE NEGATIVE CONTROL for the whole rule: inverting the comparison, or dropping
// the beat's `seen` update, makes this row red while the one above stays green.
test("peerFold: a beat inside the window does not expire — an F5 costs no popup", () => {
  const WB = load();
  const beaten = WB.peerFold({ seen: SEEN, lost: false }, { type: "beat", at: SEEN + 5000 }, WINDOW_MS);
  assert.deepStrictEqual(beaten.effects, []);
  assert.equal(beaten.state.seen, SEEN + 5000);
  const tick = WB.peerFold(beaten.state, { type: "tick", at: SEEN + 6001 }, WINDOW_MS);
  assert.deepStrictEqual(tick.effects, [], "1001 ms of silence is well inside a 6000 ms window");
  assert.equal(tick.state.lost, false);
});

test("peerFold: the boundary is strict — a tick exactly at the window is still alive", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "tick", at: SEEN + WINDOW_MS }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: a peer never heard from does not expire — the boot-adoption grace", () => {
  const WB = load();
  const out = WB.peerFold({ seen: null, lost: false }, { type: "tick", at: SEEN + 999999 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, false);
});

test("peerFold: loss is terminal — a beat after the loss does not resurrect the peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: true }, { type: "beat", at: SEEN + 1 }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.equal(out.state.lost, true);
  assert.equal(out.state.seen, SEEN, "a lost peer's clock stops with it");
});

test("peerFold: an announced departure loses the peer without waiting the window out", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "gone" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, [{ type: "peer-lost" }]);
  assert.equal(out.state.lost, true);
});

test("peerFold: an unknown event is inert, so a stray channel message cannot lose a peer", () => {
  const WB = load();
  const out = WB.peerFold({ seen: SEEN, lost: false }, { type: "origin-ping" }, WINDOW_MS);
  assert.deepStrictEqual(out.effects, []);
  assert.deepStrictEqual(out.state, { seen: SEEN, lost: false });
  const undef = WB.peerFold(undefined, { type: "tick", at: SEEN }, WINDOW_MS);
  assert.deepStrictEqual(undef.effects, []);
  assert.deepStrictEqual(undef.state, { seen: null, lost: false });
});

// --- the fence cap and the default name -------------------------------------
// The name used to be `Fence ${fences.length + 1}`. MEASURED against the running
// shell: `length` freezes at the cap, so the 13th fence, the 14th and every one
// after were all born "Fence 13" — three of them coexisting in the list that IS
// the plane's map. Numbering from the names already present cannot collide.
test("nextFenceName counts from the names on the plane, not from how many there are", () => {
  const WB = load();
  assert.equal(WB.nextFenceName([]), "Fence 1");
  assert.equal(WB.nextFenceName(undefined), "Fence 1");
  assert.equal(
    WB.nextFenceName([{ name: "Fence 1" }, { name: "Fence 2" }, { name: "Fence 3" }]),
    "Fence 4",
  );
  // THE COLLISION: twelve fences whose numbers run past twelve. Counting would
  // answer "Fence 13" for the second time.
  const twelve = Array.from({ length: 12 }, (_, i) => ({ name: `Fence ${i + 2}` }));
  assert.equal(WB.nextFenceName(twelve), "Fence 14");
  // A gap is not filled: the highest number wins, so a name is never reused by a
  // fence the operator did not delete.
  assert.equal(WB.nextFenceName([{ name: "Fence 1" }, { name: "Fence 9" }]), "Fence 10");
  // A renamed fence is not a number. "backend" must not push the next default
  // anywhere, and "Fence 99" is a number the operator chose — so it counts.
  assert.equal(WB.nextFenceName([{ name: "backend" }, { name: "planning" }]), "Fence 1");
  assert.equal(WB.nextFenceName([{ name: "Fence 99" }]), "Fence 100");
  // Near-misses are not numbers either.
  assert.equal(
    WB.nextFenceName([{ name: "Fence" }, { name: "Fence 2b" }, { name: "fence 5" }, { name: null }]),
    "Fence 1",
  );
});

test("the fence cap is a number the shell can state, and the plane is at it from the 12th fence", async () => {
  assert.equal(load().FENCE_MAX, 12);
  // A desk with `n` fences, landed through the real GET.
  const plane = async (n) => {
    const fences = Array.from({ length: n }, (_, i) => ({
      id: `f${i}`,
      name: `Fence ${i + 1}`,
      rect: { left: i * 10, top: 0, width: 5, height: 5 },
      locked: false,
      ts: i,
    }));
    const realFetch = globalThis.fetch;
    globalThis.fetch = async () => ({ ok: true, json: async () => ({ windows: [], fences }) });
    try {
      const wb = load({
        WBConsoleOpts: { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } },
      });
      await wb.whenDeskLoaded();
      assert.equal(wb.fenceRecords().length, n, "the desk landed");
      return wb;
    } finally {
      globalThis.fetch = realFetch;
    }
  };
  assert.equal((await plane(0)).atFenceCap(), false, "an empty plane");
  assert.equal((await plane(11)).atFenceCap(), false, "one below the cap");
  const full = await plane(12);
  assert.equal(full.atFenceCap(), true, "at the cap");
  assert.equal(full.createFence(), false, "a fence past the cap is refused");
});

// --- pasteDecision: the image-paste rule (ADR-0055 §5) ----------------------

test("pasteDecision lets a text-only paste fall through to xterm", () => {
  const { pasteDecision } = load();
  assert.equal(pasteDecision({ types: ["text/plain"], size: -1, watching: false }), "passthrough");
  assert.equal(pasteDecision({ types: [], size: -1, watching: false }), "passthrough");
  assert.equal(pasteDecision({ types: undefined, size: -1, watching: false }), "passthrough");
  // Even a watcher's TEXT paste is xterm's business (its own gate refuses it).
  assert.equal(pasteDecision({ types: ["text/plain"], size: -1, watching: true }), "passthrough");
});

test("pasteDecision hands an image to image.write for the baton holder", () => {
  const { pasteDecision } = load();
  assert.equal(pasteDecision({ types: ["image/png"], size: 1234, watching: false }), "drop");
  // Text alongside the image (a browser copies both from a web page): the image wins.
  assert.equal(
    pasteDecision({ types: ["text/html", "text/plain", "image/png"], size: 10, watching: false }),
    "drop",
  );
  assert.equal(pasteDecision({ types: ["image/png"], size: 0, watching: false }), "drop");
});

test("pasteDecision refuses a watcher's image visibly, before any size question", () => {
  const { pasteDecision } = load();
  assert.equal(pasteDecision({ types: ["image/png"], size: 10, watching: true }), "watched");
  assert.equal(pasteDecision({ types: ["image/png"], size: 1e9, watching: true }), "watched");
});

test("pasteDecision refuses an image past the daemon's cap without sending it", () => {
  const { pasteDecision } = load();
  const cap = 4 * 1024 * 1024;
  assert.equal(pasteDecision({ types: ["image/png"], size: cap, watching: false }), "drop");
  assert.equal(pasteDecision({ types: ["image/png"], size: cap + 1, watching: false }), "too-large");
  // An image item whose file could not be read has no size: refuse, never send.
  assert.equal(pasteDecision({ types: ["image/png"], size: -1, watching: false }), "too-large");
  assert.equal(pasteDecision({ types: ["image/png"], size: undefined, watching: false }), "too-large");
});

// --- endNotice: the last line of a console that gave up -------------------
// A refused launch never had a session, so this line is the only place the
// browser can show why. Every other end keeps the line it always printed.

test("endNotice names the reason of a refused launch, and only of one", () => {
  const { endNotice } = load();
  assert.equal(endNotice("refused", "unknown repo"), "[could not start: unknown repo]");
  assert.equal(endNotice("refused", "  unknown repo \n"), "[could not start: unknown repo]");
  // A refusal with no words still says it did not start, never "undefined".
  assert.equal(endNotice("refused", null), "[could not start]");
  assert.equal(endNotice("refused", ""), "[could not start]");
  assert.equal(endNotice("refused", 42), "[could not start]");
  for (const reason of ["child-exited", "daemon-shutdown", "taken-over", null]) {
    assert.equal(endNotice(reason, "ignored"), "[session closed]", String(reason));
  }
});

// --- peer placeholders: a console whose peer cannot serve it ---------------
// The box words the peer's fleet state, offers only the action that can fix
// it, and comes back by itself only as a shell.

const PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const peerGroup = (state, extra = {}) => ({
  daemon: PEER,
  environment: "macOS 15",
  name: "corcino-mac",
  tunnel: true,
  state,
  diagnosis: `diagnosis of ${state}`,
  nudgeable: false,
  local: false,
  ...extra,
});

test("peerOfflineView words each peer state and offers only the action that fixes it", () => {
  const { peerOfflineView } = load();
  const wsl = { tunnel: false, name: "", environment: "WSL: Ubuntu", nudgeable: true };
  const rows = [
    [peerGroup("asleep", wsl), "WSL: Ubuntu is asleep.", "wake"],
    [peerGroup("unreachable", wsl), "Ralphy is not running on WSL: Ubuntu.", "wake"],
    [peerGroup("unreachable"), "Ralphy is not running on corcino-mac.", "retry"],
    [peerGroup("tunnel-closed"), "Reconnecting to corcino-mac…", "wait"],
    [
      peerGroup("tunnel-silent"),
      "corcino-mac does not answer.",
      "retry",
    ],
    [peerGroup("unauthorized"), "corcino-mac cannot open this console.", null],
    [peerGroup("version-mismatch"), "corcino-mac cannot open this console.", null],
    [peerGroup("refused"), "corcino-mac cannot open this console.", null],
    [peerGroup("malformed"), "corcino-mac cannot open this console.", null],
    // The fleet has not seen what the refused launch saw.
    [peerGroup("reachable"), "corcino-mac did not start this console.", "retry"],
  ];
  for (const [group, text, action] of rows) {
    const got = peerOfflineView(group, "refused words", "fallback");
    assert.deepEqual(
      got,
      { text, detail: `diagnosis of ${group.state}`, action },
      group.state,
    );
  }
});

test("peerOfflineView with no fleet group yet uses the refusal and the recorded environment", () => {
  const { peerOfflineView } = load();
  assert.deepEqual(peerOfflineView(null, "  tunnel open, daemon silent \n", "macOS 15"), {
    text: "macOS 15 did not start this console.",
    detail: "tunnel open, daemon silent",
    action: "retry",
  });
  // Nothing known at all still names someone, and shows no empty detail.
  assert.deepEqual(peerOfflineView(null, undefined, ""), {
    text: "The other computer did not start this console.",
    detail: "",
    action: "retry",
  });
});

test("peerReturnDecision relaunches only a shell, only after the box saw its peer offline", () => {
  const { peerReturnDecision } = load();
  const back = { available: true, wasOffline: true };
  assert.equal(peerReturnDecision({ kind: "console", canLaunch: true, ...back }), "relaunch");
  // A vendor CLI never starts without a click.
  assert.equal(peerReturnDecision({ kind: "agent", canLaunch: true, ...back }), "offer");
  // The popup authors no session at all.
  assert.equal(peerReturnDecision({ kind: "console", canLaunch: false, ...back }), "offer");
  // A refusal on a peer the fleet still calls reachable: no relaunch loop.
  assert.equal(
    peerReturnDecision({ kind: "console", canLaunch: true, available: true, wasOffline: false }),
    "stay",
  );
  assert.equal(
    peerReturnDecision({ kind: "console", canLaunch: true, available: false, wasOffline: true }),
    "stay",
  );
});

test("heldReturnDecision launches only when the list heard from the peer that nothing runs", () => {
  const { heldReturnDecision } = load();
  const row = { id: 3, repo: "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/repo" };
  // Not known (the list did not hear from the peer): never a launch.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: undefined }), "stay");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: true, session: undefined }), "stay");
  // It still runs there: attach, whatever the kind.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: row }), "attach");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: false, session: row }), "attach");
  // Heard, and not running: a shell opens again, a vendor CLI waits for a click.
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: true, session: null }), "relaunch");
  assert.equal(heldReturnDecision({ kind: "agent", canLaunch: true, session: null }), "offer");
  assert.equal(heldReturnDecision({ kind: "console", canLaunch: false, session: null }), "offer");
});

test("peerHeld holds a peer project only while its known peer cannot serve it", () => {
  const { peerHeld } = load();
  const ref = `${PEER}/owner/repo`;
  const groups = (state) => new Map([[PEER, peerGroup(state)]]);
  assert.equal(peerHeld(ref, groups("tunnel-silent"))?.state, "tunnel-silent");
  assert.equal(peerHeld(ref, groups("asleep"))?.state, "asleep");
  assert.equal(peerHeld(ref, groups("reachable")), null);
  // No state yet counts as available, as in the sidebar.
  assert.equal(peerHeld(ref, groups("")), null);
  assert.equal(peerHeld(ref, new Map()), null);
  assert.equal(peerHeld("owner/repo", groups("tunnel-silent")), null);
});

test("peerGate holds a reattach only while its known peer cannot serve it", () => {
  const { peerGate } = load();
  const opening = ["connect", "reconnect", "park-as-watcher"];
  const down = ["asleep", "unreachable", "tunnel-closed", "tunnel-silent", "unauthorized", "version-mismatch", "refused", "malformed"];
  for (const decision of opening) {
    for (const state of down) {
      assert.equal(peerGate({ decision, group: peerGroup(state), id: 7 }), "hold", `${decision} on ${state}`);
    }
    // Reachable, a state not heard yet, and no group: the socket opens.
    assert.equal(peerGate({ decision, group: peerGroup("reachable"), id: 7 }), decision);
    assert.equal(peerGate({ decision, group: peerGroup(""), id: 7 }), decision);
    assert.equal(peerGate({ decision, group: null, id: 7 }), decision);
    // A launch has no session to hold for: its placeholder says why.
    assert.equal(peerGate({ decision, group: peerGroup("asleep"), id: null }), decision);
  }
  // A session that ended stays ended, whatever the peer does.
  assert.equal(peerGate({ decision: "give-up", group: peerGroup("asleep"), id: 7 }), "give-up");
});

test("reconnectDecision reattaches any unannounced close and gives up only on an announced end", () => {
  const { reconnectDecision } = load();
  const base = { everOpened: true, announced: null, idKnown: true, failedReopens: 0 };
  const rows = [
    ["no id: nothing to reattach to", { idKnown: false }, "give-up"],
    ["taken over: watch", { announced: "taken-over" }, "park-as-watcher"],
    ["the daemon said why", { announced: "child-exited" }, "give-up"],
    ["too many failed opens", { failedReopens: 11 }, "give-up"],
    ["a drop of a held session", {}, "reconnect"],
    ["never opened, first tries", { everOpened: false, failedReopens: 2 }, "reconnect"],
    ["never opened, then watch", { everOpened: false, failedReopens: 3 }, "park-as-watcher"],
  ];
  for (const [name, change, want] of rows) {
    assert.equal(reconnectDecision({ ...base, ...change }), want, name);
  }
  // A proxy closes cleanly for a socket the daemon dropped (ADR-0051 §9): the
  // close itself says nothing, so an unannounced clean close reconnects.
  for (const close of [{ code: 1000, wasClean: true }, { code: 1001, wasClean: true }, { code: 1005, wasClean: true }]) {
    assert.equal(reconnectDecision({ ...base, opened: true, ...close }), "reconnect", JSON.stringify(close));
  }
});

// --- resumeDecision: coming back from a suspend --------------------------
// A tablet's tab is frozen with its sockets still reporting OPEN, and the link
// is torn down without a close frame, so the exponential backoff never arms.
// `stale` is the caller's liveness verdict (the shell's presence heartbeat).
// The decision table itself is pinned once, in wb-daemon.test.mjs.

test("resumeAll and setStaleProbe are exported like the rest of the module's seam", () => {
  const c = load();
  assert.equal(typeof c.resumeAll, "function");
  assert.equal(typeof c.setStaleProbe, "function");
  // With no window open there is nothing to resume, and it must not throw:
  // `online` fires in a document that has painted no console at all.
  assert.equal(c.resumeAll(true), 0);
  // The probe seam is what keeps the shell's heartbeat verdict out of this
  // module. Setting a non-function clears it rather than poisoning the path.
  c.setStaleProbe(() => true);
  c.setStaleProbe(null);
  assert.equal(c.resumeAll(false), 0);
});

// --- encodeDetach: why the page closes a console socket ----------------------
// The daemon logs the reason, so a dormancy reattach is told apart from a
// network drop. The daemon's `Leave::from_command` test reads this same JSON.
const textOf = (frame) => new TextDecoder().decode(frame.subarray(1));

test("encodeDetach is a command frame that names the reason", () => {
  const { encodeDetach } = load();
  const frame = encodeDetach("dormant");
  assert.equal(frame[0], 0x02, "the command tag");
  assert.equal(textOf(frame), '{"id":0,"verb":"detach","payload":{"reason":"dormant"}}');
});

test("encodeResize keeps its frame", () => {
  const { encodeResize } = load();
  const frame = encodeResize(24, 80);
  assert.equal(frame[0], 0x02, "the command tag");
  assert.equal(textOf(frame), '{"id":0,"verb":"resize","payload":{"rows":24,"cols":80}}');
});

// --- dormancyDecision: a console off the viewport gives its renderer back ---
// Chrome caps a document at ~16 live WebGL contexts. Past that the xterm addon
// disposes itself and EVERY terminal falls back to the DOM renderer, so a desk
// gets slower the more consoles are open. A window scrolled well off the stage
// viewport therefore disposes its terminal and closes its socket, and rebuilds
// when it returns — the session is the daemon's, and the reattach replays it.
//
// The rule is a pure fold: the IntersectionObserver supplies `intersecting` and
// the caller owns the fifteen-second grace period. `live` is a window that is
// allowed to sleep, so each test below changes exactly one thing about it.
const live = {
  intersecting: false,
  covered: false,
  dormant: false,
  maximized: false,
  fullscreen: false,
  focused: false,
  hasTerminal: true,
  ended: false,
  sessionId: 7,
};

test("dormancyDecision sleeps only a live console nobody can see", () => {
  const { dormancyDecision } = load();
  const asleep = { dormant: true, hasTerminal: false };
  // [case, what changes about `live`, expected decision]
  const rows = [
    ["a live console off the viewport sleeps", {}, "sleep"],
    ["a visible console holds", { intersecting: true }, "hold"],
    // Visible outranks every other reading: a dormant window that comes back
    // wakes even while it is also maximized or focused.
    ["a dormant console that comes back wakes", { intersecting: true, ...asleep }, "wake"],
    [
      "a dormant console that comes back wakes even maximized and focused",
      { intersecting: true, ...asleep, maximized: true, focused: true },
      "wake",
    ],
    // Columns, a maximize or the physical screen fill the viewport. The
    // windows under them are inside it, so the observer calls them visible,
    // but nobody sees them: they sleep, and they do not wake until uncovered.
    ["a console under a full bleed sleeps", { intersecting: true, covered: true }, "sleep"],
    [
      "a dormant console under a full bleed stays asleep",
      { intersecting: true, covered: true, ...asleep },
      "hold",
    ],
    // Already asleep and still away: nothing to do. Without this the caller
    // would re-arm its timer on every observer callback for the life of the page.
    ["a window is never slept twice", { ...asleep, sessionId: null }, "hold"],
    // Both fill the viewport, so "outside" is a lie the observer can still tell
    // in the frame between the class landing and the layout that follows it.
    ["a maximized console holds", { maximized: true }, "hold"],
    ["a fullscreen console holds", { fullscreen: true }, "hold"],
    // Every drag and every resize begins with a `pointerdown` that calls
    // `focusWin`, so this one guard covers a window being hauled across the
    // plane without a second "dragging" flag nothing else in the module keeps.
    ["the focused (and dragged) console holds", { focused: true }, "hold"],
    ["a placeholder holds: there is no terminal to dispose", { hasTerminal: false }, "hold"],
    // No daemon-side session means no replay: sleeping would throw away the
    // last thing the agent said, permanently.
    ["an ended session holds", { ended: true }, "hold"],
    // The R1 hazard, in this direction: `WBSessionRoute.url` composes a LAUNCH
    // url when `id` is absent, so waking such a window would spawn a SECOND
    // vendor CLI rather than reattaching to the first.
    ["a null session id holds", { sessionId: null }, "hold"],
    ["an undefined session id holds", { sessionId: undefined }, "hold"],
    // Zero is a real session id, not an absent one.
    ["session id zero sleeps", { sessionId: 0 }, "sleep"],
  ];
  for (const [name, change, want] of rows) {
    assert.equal(dormancyDecision({ ...live, ...change }), want, name);
  }
});

// A restored console that reattaches starts asleep, so a console off the
// viewport never replays its text; the observer's first report wakes the
// visible ones.
test("birthDecision starts asleep only a reattach the observer can wake", () => {
  const { birthDecision } = load();
  const rows = [
    ["a reattach starts asleep", { id: 7, observed: true }, "dormant"],
    ["session id zero is a reattach", { id: 0, observed: true }, "dormant"],
    // A launch has no id to wake to: it would spawn a second CLI.
    ["a launch attaches", { id: null, observed: true }, "attach"],
    ["an undefined id attaches", { id: undefined, observed: true }, "attach"],
    // Without an IntersectionObserver nothing would ever wake the window.
    ["no observer: a reattach attaches", { id: 7, observed: false }, "attach"],
  ];
  for (const [name, inputs, want] of rows) {
    assert.equal(birthDecision(inputs), want, name);
  }
});

// --- keyboardInset: the virtual keyboard's bite out of the viewport --------
// iOS pans the visual viewport instead of resizing the layout one, so the
// keyboard's height has to be measured rather than reported.

test("keyboardInset is zero when no keyboard is up", () => {
  const { keyboardInset } = load();
  // The resting state on every desktop, and on Android once the layout viewport
  // has already shrunk by itself (interactive-widget=resizes-content).
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 900, offsetTop: 0, scale: 1 }),
    0,
  );
});

test("keyboardInset measures the occluded strip, panned or not", () => {
  const { keyboardInset } = load();
  // Resized visual viewport, not scrolled: the plain case.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 0, scale: 1 }),
    340,
  );
  // iOS panned the visual viewport down by 120: the strip we cannot paint into
  // is what is left over BELOW it, not the whole difference — counting the pan
  // twice would shrink the console by more than the keyboard takes.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 120, scale: 1 }),
    220,
  );
});

test("keyboardInset refuses to read a pinch as a keyboard", () => {
  const { keyboardInset } = load();
  // Zoomed in, `height` shrinks for a reason that has nothing to do with an
  // occluded bottom; subtracting it would shrink the console the operator just
  // zoomed into.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 400, offsetTop: 0, scale: 2.5 }),
    0,
  );
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 400, offsetTop: 0, scale: 0.5 }),
    0,
  );
  // A scale that is 1 to within measurement noise is not a pinch.
  assert.equal(
    keyboardInset({ innerHeight: 900, height: 560, offsetTop: 0, scale: 1.004 }),
    340,
  );
});

test("keyboardInset never returns a negative or a non-number", () => {
  const { keyboardInset } = load();
  // A visual viewport TALLER than the layout one is reported on some Android
  // builds mid-animation; a negative inset would grow the window off-screen.
  assert.equal(keyboardInset({ innerHeight: 900, height: 940, offsetTop: 0, scale: 1 }), 0);
  // Absent fields (a browser mid-teardown) must read as "no keyboard".
  assert.equal(keyboardInset({}), 0);
  assert.equal(keyboardInset({ innerHeight: NaN, height: 100, offsetTop: 0, scale: 1 }), 0);
  // Sub-pixel viewports are common on a scaled display: the CSS var is px.
  assert.equal(
    keyboardInset({ innerHeight: 900.4, height: 560.1, offsetTop: 0, scale: 1 }),
    340,
  );
});

// --- keySequence: the bytes a tapped key sends ----------------------------

test("keySequence sends the control characters a virtual keyboard has no key for", () => {
  const { keySequence } = load();
  assert.equal(keySequence("esc", false), "\x1b");
  assert.equal(keySequence("tab", false), "\t");
  assert.equal(keySequence("enter", false), "\r");
  assert.equal(keySequence("ctrl-c", false), "\x03");
  assert.equal(keySequence("slash", false), "/");
  // The mode does not touch them — only the arrows are mode-dependent.
  assert.equal(keySequence("esc", true), "\x1b");
  assert.equal(keySequence("enter", true), "\r");
  assert.equal(keySequence("ctrl-c", true), "\x03");
  assert.equal(keySequence("slash", true), "/");
});

test("keySequence follows the terminal into application cursor mode", () => {
  const { keySequence } = load();
  // Normal mode: CSI. A shell's history and line editing read these.
  assert.equal(keySequence("up", false), "\x1b[A");
  assert.equal(keySequence("down", false), "\x1b[B");
  assert.equal(keySequence("right", false), "\x1b[C");
  assert.equal(keySequence("left", false), "\x1b[D");
  // Application mode: SS3. A full-screen program (which is what a vendor CLI's
  // menu is) asked for this, and sending CSI there scrolls nothing.
  assert.equal(keySequence("up", true), "\x1bOA");
  assert.equal(keySequence("down", true), "\x1bOB");
  assert.equal(keySequence("right", true), "\x1bOC");
  assert.equal(keySequence("left", true), "\x1bOD");
});

test("keySequence with the Shift latch sends xterm's shifted forms", () => {
  const { keySequence } = load();
  // Tab becomes back-tab, which Claude Code cycles its modes on.
  assert.equal(keySequence("tab", false, true), "\x1b[Z");
  assert.equal(keySequence("tab", true, true), "\x1b[Z");
  // A shifted arrow carries modifier 2, and it is CSI in both cursor modes.
  for (const appCursor of [false, true]) {
    assert.equal(keySequence("up", appCursor, true), "\x1b[1;2A");
    assert.equal(keySequence("down", appCursor, true), "\x1b[1;2B");
    assert.equal(keySequence("right", appCursor, true), "\x1b[1;2C");
    assert.equal(keySequence("left", appCursor, true), "\x1b[1;2D");
  }
  // No Shift form: sent unchanged.
  assert.equal(keySequence("esc", false, true), "\x1b");
  assert.equal(keySequence("enter", false, true), "\r");
  assert.equal(keySequence("ctrl-c", false, true), "\x03");
  assert.equal(keySequence("slash", false, true), "/");
});

test("keySequence sends nothing for a name it does not know", () => {
  const { keySequence } = load();
  // The click handler routes `copy` and the font steps elsewhere; anything that
  // reaches here unrecognised must be silence, never a stray byte to the child.
  for (const name of ["copy", "font-up", "ctrl", "shift", "", null, undefined, "toString"]) {
    assert.equal(keySequence(name, false), "");
    assert.equal(keySequence(name, false, true), "");
  }
});

// --- applyCtrlLatch: a chord typed one finger at a time -------------------

test("applyCtrlLatch folds the next single character and disarms", () => {
  const { applyCtrlLatch } = load();
  assert.deepEqual(applyCtrlLatch(true, "c"), { out: "\x03", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "C"), { out: "\x03", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "d"), { out: "\x04", latched: false });
  assert.deepEqual(applyCtrlLatch(true, "["), { out: "\x1b", latched: false });
});

test("applyCtrlLatch passes everything through while disarmed", () => {
  const { applyCtrlLatch } = load();
  assert.deepEqual(applyCtrlLatch(false, "c"), { out: "c", latched: false });
  assert.deepEqual(applyCtrlLatch(false, "\x1b[A"), { out: "\x1b[A", latched: false });
});

test("applyCtrlLatch keeps the latch armed for input it cannot fold", () => {
  const { applyCtrlLatch } = load();
  // An arrow is three bytes: masking the first would corrupt the escape and
  // silently eat the key the operator meant to modify.
  assert.deepEqual(applyCtrlLatch(true, "\x1b[A"), { out: "\x1b[A", latched: true });
  // A paste arrives as one long string on the same path.
  assert.deepEqual(applyCtrlLatch(true, "hello"), { out: "hello", latched: true });
  // Outside @-_ there is no control character to fold to.
  assert.deepEqual(applyCtrlLatch(true, "1"), { out: "1", latched: true });
  assert.deepEqual(applyCtrlLatch(true, "\x03"), { out: "\x03", latched: true });
  // Non-strings never reach the child, and must not throw on the way.
  assert.deepEqual(applyCtrlLatch(true, undefined), { out: undefined, latched: true });
});

// --- terminalInputMode: the keyboard opens only from the key bar -----------

test("terminalInputMode hides the virtual keyboard until the bar opens it", () => {
  const { terminalInputMode } = load();
  // A touch screen: the field keeps focus with no keyboard on screen.
  assert.equal(terminalInputMode(true, false), "none");
  // The keyboard key opened it: the browser shows its keyboard again.
  assert.equal(terminalInputMode(true, true), null);
  // No bar (a desktop): the attribute stays absent, whatever the state.
  assert.equal(terminalInputMode(false, false), null);
  assert.equal(terminalInputMode(false, true), null);
});

// --- isTerminalReply: the answers a replayed backlog must not send ----------

test("isTerminalReply knows each answer xterm writes back for a query", () => {
  const { isTerminalReply } = load();
  // ConPTY's startup `ESC[6n`, answered again on every reattach: the stray `R`.
  assert.equal(isTerminalReply("\x1b[1;1R"), true);
  assert.equal(isTerminalReply("\x1b[24;80R"), true);
  assert.equal(isTerminalReply("\x1b[?24;80R"), true);
  assert.equal(isTerminalReply("\x1b[0n"), true);
  assert.equal(isTerminalReply("\x1b[?1;2c"), true);
  assert.equal(isTerminalReply("\x1b[>0;276;0c"), true);
  assert.equal(isTerminalReply("\x1b[?2004;1$y"), true);
  assert.equal(isTerminalReply("\x1b[8;24;80t"), true);
  assert.equal(isTerminalReply("\x1b[?0u"), true);
  assert.equal(isTerminalReply("\x1bP1$r0m\x1b\\"), true);
  assert.equal(isTerminalReply("\x1b]11;rgb:0000/0000/0000\x1b\\"), true);
  assert.equal(isTerminalReply("\x1b]10;rgb:ffff/ffff/ffff\x07"), true);
});

test("isTerminalReply lets typed keys and pastes through", () => {
  const { isTerminalReply } = load();
  for (const typed of [
    "R",
    "c",
    "\r",
    "\x03",
    "\x1b",
    "\x1b[A",
    "\x1bOR",
    "\x1b[3~",
    "\x1b[200~text\x1b[201~",
    "\x1b[I",
    "ls -la\r",
    "\x1b[1;1Rx",
    undefined,
  ]) {
    assert.equal(isTerminalReply(typed), false, JSON.stringify(typed));
  }
});

// --- keyBarVisible: when the row appears ----------------------------------

test("keyBarVisible obeys an explicit choice over the device", () => {
  const { keyBarVisible } = load();
  // The escape hatch in both directions: a desktop operator who wants the bar,
  // and a tablet operator with a hardware keyboard who does not.
  for (const coarse of [true, false]) {
    assert.equal(keyBarVisible("on", coarse), true);
    assert.equal(keyBarVisible("off", coarse), false);
  }
});

test("keyBarVisible defaults to whether the machine has a touch surface", () => {
  const { keyBarVisible } = load();
  // Absent, null, or a spelling from a future version: all auto.
  for (const mode of [null, undefined, "unset", "auto", ""]) {
    assert.equal(keyBarVisible(mode, true), true);
    assert.equal(keyBarVisible(mode, false), false);
  }
});

// --- pasteOffered: the paste key's enabled state ---------------------------

test("pasteOffered needs a clipboard that can READ", () => {
  const { pasteOffered } = load();
  // An insecure LAN origin has no `navigator.clipboard` at all.
  assert.equal(pasteOffered(undefined), false);
  assert.equal(pasteOffered(null), false);
  // A write-only shim (the clipboard test recorder, or an old engine) is not
  // enough: there is no `execCommand` fallback for a read.
  assert.equal(pasteOffered({ writeText() {} }), false);
  assert.equal(pasteOffered({ readText: "yes" }), false);
  assert.equal(pasteOffered({ readText() {} }), true);
});

// --- rightClickAction / holdMoveReport: the mouse under a TUI --------------

test("rightClickAction copies a selection and pastes without one", () => {
  const { rightClickAction } = load();
  // The selection wins over paste: copying it is what the press was for.
  assert.equal(rightClickAction(true, true), "copy");
  // The copy has an `execCommand` fallback, so an insecure origin copies too.
  assert.equal(rightClickAction(true, false), "copy");
  assert.equal(rightClickAction(false, true), "paste");
  // An insecure origin cannot read the clipboard, and the browser menu stays
  // closed: the press does nothing.
  assert.equal(rightClickAction(false, false), "none");
});

test("pressRoute holds only a plain left press under a TUI", () => {
  const { pressRoute } = load();
  for (const mode of ["x10", "vt200", "drag", "any"]) {
    assert.equal(pressRoute(mode, 0, false), "hold");
    // Any modifier keeps xterm's routing: Shift selects, Alt drags the child.
    assert.equal(pressRoute(mode, 0, true), "pass");
    // Middle and right have their own paths.
    assert.equal(pressRoute(mode, 1, false), "pass");
    assert.equal(pressRoute(mode, 2, false), "pass");
  }
  // A plain shell already selects on a drag.
  assert.equal(pressRoute("none", 0, false), "pass");
  assert.equal(pressRoute(undefined, 0, false), "pass");
});

test("forceSelectionKeys is Option on macOS and Shift elsewhere", () => {
  const { forceSelectionKeys } = load();
  assert.deepEqual(forceSelectionKeys("MacIntel"), { altKey: true });
  assert.deepEqual(forceSelectionKeys("iPad"), { altKey: true });
  // Alt outside macOS asks xterm for a column selection.
  assert.deepEqual(forceSelectionKeys("Win32"), { shiftKey: true });
  assert.deepEqual(forceSelectionKeys("Linux x86_64"), { shiftKey: true });
  assert.deepEqual(forceSelectionKeys(undefined), { shiftKey: true });
});

test("holdMoveReport holds only button-less moves over a selection under a TUI", () => {
  const { holdMoveReport } = load();
  assert.equal(holdMoveReport("any", true, 0), true);
  // Nothing to protect: the TUI keeps its hover.
  assert.equal(holdMoveReport("any", false, 0), false);
  // A pressed button is a drag, not a reach for the right button.
  assert.equal(holdMoveReport("any", true, 1), false);
  // No tracking: xterm reports nothing, so nothing to hold.
  assert.equal(holdMoveReport("none", true, 0), false);
  assert.equal(holdMoveReport(undefined, true, 0), false);
});

// --- clipboardContent: what the paste key pastes ---------------------------

test("clipboardContent prefers an image, then text, then nothing", async () => {
  const { clipboardContent } = load();
  const item = (parts) => ({
    types: Object.keys(parts),
    getType: async (t) => parts[t],
  });
  const png = { type: "image/png", size: 3 };
  const text = { text: async () => "hello" };
  // An iOS screenshot: image only — `readText()` would have resolved "".
  assert.deepEqual(await clipboardContent([item({ "image/png": png })]), { image: png });
  // Both on one item: the image wins, as in the keyboard paste event.
  assert.deepEqual(
    await clipboardContent([item({ "text/plain": text, "image/png": png })]),
    { image: png },
  );
  assert.deepEqual(await clipboardContent([item({ "text/plain": text })]), { text: "hello" });
  assert.deepEqual(await clipboardContent([item({ "text/html": text })]), { text: "" });
  assert.deepEqual(await clipboardContent([]), { text: "" });
});

// --- phoneBleed: when a maximized console folds the chrome away ------------

test("phoneBleed is maximize AND a phone-width viewport", () => {
  const { phoneBleed, PHONE_MAX_WIDTH } = load();
  // The breakpoint is the workbench's phone breakpoint: 01-base.css gates the
  // phone layout on the same number, so a drift between the two fails here.
  const css = readFileSync(join(UI, "styles/01-base.css"), "utf8");
  const phone = css.match(/@media \(max-width: (\d+)px\) \{/);
  assert.ok(phone, "01-base.css has a phone-width @media block");
  assert.equal(PHONE_MAX_WIDTH, Number(phone[1]));
  assert.equal(phoneBleed(true, 390), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH), true);
  assert.equal(phoneBleed(true, PHONE_MAX_WIDTH + 1), false);
  assert.equal(phoneBleed(true, 1280), false);
  // Not maximized: nothing folds, whatever the width.
  assert.equal(phoneBleed(false, 390), false);
  // A viewport that has never laid out reports NaN — no bleed, no throw.
  assert.equal(phoneBleed(true, NaN), false);
  assert.equal(phoneBleed(true, undefined), false);
});

// --- selectionRow: the buffer line under a finger --------------------------

test("selectionRow maps a touch to a buffer-absolute row", () => {
  const { selectionRow } = load();
  // Screen at y=100, 20px rows, 24 rows on screen, scrolled 50 lines in.
  assert.equal(selectionRow(100, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(119, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(120, 100, 20, 24, 50), 51);
  assert.equal(selectionRow(345, 100, 20, 24, 50), 62);
  // Unscrolled: the row IS the buffer line.
  assert.equal(selectionRow(140, 100, 20, 24, 0), 2);
});

test("selectionRow clamps to the screen", () => {
  const { selectionRow } = load();
  // Above the screen: the top row. Below it: the last row, never a line that
  // is not on screen.
  assert.equal(selectionRow(0, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(9999, 100, 20, 24, 50), 73);
  assert.equal(selectionRow(9999, 100, 20, 1, 0), 0);
});

test("selectionRow never answers NaN", () => {
  const { selectionRow } = load();
  // A terminal that has not laid out has a zero cell height; a missing
  // viewportY is 0. Either way the answer is a row `selectLines` accepts.
  assert.equal(selectionRow(140, 100, 0, 24, 50), 50);
  assert.equal(selectionRow(140, 100, NaN, 24, 50), 50);
  assert.equal(selectionRow(NaN, 100, 20, 24, 50), 50);
  assert.equal(selectionRow(140, 100, 20, 24, undefined), 2);
  assert.equal(selectionRow(140, undefined, 20, 24, 0), 7);
});

// --- stepFont: the A− / A+ range ------------------------------------------

test("stepFont walks one px at a time and stops at both ends", () => {
  const c = load();
  assert.equal(c.stepFont(15, 1), 16);
  assert.equal(c.stepFont(15, -1), 14);
  assert.equal(c.stepFont(c.FONT_MAX, 1), c.FONT_MAX);
  assert.equal(c.stepFont(c.FONT_MIN, -1), c.FONT_MIN);
  // Past the ends from outside the range — a store hand-edited before the
  // normalisation in wb-view.js was added.
  assert.equal(c.stepFont(400, 1), c.FONT_MAX);
  assert.equal(c.stepFont(1, -1), c.FONT_MIN);
});

test("stepFont starts from xterm's own default when nothing is stored", () => {
  const c = load();
  // `fontSize()` answers null-ish when the profile has no preference; stepping
  // from there must land next to the size the operator is actually looking at.
  assert.equal(c.stepFont(null, 1), c.FONT_DEFAULT + 1);
  assert.equal(c.stepFont(undefined, -1), c.FONT_DEFAULT - 1);
  assert.equal(c.stepFont(NaN, 1), c.FONT_DEFAULT + 1);
  // Always an integer: a half-px size is a blurred glyph grid.
  assert.equal(Number.isInteger(c.stepFont(15.4, 1)), true);
});

test("the font range holds xterm's default, so an unset preference changes nothing", () => {
  const c = load();
  assert.ok(c.FONT_MIN < c.FONT_DEFAULT && c.FONT_DEFAULT < c.FONT_MAX);
  // With no view store (the popup, and this harness) the size is the default.
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

test("a touch surface with no stored preference starts at the same size", () => {
  const coarse = (q) => ({ matches: q.includes("coarse"), addEventListener() {} });
  const c = load({ matchMedia: coarse });
  assert.equal(c.fontSize(), c.FONT_DEFAULT);
});

// --- touchGesture / touchCentroid: how many fingers, whose gesture ---------
test("touchGesture gives one finger to the terminal, two to the canvas, the rest to the system", () => {
  const { touchGesture } = load();
  // [case, finger count, maxlock, expected owner]
  const rows = [
    ["one finger", 1, false, "terminal"],
    ["two fingers", 2, false, "canvas"],
    // Under maxlock one finger stays the terminal's, and two go to nobody.
    ["one finger under maxlock", 1, true, "terminal"],
    ["two fingers under maxlock", 2, true, "none"],
    ["three fingers", 3, false, "none"],
    ["no fingers", 0, false, "none"],
    ["an unknown count", undefined, false, "none"],
  ];
  for (const [name, fingers, maxlock, want] of rows) {
    assert.equal(touchGesture(fingers, maxlock), want, name);
  }
});

test("touchCentroid is the point between the fingers", () => {
  const { touchCentroid } = load();
  assert.deepEqual(touchCentroid([{ clientX: 10, clientY: 20 }, { clientX: 30, clientY: 60 }]), { x: 20, y: 40 });
  assert.deepEqual(touchCentroid([{ clientX: 5, clientY: 5 }]), { x: 5, y: 5 });
  assert.deepEqual(touchCentroid([]), { x: 0, y: 0 });
  assert.deepEqual(touchCentroid(undefined), { x: 0, y: 0 });
});

// --- dragThreshold / dragBegins: a tap is not a drag ------------------------
test("dragThreshold is 4px for a mouse and 10px for a finger, a pen or an unknown pointer", () => {
  const { dragThreshold } = load();
  assert.equal(dragThreshold("mouse"), 4);
  assert.equal(dragThreshold("touch"), 10);
  assert.equal(dragThreshold("pen"), 10);
  assert.equal(dragThreshold(undefined), 10);
  assert.equal(dragThreshold(""), 10);
});

const BEGINS = [
  // [start, pointer, threshold, expected, why]
  [{ x: 0, y: 0 }, { x: 0, y: 0 }, 4, false, "no travel is a tap"],
  [{ x: 100, y: 100 }, { x: 103.99, y: 100 }, 4, false, "just under the mouse threshold"],
  [{ x: 100, y: 100 }, { x: 104, y: 100 }, 4, true, "exactly the mouse threshold arms"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 4, true, "the diagonal counts as 5, not 3 or 4"],
  [{ x: 0, y: 0 }, { x: 3, y: 4 }, 10, false, "5px is a finger's slip"],
  [{ x: 0, y: 0 }, { x: -6, y: 8 }, 10, true, "direction does not matter"],
  [{ x: 50, y: 50 }, { x: 50, y: 41 }, 10, false, "9px straight up is still a tap for a finger"],
];

for (const [start, pointer, threshold, want, why] of BEGINS) {
  test(`dragBegins: ${why}`, () => {
    const { dragBegins } = load();
    assert.equal(dragBegins(start, pointer, threshold), want);
  });
}

// --- isDoubleTap: two taps on a titlebar ----------------------------------
const TAPS = [
  // [prev, tap, expected, why]
  [null, { t: 100, x: 0, y: 0 }, false, "a first tap is not a double tap"],
  [{ t: 0, x: 50, y: 50 }, { t: 200, x: 52, y: 49 }, true, "a second tap close in time and place"],
  [{ t: 0, x: 50, y: 50 }, { t: 300, x: 50, y: 50 }, true, "exactly the time limit still counts"],
  [{ t: 0, x: 50, y: 50 }, { t: 301, x: 50, y: 50 }, false, "one ms past the time limit is a new first tap"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 6, y: 8 }, false, "10px away is another place"],
  [{ t: 0, x: 0, y: 0 }, { t: 100, x: 5, y: 8 }, true, "just under 10px is the same place"],
  [{ t: 500, x: 0, y: 0 }, { t: 400, x: 0, y: 0 }, false, "a tap from the future is not a pair"],
];

for (const [prev, tap, want, why] of TAPS) {
  test(`isDoubleTap: ${why}`, () => {
    const { isDoubleTap, DOUBLE_TAP_MS } = load();
    assert.equal(DOUBLE_TAP_MS, 300);
    assert.equal(isDoubleTap(prev, tap), want);
  });
}

// --- touchScrollTarget: whose gesture a finger's drag is -------------------
// The finger must be the trackpad, and xterm gives the trackpad's wheel to
// three different owners. Under a TUI that tracks the mouse the viewport's
// history is a heap of the app's stale frames — the "ghosts" an iPad showed.
test("touchScrollTarget moves the viewport only in the plain case", () => {
  const { touchScrollTarget } = load();
  // [case, mouse-tracking mode, buffer, expected owner]
  const rows = [
    // An app that is tracking the mouse gets the gesture, in either buffer.
    ...["x10", "vt200", "drag", "any"].flatMap((mode) => [
      [`${mode} tracking, normal buffer`, mode, "normal", "app"],
      [`${mode} tracking, alternate buffer`, mode, "alternate", "app"],
    ]),
    // The alternate buffer has no history, so the app gets it there too.
    ["no tracking, alternate buffer", "none", "alternate", "app"],
    ["no tracking, normal buffer", "none", "normal", "viewport"],
    ["an unknown mode, normal buffer", undefined, "normal", "viewport"],
    ["nothing known", null, undefined, "viewport"],
  ];
  for (const [name, mode, buffer, want] of rows) {
    assert.equal(touchScrollTarget(mode, buffer), want, name);
  }
});

// --- touchScrollLines: the gesture the console had to take back -----------
// A drag over a console used to pan the whole canvas: the touch lands on
// `.xterm-screen`, and the element that scrolls is its sibling, not its
// ancestor, so the browser walked up to `#workspace` (xterm.js #3613/#594).

test("touchScrollLines converts a drag into lines at the terminal's cell height", () => {
  const { touchScrollLines } = load();
  // Dragging the content DOWN moves the VIEW up, hence the sign flip.
  assert.equal(touchScrollLines(-34, 17), 2);
  assert.equal(touchScrollLines(34, 17), -2);
  // Fractional on purpose: a slow drag moves less than a row per event, and
  // truncating each one on its own rounds the whole gesture away to nothing.
  assert.equal(touchScrollLines(-8.5, 17), 0.5);
});

test("touchScrollLines refuses to divide by a cell height it does not have", () => {
  const { touchScrollLines } = load();
  // A terminal mid-teardown, or one that has never laid out, reports 0 —
  // and `-dy / 0` is Infinity, which `scrollLines` would take literally.
  assert.equal(touchScrollLines(-100, 0), 0);
  assert.equal(touchScrollLines(-100, -1), 0);
  assert.equal(touchScrollLines(-100, NaN), 0);
  assert.equal(touchScrollLines(NaN, 17), 0);
  assert.equal(touchScrollLines(undefined, 17), 0);
});

// --- flingStep: the glide that makes scrollback reachable by hand ---------

test("flingStep decays toward a stop and reports the distance for the frame", () => {
  const c = load();
  const first = c.flingStep(1, 16);
  assert.equal(first.dy, 16);
  // One frame of decay, not a fixed subtraction: a longer frame decays more.
  assert.ok(first.velocity < 1 && first.velocity > 0.9);
  assert.ok(c.flingStep(1, 32).velocity < first.velocity);
});

test("flingStep cuts the glide once it stops being motion", () => {
  const c = load();
  // Below the floor it is drift, not a fling — and a velocity that never
  // reaches zero is a requestAnimationFrame loop that never ends.
  assert.equal(c.flingStep(0.001, 16).velocity, 0);
  assert.equal(c.flingStep(0, 16).velocity, 0);
  // A long enough frame gap must also land on a stop rather than overshooting.
  assert.equal(c.flingStep(1, 100000).velocity, 0);
  // Garbage in never produces a moving glide.
  assert.deepEqual(c.flingStep(NaN, 16), { dy: 0, velocity: 0 });
  assert.deepEqual(c.flingStep(1, 0), { dy: 0, velocity: 0 });
});

test("a fling always terminates", () => {
  const c = load();
  let v = 5;
  let frames = 0;
  while (v !== 0 && frames < 10000) {
    v = c.flingStep(v, 16).velocity;
    frames += 1;
  }
  assert.equal(v, 0, "the glide must reach a stop");
  assert.ok(frames < 200, `and get there quickly, not in ${frames} frames`);
});

// --- fullscreenOffered: where the fullscreen button is worth building -----
// Two independent reasons to withhold it, and the table keeps them separable:
// no API at all (sandboxed frame, standalone PWA), and an API that WebKit hands
// back the moment a text field takes focus.
test("fullscreenOffered builds the button only where the engine can hold it", () => {
  const { fullscreenOffered } = load();
  // [case, fullscreen API present, vendor, expected]
  const rows = [
    ["no API", false, "Google Inc.", false],
    ["an unknown API", undefined, "Google Inc.", false],
    ["no API and no vendor", null, "", false],
    // WebKit hands fullscreen back the moment the keyboard takes focus.
    ["WebKit", true, "Apple Computer, Inc.", false],
    ["Chromium", true, "Google Inc.", true],
    ["an empty vendor", true, "", true],
  ];
  for (const [name, api, vendor, want] of rows) {
    assert.equal(fullscreenOffered(api, vendor), want, name);
  }
});

test("isWebKit is the one engine question both decisions ask", () => {
  const { isWebKit } = load();
  assert.equal(isWebKit("Apple Computer, Inc."), true);
  assert.equal(isWebKit("Google Inc."), false);
  assert.equal(isWebKit(""), false);
  assert.equal(isWebKit(undefined), false);
});

// --- prefersDomRenderer: which engines must not get the GPU renderer ------
// The WebGL addon draws scrolled rows twice on WebKit — reported from an iPad
// as the text "distorting", and reproducible by dragging the scrollbar, a path
// this module does not touch. Upstream has carried it for years (xterm.js
// #3357, #5816) and the standing answer is to not use the addon there.

test("prefersDomRenderer asks about the ENGINE, not the brand", () => {
  const { prefersDomRenderer } = load();
  // [case, vendor, expected]
  const rows = [
    // Safari, and every other browser on iPadOS — all WebKit underneath, all
    // reporting the same vendor. That is exactly why the vendor is the question.
    ["WebKit", "Apple Computer, Inc.", true],
    ["Chromium", "Google Inc.", false],
    ["Firefox, which reports an empty vendor", "", false],
    // A browser that reports nothing at all keeps the faster renderer: the DOM
    // fallback is the safe answer for a KNOWN-bad engine, not a default.
    ["no vendor", undefined, false],
    ["a null vendor", null, false],
    ["a vendor that is not a string", 42, false],
  ];
  for (const [name, vendor, want] of rows) {
    assert.equal(prefersDomRenderer(vendor), want, name);
  }
});

// --- gpuHolders: which windows hold one of the page's WebGL contexts ---------
// Chrome keeps 16 live contexts per renderer process and drops the oldest past
// that, so a cascade of 20 seen consoles lost four. The page hands out a
// budget, to the seen, uncovered windows on top.

test("gpuHolders gives the budget to the seen, uncovered windows on top", () => {
  const { gpuHolders } = load();
  const win = (z, more = {}) => ({ seen: true, covered: false, hasTerminal: true, z, ...more });
  // [case, windows, budget, expected indexes]
  const rows = [
    ["the highest z first, cut at the budget", [win(1), win(5), win(3), win(4)], 2, [1, 3]],
    ["fewer candidates than the budget: all of them", [win(1), win(2)], 12, [1, 0]],
    [
      "a window not seen, covered or without a terminal never holds one, even on top",
      [win(9, { seen: false }), win(8, { covered: true }), win(7, { hasTerminal: false }), win(1)],
      2,
      [3],
    ],
    ["ties keep the input order", [win(2), win(2), win(2)], 2, [0, 1]],
  ];
  for (const [name, windows, budget, want] of rows) {
    assert.deepEqual(gpuHolders(windows, budget), want, name);
  }
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
    classList: { contains: (c) => (c === "maximized" && maximized) || classes.includes(c) },
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

test("restoreRect reads a measurable window from the DOM", () => {
  const { restoreRect } = load();
  // The inline rect is stale on purpose: after a drag the DOM is the truth.
  const win = fakeWin({ offsets: REAL, inline: { left: "1px", top: "1px", width: "1px", height: "1px" } });
  assert.deepEqual(restoreRect(win), REAL);
});

test("restoreRect keeps a hidden window's box instead of storing zeros", () => {
  const { restoreRect } = load();
  const win = fakeWin({ offsets: HIDDEN, inline: INLINE });
  assert.deepEqual(restoreRect(win), REAL);
});

test("restoreRect on a hidden window honours a real 0 in the inline rect", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    offsets: HIDDEN,
    inline: { left: "0px", top: "0px", width: "560px", height: "340px" },
  });
  assert.deepEqual(restoreRect(win), { left: 0, top: 0, width: 560, height: 340 });
});

test("restoreRect on a maximized window still reads the pre-maximize inline rect", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    maximized: true,
    offsets: { left: 0, top: 0, width: 1440, height: 900 },
    inline: INLINE,
  });
  assert.deepEqual(restoreRect(win), REAL);
});

// --- columns: only the leftmost is the maximized console the desk records ----
// `wb-console.js` never loads `wb-columns.ts` (the popup boots without it), so
// the harness runs the REAL fold beside it, as `app.ts` does in the browser.
function loadColumns() {
  return WBColumns;
}

// `applyColumns` maximizes only where `columnClasses(...).maximized` is true,
// and with `persist` that maximize is written to the desk.
// NEGATIVE CONTROL: answering `maximized: true` for every entry fails the "b"
// assertion below — that is two consoles recorded as maximized.
test("columnClasses never marks a column right of the leftmost maximized", () => {
  const { columnClasses } = load();
  const C = loadColumns();
  const p = C.painted([["a"], ["b"], ["c"]], 3);
  assert.deepEqual(columnClasses(p, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(p, "b"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "c"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "x"), { column: false, maximized: null });
});

test("restoring the leftmost column promotes the next to the maximize the desk records", () => {
  const { columnClasses } = load();
  const C = loadColumns();
  const r = C.restore([["a"], ["b"], ["c"]], "a");
  assert.equal(r.unmax, "a");
  assert.equal(columnClasses(C.painted(r.columns, 3), "b").maximized, true);
  assert.equal(columnClasses(C.painted(r.columns, 3), "c").maximized, false);
  assert.equal(columnClasses(C.painted(r.columns, 3), "a").maximized, null);
  // The last column left is an ordinary maximized console again.
  const last = C.restore([["a"], ["b"]], "a");
  assert.deepEqual(columnClasses(C.painted(last.columns, 2), "b"), {
    column: false,
    maximized: true,
  });
});

// Rows (ADR-0051 §5): only the top row of the leftmost column is the
// maximized console, and one column of two rows is already columns.
// NEGATIVE CONTROL: dropping the row check from `maximized` marks "b" too;
// counting columns instead of painted consoles leaves "a" and "b" plain.
test("columnClasses marks only the top row of the leftmost column maximized", () => {
  const { columnClasses } = load();
  const C = loadColumns();
  const p = C.painted([["a", "b"], ["c"]], Infinity);
  assert.deepEqual(columnClasses(p, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(p, "b"), { column: true, maximized: false });
  assert.deepEqual(columnClasses(p, "c"), { column: true, maximized: false });
  const one = C.painted([["a", "b"]], Infinity);
  assert.deepEqual(columnClasses(one, "a"), { column: true, maximized: true });
  assert.deepEqual(columnClasses(one, "b"), { column: true, maximized: false });
});

// A column right of the leftmost is not `.maximized` (ADR-0051 §5), yet its
// painted box is view state: reading it would write the column onto the desk.
test("restoreRect on a column reads the inline rect, not the painted column box", () => {
  const { restoreRect } = load();
  const win = fakeWin({
    classes: ["column"],
    offsets: { left: 960, top: 0, width: 960, height: 1000 },
    inline: INLINE,
  });
  assert.deepEqual(restoreRect(win), REAL);
});

// --- the desk changes (ADR-0050 amendment 2026-10-04, changes, not the desk) --
// A page sends only the fields each act changed. These tests load the module
// over a fake daemon: the GET serves `served`, and the sink records every
// body it is handed and answers `ok` with no desk.

async function deskPage(served = {}, extras = {}, docExtras = {}) {
  const realFetch = globalThis.fetch;
  const gets = [];
  globalThis.fetch = async (url) => {
    gets.push(String(url));
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [], ...served }) };
  };
  const sent = [];
  const answers = extras.answers || [];
  const wb = load(
    {
      WBConsoleOpts: {
        deskSink: {
          put(body) {
            sent.push(JSON.parse(body));
            return Promise.resolve(answers.shift() || { kind: "ok", reply: null });
          },
          putSync(body) {
            sent.push({ closing: true, ...JSON.parse(body) });
          },
        },
      },
      ...extras.window,
    },
    { getElementById: () => null, ...docExtras },
  );
  await wb.whenDeskLoaded();
  return { wb, sent, gets, restore: () => (globalThis.fetch = realFetch) };
}
const settle = (ms = 400) => new Promise((r) => setTimeout(r, ms));
const changesOf = (sent) => sent.flatMap((b) => b.changes);

// The selected checkout per project (ADR-0063 §4) is the fourth record type.
test("a checkout pick and a clear are two changes, and the clear names the tree it clears", async () => {
  const page = await deskPage({ checkouts: { "o/r": "wt" } });
  try {
    const { wb, sent } = page;
    assert.equal(wb.checkoutOf("o/r"), "wt");
    assert.deepEqual(wb.checkouts(), { "o/r": "wt" });
    // A copy, never the view itself.
    wb.checkouts()["o/r"] = "tampered";
    assert.equal(wb.checkoutOf("o/r"), "wt");

    wb.setCheckout("o/r", null);
    assert.equal(wb.checkoutOf("o/r"), null);
    wb.setCheckout("o/s", null); // nothing selected there: nothing to say
    wb.setCheckout("o/r", "wt-b");
    assert.equal(wb.checkoutOf("o/r"), "wt-b");
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "checkout-clear", repo: "o/r", ifName: "wt" },
      { op: "checkout", repo: "o/r", name: "wt-b" },
    ]);
  } finally {
    page.restore();
  }
});

test("a desk from an older daemon has no checkouts and reads as none", async () => {
  const page = await deskPage({ checkouts: undefined });
  try {
    assert.equal(page.wb.checkoutOf("o/r"), null);
    assert.deepEqual(page.wb.checkouts(), {});
  } finally {
    page.restore();
  }
});

// The measured loss of 2026-10-04 came from a page that wrote what it had not
// changed. A flush reads nothing first and carries no record it was not told.
test("a flush sends only this page's changes: no read before it, no whole desk", async () => {
  const theirs = { id: "theirs", repo: "o/r", agent: "console", kind: "console", rect: { left: 1, top: 1, width: 300, height: 200 }, sessionId: 4, consoleName: "r #1" };
  const page = await deskPage({ rev: 3, generation: 7, windows: [theirs] });
  try {
    page.wb.setCheckout("o/r", "wt");
    await settle();
    assert.equal(page.gets.length, 1, `only the load read the desk: ${page.gets}`);
    assert.equal(page.sent.length, 1);
    assert.deepEqual(Object.keys(page.sent[0]).sort(), ["changes", "generation", "seq"]);
    assert.equal(page.sent[0].generation, 7);
    assert.ok(!JSON.stringify(page.sent).includes("theirs"), JSON.stringify(page.sent));
  } finally {
    page.restore();
  }
});

const RECT = { left: 80, top: 120, width: 240, height: 180 };
const card = (id, path, extra = {}) => ({ id, repo: "o/r", path, rect: RECT, ...extra });

test("saveNotes sends a create, the fields that changed, and a remove", async () => {
  const page = await deskPage({ notes: [card("n1", "a.note"), card("n2", "b.note")] });
  try {
    const { wb, sent } = page;
    // A move stamps nothing: a `ts` handed back is not a field.
    wb.saveNotes(wb.notes().map((n) => (n.id === "n1" ? { ...n, rect: { ...RECT, left: 500 }, ts: 99 } : n)));
    wb.saveNotes(wb.notes().filter((n) => n.id !== "n2"));
    wb.saveNotes(wb.notes().concat([card("n3", "")]));
    wb.saveNotes(wb.notes().map((n) => (n.id === "n3" ? { ...n, path: "c.note" } : n)));
    wb.saveNotes(wb.notes());
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "note", id: "n1", fields: { rect: { ...RECT, left: 500 } } },
      { op: "remove", type: "note", id: "n2" },
      { op: "create", type: "note", record: card("n3", "") },
      { op: "set", type: "note", id: "n3", fields: { file: { repo: "o/r", path: "c.note", checkout: null } } },
    ]);
    // A copy: mutating what was handed out must not reach the view.
    const kept = wb.notes();
    kept.pop();
    assert.equal(wb.notes().length, 2);
  } finally {
    page.restore();
  }
});

test("a fence rename sends its name, and a fence remove sends the remove", async () => {
  const fence = { id: "f1", name: "Fence 1", rect: { left: 0, top: 0, width: 400, height: 300 } };
  const page = await deskPage({ fences: [fence] });
  try {
    page.wb.renameFence("f1", "backend");
    page.wb.removeFence("f1");
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "fence", id: "f1", fields: { name: "backend" } },
      { op: "remove", type: "fence", id: "f1" },
    ]);
  } finally {
    page.restore();
  }
});

// A window the module writes without a gesture: a fake with the fields the
// writes read.
function deskWin(id, extra = {}) {
  return {
    _deskId: id,
    _deskRepo: "o/r",
    _deskAgent: "console",
    _deskKind: "console",
    _deskDaemonId: null,
    _deskEnvironment: null,
    _deskCheckout: null,
    _deskLocked: false,
    _deskConsoleName: "r #1",
    _deskUnrecorded: false,
    isConnected: true,
    classList: { contains: () => false },
    offsetLeft: 30,
    offsetTop: 40,
    offsetWidth: 640,
    offsetHeight: 420,
    style: {},
    ...extra,
  };
}
const SAVED = { id: "w-1", repo: "o/r", agent: "console", kind: "console", rect: { left: 100, top: 100, width: 600, height: 400 }, max: false, sessionId: 7, checkout: "wt-a", consoleName: "r #1" };

// The DOM that `applyColumns` paints: a stage whose windows match the few
// selectors it queries, and a workspace with no scroll. The stage measures 0
// wide, so the fence refresh after a write returns early.
function columnDom(wins) {
  const match = (w, sel) =>
    sel.split(",").some((one) =>
      one.trim().split(".").filter(Boolean).every((c) => w.classList.contains(c)),
    );
  const all = (sel) => wins.filter((w) => match(w, sel));
  const one = (sel) => all(sel)[0] ?? null;
  const stageEl = { offsetWidth: 0, offsetHeight: 0, style: {}, querySelectorAll: all, querySelector: one };
  const workspaceEl = {
    scrollLeft: 0,
    scrollTop: 0,
    classList: { toggle() {} },
    querySelectorAll: all,
    querySelector: one,
  };
  return { stage: stageEl, workspace: workspaceEl };
}
function columnWin(id, maximized = false) {
  const classes = new Set(maximized ? ["session-window", "maximized"] : ["session-window"]);
  return deskWin(id, {
    classList: {
      contains: (c) => classes.has(c),
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      toggle: (c, on) => (on ? classes.add(c) : classes.delete(c), on),
    },
    style: { setProperty() {}, removeProperty() {} },
  });
}
const colRecord = (id, max) => ({ ...SAVED, id, max, sessionId: null, checkout: null });
const byId = (changes) => [...changes].sort((x, y) => x.id.localeCompare(y.id));

// ADR-0051 §5: the console that becomes first in the columns is written to
// the desk as an ordinary maximize, and the one that stops being first is
// written as not maximized. `WBColumns.fromStored` keeps the stored grid only
// when the desk records its first console as maximized.
test("applyColumns writes the moved maximize to the desk when asked to persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
  }
});

// The old maximized console can stay in the grid, not first: a fence opened as
// columns whose leftmost member is another console. No `unmax` names it.
test("applyColumns writes the unmaximize of a console that stays a column, not first", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    page.wb.applyColumns(C.painted([["w-b"], ["w-a"]], 2), { cap: 2, unmax: null, persist: true });
    await settle();
    assert.deepEqual(byId(changesOf(page.sent)), [
      { op: "set", type: "window", id: "w-a", fields: { max: false } },
      { op: "set", type: "window", id: "w-b", fields: { max: true } },
    ]);
  } finally {
    page.restore();
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
  assert.throws(() => markRelaunch("w-b", () => { throw new Error("gone"); }), /gone/);
  assert.equal(isRelaunching("w-b"), false, "a take-down that throws clears the mark");
});

// A relaunch takes the window off the stage and spawns a new one under the
// same id; the shell's columns wait for an id marked in between. These paths
// need a real DOM, so the wiring is pinned on the source.
test("a restart and a placeholder's Launch mark the console as relaunching", () => {
  assert.match(SRC, /markRelaunch\(carry\.id, discard\);/);
  assert.match(SRC, /markRelaunch\(carry\.id, \(\) => drop\(true\)\);/);
  assert.match(SRC, /\} finally \{\s*relaunching\.delete\(carry\?\.id\);/);
});

// The torn-off fence window's grid is never stored (ADR-0051 §8, amended
// 2026-10-05), so its one call never asks to write the maximize.
test("the torn-off fence window paints its columns without persist", () => {
  // The page and its page script module.
  const html =
    readFileSync(join(UI, "detached-fence.html"), "utf8") + readFileSync(join(UI, "wb-detached-fence.ts"), "utf8");
  const calls = html.match(/WBConsole\.applyColumns\([^;]*;/g) || [];
  assert.equal(calls.length, 1, calls.join("\n"));
  assert.ok(!/\bpersist\b/.test(html), "the torn-off fence window names persist");
});

// The torn-off fence window paints its own grid and never stores it
// (ADR-0051 §8, amended 2026-10-05), so its maximize is not written.
test("applyColumns writes nothing to the desk without persist", async () => {
  let dom = null;
  const page = await deskPage(
    { windows: [colRecord("w-a", true), colRecord("w-b", false)] },
    {},
    { getElementById: (id) => dom?.[id] ?? null, dispatchEvent() {} },
  );
  try {
    const C = loadColumns();
    const a = columnWin("w-a", true);
    const b = columnWin("w-b");
    dom = columnDom([a, b]);
    const r = C.restore([["w-a"], ["w-b"]], "w-a");
    page.wb.applyColumns(C.painted(r.columns, 2), { cap: 2, unmax: r.unmax });
    await settle();
    assert.ok(b.classList.contains("maximized"), "the page still shows the move");
    assert.deepEqual(page.sent, []);
  } finally {
    page.restore();
  }
});

// Seven of the eleven old window writes were not about the rect, and each
// wrote the rect anyway. A reconnect writes the session, only when it moved.
test("a reconnect writes the session only when it differs, and never the rect", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const win = deskWin("w-1", { _term: { sessionId: 7 }, _deskCheckout: "wt-a" });
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(page.sent, [], "the same session writes nothing");
    win._term.sessionId = 8;
    page.wb.recordSession(win);
    await settle();
    assert.deepEqual(changesOf(page.sent), [
      { op: "set", type: "window", id: "w-1", fields: { session: { sessionId: 8, daemonId: null, environment: null } } },
    ]);
  } finally {
    page.restore();
  }
});

test("a birth writes only what it changed; an adopted console waits for the operator's first act", async () => {
  const page = await deskPage({ windows: [SAVED] });
  try {
    const { wb, sent } = page;
    // A relaunch in the primary tree: the record said `wt-a`.
    wb.recordBirth(deskWin("w-1"), SAVED);
    // An adopted console: its record is another page's to write.
    const adopted = deskWin("w-adopted");
    wb.recordBirth(adopted, { unrecorded: true });
    // A new console: its record is created.
    wb.recordBirth(deskWin("w-new"), undefined);
    await settle();
    assert.deepEqual(changesOf(sent), [
      { op: "set", type: "window", id: "w-1", fields: { checkout: null } },
      {
        op: "create",
        type: "window",
        record: {
          id: "w-new", repo: "o/r", agent: "console", kind: "console",
          rect: { left: 30, top: 40, width: 640, height: 420 }, max: false, sessionId: null,
          daemonId: null, environment: null, checkout: null, locked: false, consoleName: "r #1",
        },
      },
    ]);
    assert.equal(adopted._deskUnrecorded, true);
    // The operator's first act on it: the record, then the act, in one batch.
    sent.length = 0;
    wb.setWin(adopted, { locked: true });
    await settle();
    assert.equal(sent.length, 1, "one batch");
    assert.deepEqual(
      sent[0].changes.map((c) => [c.op, c.type, c.record?.id ?? c.id]),
      [["create", "window", "w-adopted"], ["set", "window", "w-adopted"]],
    );
    assert.equal(adopted._deskUnrecorded, false);
  } finally {
    page.restore();
  }
});

// A failure the daemon may still accept keeps the batch, resent with its
// `seq`; a 400 drops it; a restore reloads the page.
test("a flush keeps a batch the network lost, drops one the daemon calls malformed, and reloads on a restore", async () => {
  let reloads = 0;
  const page = await deskPage(
    {},
    {
      answers: [{ kind: "network" }, { kind: "ok", reply: null }, { kind: "refused", status: 400, reply: { error: "x" } }, { kind: "refused", status: 409, reply: { state: "restored" } }],
      window: { location: { reload: () => reloads++ } },
    },
  );
  try {
    const { wb, sent } = page;
    wb.setCheckout("o/r", "a");
    await settle(1600); // the first retry waits one second
    assert.equal(sent.length, 2);
    assert.deepEqual(sent[1], sent[0], "the resend is the same batch, with the same seq");
    wb.setCheckout("o/r", "b");
    await settle();
    assert.equal(sent[2].seq, sent[0].seq + 1);
    wb.setCheckout("o/r", "c");
    await settle();
    assert.deepEqual(changesOf([sent[3]]), [{ op: "checkout", repo: "o/r", name: "c" }], "the dropped change is gone");
    assert.equal(reloads, 1, "a restore reloads the page");
    wb.setCheckout("o/r", "d");
    await settle();
    assert.equal(sent.length, 4, "a page that reloads sends nothing more");
  } finally {
    page.restore();
  }
});

// A live session no record claims is that console's own placeholder come back
// to life (its `sessionId` lost to a lost flush, or reissued by a restarted
// daemon), and it attaches THERE — not into a fresh cascaded record next to
// it. A shell record would otherwise `relaunch` a SECOND PTY beside the one
// still running. Only with no waiting record on the same repo, vendor, kind
// and worktree is it adopted.
test("reconcileDesk attaches an unclaimed session to its waiting record before adopting", () => {
  const wb = load();
  const agent = { repo: "owner/repo", agent: "claude", kind: "agent" };
  const out = wb.reconcileDesk({
    layout: [
      { id: "wt", ...agent, checkout: "wt-a", sessionId: null },
      { id: "primary", ...agent, sessionId: null },
      { id: "shell", repo: "owner/repo", agent: "console", kind: "console", sessionId: 1 },
    ],
    sessions: [
      { id: 5, ...agent, checkout: null },
      { id: 6, repo: "owner/repo", agent: "console", kind: "console" },
      { id: 7, repo: "owner/other", agent: "codex", kind: "agent" },
    ],
  });
  assert.deepEqual(
    out.map(({ record, session, action }) => [record?.id ?? null, session?.id ?? null, action]),
    [
      ["wt", null, "placeholder"],
      ["primary", 5, "attach"],
      ["shell", 6, "attach"],
      [null, 7, "adopt"],
    ],
  );
});

// Two sessions no record claims: the second one's search for a waiting record
// passes the first one's `adopt` entry, which has no record.
test("reconcileDesk adopts every unclaimed session, not only the first", () => {
  const wb = load();
  const out = wb.reconcileDesk({
    layout: [],
    sessions: [
      { id: 1, repo: "owner/repo", agent: "claude", kind: "agent" },
      { id: 2, repo: "owner/repo", agent: "codex", kind: "agent" },
    ],
  });
  assert.deepEqual(
    out.map(({ session, action }) => [session.id, action]),
    [
      [1, "adopt"],
      [2, "adopt"],
    ],
  );
});

// A placeholder on a page loaded before another device started its console:
// the fresh desk and session list decide whether Relaunch attaches instead of
// launching a second vendor CLI.
test("placeholderSession finds the session this record owns by now, and no other", () => {
  const wb = load();
  const agent = { repo: "owner/repo", agent: "claude", kind: "agent" };
  const rec = (id, sessionId) => ({ id, ...agent, sessionId });
  const live = (id) => ({ id, ...agent, checkout: null });
  const rows = [
    // [what, layout, sessions, held, want]
    ["another device relaunched the record", [rec("ph", 8)], [live(8)], [], 8],
    ["a stale id, one unclaimed session on the same tuple", [rec("ph", 3)], [live(8)], [], 8],
    [
      "the only match is already shown on this page",
      [rec("ph", 3)],
      [live(8)],
      [{ id: 8, repo: "owner/repo" }],
      null,
    ],
    [
      "the other console claims its own session by id",
      [rec("other", 8), rec("ph", 3)],
      [live(8)],
      [],
      null,
    ],
    ["no live session", [rec("ph", 3)], [], [], null],
    [
      "held ids on another repo do not hide this one",
      [rec("ph", 3)],
      [live(8)],
      [{ id: 8, repo: "owner/other" }],
      8,
    ],
  ];
  for (const [what, layout, sessions, held, want] of rows) {
    const got = wb.placeholderSession({ layout, sessions, recordId: "ph", held });
    assert.equal(got?.id ?? null, want, what);
  }
});

// --- barKey: the key bar's Shift latch -------------------------------------

test("barKey: Shift toggles the latch and one key that sends bytes uses it", () => {
  const { barKey } = load();
  assert.deepEqual(barKey("shift", false, false), { seq: "", latched: true });
  assert.deepEqual(barKey("shift", false, true), { seq: "", latched: false });
  assert.deepEqual(barKey("tab", false, true), { seq: "\x1b[Z", latched: false });
  assert.deepEqual(barKey("up", true, true), { seq: "\x1b[1;2A", latched: false });
  // Esc, Enter and / have no Shift form, but they still use the latch up.
  assert.deepEqual(barKey("esc", false, true), { seq: "\x1b", latched: false });
  assert.deepEqual(barKey("enter", false, true), { seq: "\r", latched: false });
  assert.deepEqual(barKey("slash", false, true), { seq: "/", latched: false });
  // A key that sends nothing leaves the latch as it was.
  assert.deepEqual(barKey("nope", false, true), { seq: "", latched: true });
  assert.deepEqual(barKey("tab", false, false), { seq: "\t", latched: false });
});

// The shell records a name the popup reports only for a note that popup holds
// and that has no name yet (#475): the popup cannot write the desk itself.
test("a popup's note-name report is checked before the shell records it", () => {
  const wb = load();
  const entry = { members: [{ id: "w-1" }, { id: "n-1", kind: "note" }] };
  const record = { id: "n-1" };
  const msg = { noteId: "n-1", path: ".ralphy/notes/a.note" };
  assert.equal(wb.noteNameOk(entry, record, msg), true);
  // Not a note this popup holds: a console id, an unknown id, no entry.
  assert.equal(wb.noteNameOk(entry, { id: "w-1" }, { ...msg, noteId: "w-1" }), false);
  assert.equal(wb.noteNameOk(entry, { id: "x" }, { ...msg, noteId: "x" }), false);
  assert.equal(wb.noteNameOk(undefined, record, msg), false);
  // A name is given once.
  assert.equal(wb.noteNameOk(entry, { ...record, path: "b.note" }, msg), false);
  assert.equal(wb.noteNameOk(entry, null, msg), false);
  // The path must name a note, inside the repo.
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "a.md" }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "../a.note" }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: 42 }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "/etc/a.note" }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "\\\\host\\a.note" }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "C:\\a.note" }), false);
  assert.equal(wb.noteNameOk(entry, record, { ...msg, path: "c:a.note" }), false);
});

// A popup that is closing still talks on the channel, and a newer popup of
// the same fence may already be open (#476). Only the popup the entry holds
// is heard.
test("a lifecycle message is heard only from the popup the entry holds", () => {
  const wb = load();
  assert.equal(wb.popupMatches(undefined, { pid: "a" }), false);
  // Restored after a reload: no `pid` yet, so any popup of the fence is heard.
  assert.equal(wb.popupMatches({ pid: null }, { pid: "a" }), true);
  assert.equal(wb.popupMatches({}, {}), true);
  assert.equal(wb.popupMatches({ pid: "b" }, { pid: "b" }), true);
  assert.equal(wb.popupMatches({ pid: "b" }, { pid: "a" }), false);
  assert.equal(wb.popupMatches({ pid: "b" }, {}), false);
});

// The folds and the geometry are pure: none mutates what it is given.
test("the pure folds mutate none of their arguments", () => {
  const WB = load();
  const detach = (event) => [
    `detachFold on ${event.type}`,
    [["f-a", "f-b"], event],
    (reg, e) => WB.detachFold(reg, e),
    // A NEW array comes back, never the input.
    (out, [reg]) => assert.notEqual(out.registry, reg, `detachFold on ${event.type} returns a new array`),
  ];
  const peer = (event) => [
    `peerFold on ${event.type}`,
    [{ seen: SEEN, lost: false }, event, WINDOW_MS],
    (...args) => WB.peerFold(...args),
  ];
  // [case, arguments, call, extra check on the result]
  const rows = [
    [
      "anchorIntoView",
      [{ left: 600, top: 400, width: 200, height: 100 }, { width: 1000, height: 700 }, EXT],
      (...args) => WB.anchorIntoView(...args),
    ],
    [
      "panNudge",
      [{ x: 1076, y: 400 }, { left: 100, top: 50, right: 1100, bottom: 850 }, BAND, STEP],
      (...args) => WB.panNudge(...args),
    ],
    [
      "bringIntoView",
      [
        { left: 1200, top: 400, width: 200, height: 100 },
        { width: 1000, height: 700 },
        { width: 2000, height: 1500 },
      ],
      (...args) => WB.bringIntoView(...args),
    ],
    ["fenceCycle", [structuredClone(GRID), "a", 1], (...args) => WB.fenceCycle(...args)],
    detach({ type: "detach", fenceId: "f-new" }),
    detach({ type: "detach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-a" }),
    detach({ type: "reattach", fenceId: "f-nope" }),
    detach({ type: "focus", fenceId: "f-a" }),
    detach({ type: "wat", fenceId: "f-a" }),
    peer({ type: "beat", at: SEEN + 10 }),
    peer({ type: "tick", at: SEEN + WINDOW_MS + 1 }),
    peer({ type: "gone" }),
    ["dragBegins", [{ x: 1, y: 2 }, { x: 30, y: 40 }, 10], (...args) => WB.dragBegins(...args)],
    [
      "isDoubleTap",
      [{ t: 0, x: 1, y: 2 }, { t: 100, x: 3, y: 4 }],
      (...args) => WB.isDoubleTap(...args),
    ],
  ];
  for (const [name, args, call, check] of rows) {
    const before = structuredClone(args);
    const out = call(...args);
    assert.deepEqual(args, before, `${name} mutated its arguments`);
    if (check) check(out, args);
  }
});

// ADR-0070 D4: a desk the daemon cannot read is a failure with a reason, and
// this page never uploads over it.
test("an unreadable desk is a failure, and no flush PUTs over it", async () => {
  const calls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    return {
      ok: false,
      status: 409,
      json: async () => ({ state: "unreadable", error: "parsing desk layout C:/x/desk.toml" }),
    };
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "the file is damaged");
    c.setCheckout("o/r", "wt-a");
    await new Promise((r) => setTimeout(r, 400));
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    calls.filter((c) => c.method === "PUT"),
    [],
    "no PUT over a desk the daemon cannot read",
  );
});

// A desk that becomes unreadable after a good load: the daemon refuses the
// PUT, the page shows the failure and sends nothing more.
test("a PUT the daemon refuses as unreadable shows the failure and stops the sending", async () => {
  const calls = [];
  let unreadable = false;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    calls.push({ url: String(url), method: init.method || "GET" });
    if (unreadable) {
      return { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "parsing desk layout" }) };
    }
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [] }) };
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "", "the first read was good");
    unreadable = true;
    calls.length = 0;
    c.setCheckout("o/r", "wt-a");
    await new Promise((r) => setTimeout(r, 400));
    assert.equal(c.deskFailure(), "the file is damaged");
    c.setCheckout("o/r", "wt-b");
    await new Promise((r) => setTimeout(r, 400));
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(
    calls.filter((c) => c.method === "PUT").length,
    1,
    "one PUT, refused, and nothing after it",
  );
});

// Another tab may start the new desk first: the daemon then answers 409
// "readable", and this tab reads the desk like any other.
test("start a new desk treats a desk another tab already started as done", async () => {
  let started = false;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) => {
    if (init.method === "POST") {
      started = true;
      return { ok: false, status: 409, json: async () => ({ state: "readable" }) };
    }
    if (!started) {
      return { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "parsing desk layout" }) };
    }
    return { ok: true, status: 200, json: async () => ({ windows: [], fences: [], notes: [] }) };
  };
  try {
    // The empty new desk is restored, which looks for the stage.
    const c = load({}, { getElementById: () => null });
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "the file is damaged");
    await c.startNewDesk();
    assert.equal(c.deskFailure(), "", "the desk is readable now");
  } finally {
    globalThis.fetch = realFetch;
  }
});

// NEGATIVE CONTROL: any other refusal is a failure that names the status.
test("start a new desk fails with the status on any other refusal", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url, init = {}) =>
    init.method === "POST"
      ? { ok: false, status: 500, json: async () => ({}) }
      : { ok: false, status: 409, json: async () => ({ state: "unreadable", error: "x" }) };
  try {
    const c = load();
    await c.whenDeskLoaded();
    await assert.rejects(c.startNewDesk(), /the daemon answered 500/);
  } finally {
    globalThis.fetch = realFetch;
  }
});

// NEGATIVE CONTROL: a transport failure is not a broken desk, so it offers no
// new desk.
test("a desk read that fails in transport sets no desk failure", async () => {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    throw new Error("offline");
  };
  try {
    const c = load();
    await c.whenDeskLoaded();
    assert.equal(c.deskFailure(), "");
  } finally {
    globalThis.fetch = realFetch;
  }
});

// --- one session per window record (ADR-0050 amendment 2026-10-04) -----------

// The session list says which record each session serves. That beats the
// `sessionId` tuple: two shells on one repo whose ids are stale would
// otherwise be handed out in layout order, each to the other's window.
test("reconcileDesk attaches each session to the record it names", () => {
  const wb = load();
  const shell = { repo: "owner/repo", agent: "console", kind: "console" };
  const out = wb.reconcileDesk({
    layout: [
      { id: "w-a", ...shell, sessionId: 9 },
      { id: "w-b", ...shell, sessionId: null },
    ],
    sessions: [
      { id: 5, ...shell, record: "w-b" },
      { id: 6, ...shell, record: "w-a" },
    ],
  });
  assert.deepEqual(
    out.map(({ record, session, action }) => [record?.id ?? null, session?.id ?? null, action]),
    [
      ["w-a", 6, "attach"],
      ["w-b", 5, "attach"],
    ],
  );
});

// A session whose record this page has not read yet comes back under THAT id,
// so the page that launched it and the page that adopted it write one record.
test("reconcileDesk adopts a session under the record it names", () => {
  const wb = load();
  const shell = { repo: "owner/repo", agent: "console", kind: "console" };
  const out = wb.reconcileDesk({
    layout: [],
    sessions: [
      { id: 5, ...shell, record: "w-new" },
      { id: 6, ...shell, record: "w-new" },
      { id: 7, ...shell },
    ],
  });
  assert.deepEqual(
    out.map(({ session, action, id }) => [session.id, action, id]),
    [
      [5, "adopt", "w-new"],
      // A second claim on the same id would be two windows under one record.
      [6, "adopt", null],
      [7, "adopt", null],
    ],
  );
});

// A full desk refuses a NEW console instead of cutting one in silence.
test("atDeskCap is true once the desk holds the cap", async () => {
  const realFetch = globalThis.fetch;
  const desk = (n) => ({
    windows: Array.from({ length: n }, (_, i) => ({
      id: `w${i}`,
      repo: "o/r",
      agent: "console",
      kind: "console",
      consoleName: `r #${i + 1}`,
      ts: i,
    })),
    fences: [],
  });
  try {
    globalThis.fetch = async () => ({ ok: true, json: async () => desk(29) });
    const wb = load({
      WBConsoleOpts: { deskSink: { put: () => Promise.resolve({ kind: "held" }), putSync() {} } },
    });
    await wb.whenDeskLoaded();
    assert.equal(wb.DESK_MAX, 30);
    assert.equal(wb.atDeskCap(), false, "29 records leave room for one more");
    globalThis.fetch = async () => ({ ok: true, json: async () => desk(30) });
    await wb.reloadDesk();
    assert.equal(wb.atDeskCap(), true, "30 records are a full desk");
  } finally {
    globalThis.fetch = realFetch;
  }
});
