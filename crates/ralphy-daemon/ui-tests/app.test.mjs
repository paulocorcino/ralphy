// Characterization tests for assets/ui/app.ts — the `shell()` component.
//
// These exist to be written BEFORE app.ts is split, not after. A split of the
// read-only folds out of a 5,000-line component is a refactor only if something
// states what those folds do today; without that it is a rewrite with a
// reassuring diff. Every assertion here was derived by running the fold, not by
// reading it — where the two disagreed, the run won and the surprise is noted.
//
// Scope on purpose: folds that COMPUTE. `shell()` also owns fetch paths, Alpine
// lifecycle and DOM work, and the harness deliberately provides an empty
// document and a throwing `fetch` so that a fold reaching for either fails
// loudly here rather than being quietly half-covered.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { loadShell, UI } from "./harness.mjs";
import { WBDeskSink } from "../assets/ui/wb-desk-sink.ts";
import { WBRelease } from "../assets/ui/wb-release.ts";
import { WBReleaseDialogs } from "../assets/ui/wb-release-dialogs.ts";
import { WBSecurityDialog } from "../assets/ui/wb-security-dialog.ts";
import { WBSettingsDialog } from "../assets/ui/wb-settings-dialog.ts";

// One load, many reads. `loadShell()` is ~20ms of evaluation and every test in
// this file only READS from the state, so they share one. A test that mutates
// must take its own — see `boardRowToIssue` below, which does.
const { state: s } = loadShell();

// NEGATIVE CONTROL for the whole file: a duplicate key in the state literal is
// silently legal in sloppy mode and the LAST one wins, which is how `app.ts`
// carried two incompatible `changesError` declarations. `Object.keys()` cannot
// see the loser, so this reads the source.
test("the state literal declares no key twice", () => {
  const text = readFileSync(join(UI, "app.ts"), "utf8");
  const body = text.slice(text.indexOf("function shell()"), text.indexOf("export function wire("));
  // BOTH spellings, because the literal uses both and the collision that
  // prompted this test could just as easily be two methods. Matching only
  // `name:` saw 119 of the 393 entries and was blind to all 271 method
  // shorthands — a gate covering 30% of the thing it guards.
  const keys = new Map();
  const count = (key) => keys.set(key, (keys.get(key) || 0) + 1);
  for (const m of body.matchAll(/^ {4}([A-Za-z_$][\w$]*):/gm)) count(m[1]);
  // A control keyword at method indent (`    if (…)`) is not a key. Without
  // this the scan reports `if` as declared three times.
  const KEYWORD = new Set(["if", "for", "while", "switch", "catch", "return", "do", "else"]);
  for (const m of body.matchAll(/^ {4}(?:async )?([A-Za-z_$][\w$]*)\s*\(/gm)) {
    if (!KEYWORD.has(m[1])) count(m[1]);
  }
  const dupes = [...keys].filter(([, n]) => n > 1).map(([k]) => k);
  // NEGATIVE CONTROL: the scan must actually reach the literal. A regex that
  // matched nothing would report no duplicates forever.
  assert.ok(
    keys.size > 350,
    `the key scan found only ${keys.size} entries — it is not reading the state literal`,
  );
  assert.deepEqual(
    dupes,
    [],
    "a duplicate key is not an error in sloppy mode — the last declaration " +
      "silently wins and every write against the first shape becomes a no-op",
  );
});

test("fmtUptime steps down through the units and never renders a negative", () => {
  assert.equal(s.fmtUptime(0), "0s");
  assert.equal(s.fmtUptime(45), "45s");
  assert.equal(s.fmtUptime(60), "1m");
  assert.equal(s.fmtUptime(3600), "1h 0m");
  assert.equal(s.fmtUptime(3661), "1h 1m");
  assert.equal(s.fmtUptime(86400), "1d 0h");
  assert.equal(s.fmtUptime(90061), "1d 1h");
  // Two coarse units only: a day-old daemon never shows minutes.
  assert.equal(s.fmtUptime(90000 + 59), "1d 1h");
  // NEGATIVE CONTROL: a clock that ran backwards, and a missing reading, must
  // both render as zero rather than as "-1s" or "NaNs".
  assert.equal(s.fmtUptime(-5), "0s");
  assert.equal(s.fmtUptime(null), "0s");
  assert.equal(s.fmtUptime(undefined), "0s");
});

test("githubUrl resolves the OPEN project's remote, and refuses when it cannot", () => {
  // The pure URL half moved to `wb-project.ts` and is tested there. This is the
  // half that STAYED — finding the open project among `projects` by composite
  // ref — and it lost its coverage in the move: a lookup regression would hand
  // back a link to another repo's issue with the whole suite green.
  const own = loadShell().state;
  own.projects = [
    { slug: "owner/a", remoteUrl: "https://github.com/owner/a.git" },
    { slug: "owner/b", remoteUrl: "https://github.com/owner/b.git" },
  ];

  own.openSlug = own.repoRef(own.projects[1]);
  assert.equal(own.githubUrl(42), "https://github.com/owner/b/issues/42");
  own.openSlug = own.repoRef(own.projects[0]);
  assert.equal(own.githubUrl(42), "https://github.com/owner/a/issues/42");

  // NEGATIVE CONTROL: no project matches, so there is nothing honest to link to
  // — and emphatically not the first project in the list.
  own.openSlug = "owner/never-registered";
  assert.equal(own.githubUrl(42), null);
  own.openSlug = null;
  assert.equal(own.githubUrl(42), null);
});

test("issueBlockers resolves each blocker it can see and admits the ones it cannot", () => {
  const own = loadShell().state;
  own.openSlug = "owner/repo";
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
  const own = loadShell().state;
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

test("clockTitle names both anchors, and says nothing about the ones it lacks", () => {
  const own = loadShell().state;
  own.nowMs = Date.parse("2026-09-08T12:30:00Z");
  const t = own.clockTitle({
    since: "2026-09-08T12:00:00Z",
    startedAt: "2026-09-08T10:00:00Z",
  });
  assert.match(t, /^phase since /);
  assert.match(t, / · run started /);
  // The elapsed time is free from `startedAt` — no second anchor is stored.
  assert.match(t, /\(2h 30m\)/);
  // Under an hour drops the hour segment entirely.
  own.nowMs = Date.parse("2026-09-08T10:07:00Z");
  assert.match(own.clockTitle({ startedAt: "2026-09-08T10:00:00Z" }), /\(7m\)/);
  // NEGATIVE CONTROL: no run, and a run with neither anchor, render empty —
  // never "undefined" or a bare separator.
  assert.equal(own.clockTitle(null), "");
  assert.equal(own.clockTitle({}), "");
});

test("kanbanColumnTitle resolves a column id to its human title", () => {
  // Against the real column table, not against "is a non-empty string" — the
  // previous form asserted `title.length >= id.length || title !== ""`, whose
  // right side was already asserted two lines above, so a fold returning one
  // constant for every column passed it.
  const columns = loadShell().window.WBKanban.COLUMNS;
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

test("rowOpen compares by composite ref, not by slug", () => {
  // Two peers can host the same slug (ADR-0052 §5); only the ref tells them
  // apart, and the open-row highlight is where conflating them shows.
  const own = loadShell().state;
  const a = { slug: "owner/repo", daemon: false };
  own.openSlug = own.repoRef(a);
  assert.equal(own.rowOpen(a), true);
  assert.equal(own.rowOpen({ slug: "owner/other", daemon: false }), false);
});

test("planHeadings drops Steps and stays empty when the prose is for another issue", () => {
  const own = loadShell().state;
  // The plan file carries its own issue key in a trailer; `planBelongsTo` reads
  // it. Both halves of this fold need a plan that HAS one, or the gate
  // short-circuits and neither the filter nor the ownership rule is exercised.
  const plan = (issue) =>
    [
      "## Feasible: yes",
      "## Steps",
      "1. do the thing",
      "## Notes & decisions",
      `<!-- ralphy-plan: issue=${issue} -->`,
    ].join("\n");

  // The prose belongs to the active issue: its headings render, minus Steps —
  // which the steps block owns and would otherwise appear twice.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: plan(42) }), [
    "Feasible: yes",
    "Notes & decisions",
  ]);
  // `planIssue` wins over `active` when both are present.
  assert.deepEqual(own.planHeadings({ planIssue: 7, active: 42, planMd: plan(7) }), [
    "Feasible: yes",
    "Notes & decisions",
  ]);

  // NEGATIVE CONTROL, and the defect the gate exists for: `.ralphy/plan.md`
  // still holds the PREVIOUS issue's plan between runs. Rendering its outline
  // under the current issue is the lie — so a mismatch renders nothing, not a
  // stale outline.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: plan(41) }), []);
  // A plan with no trailer is mid-write or not a ralphy plan; either way it
  // belongs to no issue.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: "## Feasible: yes" }), []);
  assert.deepEqual(own.planHeadings(null), []);
  assert.deepEqual(own.planHeadings({}), []);
});

test("a fresh listing evicts the levels it contradicts — a reused folder name is not the old folder", () => {
  const own = loadShell().state;
  own.openSlug = "me/vc-stress";
  own.treeMem();
  const seed = (rel, entries) => own._treeCache.set(own.treeKey(rel), entries);
  // The key is `<slug>\n<checkout>\n<rel>` (#406 added the middle segment);
  // the rel is its LAST segment.
  const cached = () =>
    [...own._treeCache.keys()].map((k) => k.split("\n").at(-1)).sort();

  // The tree as it stood: `ideias/` holding `dossie/`, itself holding a file.
  seed("", [{ name: "ideias", dir: true }, { name: "README.md", dir: false }]);
  seed("ideias", [{ name: "dossie", dir: true }]);
  seed("ideias/dossie", [{ name: "nota.md", dir: false }]);
  // A sibling that shares the PREFIX but not the path: `ideias_vbforge` must
  // survive a prune of `ideias`, or the fix trades one ghost for a blank folder.
  seed("ideias_vbforge", [{ name: "dossie", dir: true }]);
  own._treeValidated.add(own.treeKey("ideias"));

  // The rename lands: the root no longer lists `ideias`, so everything
  // remembered UNDER it is a statement about a directory that is gone.
  own.pruneTreeCache("", [
    { name: "ideias_vbforge", dir: true },
    { name: "README.md", dir: false },
  ]);
  assert.deepEqual(cached(), ["", "ideias_vbforge"]);
  // Evicted from BOTH: a key left in `_treeValidated` is the half that made the
  // ghost immortal — the level would be painted from a later cache entry and
  // never re-read.
  assert.equal(own._treeValidated.has(own.treeKey("ideias")), false);

  // A name that came back as a FILE is contradicted just as hard as one that
  // vanished — a file has no children to remember.
  seed("ideias", [{ name: "dossie", dir: true }]);
  own.pruneTreeCache("", [{ name: "ideias", dir: false }]);
  assert.equal(own._treeCache.has(own.treeKey("ideias")), false);

  // NEGATIVE CONTROL: a listing that still names its subdirectory evicts
  // nothing, including the level being replaced (its own key is the caller's to
  // overwrite, not this fold's to drop).
  seed("ideias", [{ name: "dossie", dir: true }]);
  seed("ideias/dossie", [{ name: "nota.md", dir: false }]);
  own.pruneTreeCache("ideias", [{ name: "dossie", dir: true }]);
  assert.ok(own._treeCache.has(own.treeKey("ideias")));
  assert.ok(own._treeCache.has(own.treeKey("ideias/dossie")));

  // Scoped by REPO, like every other key read: another project's `ideias/` is
  // not this listing's business.
  own._treeCache.set("other/repo\nideias", [{ name: "dossie", dir: true }]);
  own.pruneTreeCache("", [{ name: "README.md", dir: false }]);
  assert.ok(own._treeCache.has("other/repo\nideias"));
});

test("filteredProjects keeps the open project whatever the query", () => {
  const own = loadShell().state;
  const a = { slug: "owner/alpha", branch: "main", path: "C:\\src\\alpha" };
  const b = { slug: "owner/beta", branch: "main", path: "C:\\src\\beta" };
  own.projects = [a, b];
  own.openSlug = own.repoRef(a);

  // The open row's `<li>` hosts the file tree; a query that matches nothing
  // must not unmount it.
  own.projectQuery = "zzz-matches-nothing";
  assert.deepEqual(own.filteredProjects(), [a]);

  // A query that matches only the sibling keeps both: the sibling because it
  // matches, the open row because it is open.
  own.projectQuery = "beta";
  assert.deepEqual(own.filteredProjects(), [a, b]);

  // NEGATIVE CONTROL: the pin is the open row, not a change to matching — with
  // nothing open the same query filters everything out.
  own.openSlug = null;
  own.projectQuery = "zzz-matches-nothing";
  assert.deepEqual(own.filteredProjects(), []);
});

// The FILES search (ADR-0036 amendment 2026-09-15): the shell's half, driven
// with `WBDaemon.observe` stubbed and no tree mounted — the folds that decide
// what is sent and which reply is believed run without Wunderbaum.
async function withSearchShell(run, reply = { status: "ok", hits: [], truncated: false }) {
  const { state, window } = loadShell();
  const calls = [];
  let answer = reply;
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      return typeof answer === "function" ? answer() : answer;
    },
    // The real door's rule (wb-daemon.test.mjs pins it): the key only for a
    // real name, so a no-selection payload is the pre-#406 one.
    withCheckout: (payload, checkout) =>
      checkout ? { ...payload, checkout: String(checkout) } : { ...payload },
  };
  state.openSlug = "owner/repo";
  state.$refs = {};
  state.$nextTick = (f) => f();
  return await run(state, calls, (a) => (answer = a));
}

test("fileSearchNow sends the mode's verb with the trimmed query", async () => {
  await withSearchShell(async (s, calls) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "  plan ";
    await s.fileSearchNow();
    assert.deepEqual(calls.at(-1), { verb: "tree.find", payload: { repo: "owner/repo", query: "plan" } });
    await s.setFileSearchMode("content");
    assert.deepEqual(calls.at(-1), { verb: "tree.grep", payload: { repo: "owner/repo", query: "plan" } });
    // NEGATIVE CONTROL: under the floor nothing is sent and the note is clear.
    s.fileSearch.query = "p";
    await s.fileSearchNow();
    assert.equal(calls.length, 2);
    assert.equal(s.fileSearch.note, "");
  });
});

test("a reply that is not the newest is dropped, never painted", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    let release;
    answer(() => new Promise((r) => (release = r)));
    s.fileSearch.query = "old";
    const slow = s.fileSearchNow();
    // A newer search lands first.
    answer({ status: "ok", hits: [{ path: "new.md" }], truncated: false });
    s.fileSearch.query = "new";
    await s.fileSearchNow();
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    // …then the slow one resolves with its stale hits: ignored.
    release({ status: "ok", hits: [{ path: "old.md" }], truncated: false });
    await slow;
    assert.deepEqual(s.fileSearch.hits, [{ path: "new.md" }]);
    assert.equal(calls.length, 2);
  });
});

test("the gutter says what the tree cannot: the cap, a miss, a refusal", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    // [case, the daemon's reply, the note under the search box]
    const rows = [
      [
        "the cap",
        { status: "ok", hits: [{ path: "a" }], truncated: true },
        "First 200 matches. Narrow the search to see more.",
      ],
      ["a miss", { status: "ok", hits: [], truncated: false }, "No matches"],
      [
        "a refusal",
        { status: "error", reason: "unknown verb" },
        "Could not search: the daemon does not know that command.",
      ],
    ];
    for (const [name, reply, note] of rows) {
      answer(reply);
      await s.fileSearchNow();
      assert.equal(s.fileSearch.note, note, name);
    }
  });
});

test("only a live CONTENT search lends its term to a tab opened from the tree", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "needle";
    answer({ status: "ok", hits: [{ path: "a.md", count: 2 }], truncated: false });
    await s.setFileSearchMode("content");
    assert.equal(s.fileSearchFindTerm(), "needle");
    // NEGATIVE CONTROLS: a name search, or a closed field, says nothing about
    // what is inside a file.
    await s.setFileSearchMode("name");
    assert.equal(s.fileSearchFindTerm(), null);
    await s.setFileSearchMode("content");
    s.fileSearch.open = false;
    assert.equal(s.fileSearchFindTerm(), null);
  });
});

test("closing the search forgets the query and the hits", async () => {
  await withSearchShell(async (s, calls, answer) => {
    s.fileSearch.open = true;
    s.fileSearch.query = "task";
    answer({ status: "ok", hits: [{ path: "a" }], truncated: false });
    await s.fileSearchNow();
    await s.closeFileSearch();
    assert.equal(s.fileSearch.open, false);
    assert.equal(s.fileSearch.query, "");
    assert.deepEqual(s.fileSearch.hits, []);
    assert.equal(s.fileSearch.note, "");
    assert.equal(s.fileSearch.expandedBefore, null);
  });
});

// The viewer pin (#406): a tab is pinned to the checkout it was opened in, the
// pin is part of its identity, and an explicit `null` is the primary — not
// "whatever is selected now". Driven with no DOM and the viewer mount skipped
// (`$nextTick` swallowed): the tab records are the fold under test.
test("openTab pins the tab to the selected checkout and keys the tab by it", () => {
  const { state, window } = loadShell();
  window.WBView = { patch() {}, read: () => null };
  state.$nextTick = () => {};
  state.openSlug = "owner/repo";
  state.checkouts = { "owner/repo": "wt-a" };

  state.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "x" });
  const pinned = state.tabs.find((t) => t.path === "README.md");
  assert.equal(pinned.checkout, "wt-a", "the default pin is the project's selection");
  assert.equal(pinned.id, "file:owner/repo@wt-a:README.md");

  // The same rel under the PRIMARY is another tab, not the worktree's one.
  state.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "y", checkout: null });
  const primary = state.tabs.filter((t) => t.path === "README.md");
  assert.equal(primary.length, 2, "two trees, two tabs");
  assert.equal(primary[1].checkout, null, "an explicit null is the primary even while wt-a is selected");
  assert.equal(primary[1].id, "file:owner/repo:README.md", "the primary's id is the pre-#406 spelling");

  // Selection moves on; the pins do not.
  state.checkouts = {};
  state.openTab({ project: "owner/repo", path: "README.md", title: "README.md", ftype: "markdown", content: "z", checkout: "wt-a" });
  assert.equal(state.tabs.filter((t) => t.path === "README.md").length, 2, "the pinned tab is reused, not duplicated");
  assert.equal(state.active, "file:owner/repo@wt-a:README.md", "re-opening activates the pinned tab");
});

test("persistView stores the pin and restoreView hands it back explicitly", () => {
  const { state, window } = loadShell();
  let stored = null;
  window.WBView = { patch: (v) => (stored = v), read: () => stored };
  state.$nextTick = () => {};
  state.openSlug = "owner/repo";
  state.checkouts = { "owner/repo": "wt-a" };
  state.openTab({ project: "owner/repo", path: "a.txt", title: "a.txt", ftype: "code", content: "a" });
  state.openTab({ project: "owner/repo", path: "b.txt", title: "b.txt", ftype: "code", content: "b", checkout: null });
  assert.deepEqual(
    stored.tabs.map((t) => [t.path, t.checkout]),
    [["a.txt", "wt-a"], ["b.txt", null]],
  );

  // A fresh shell whose selection is now the PRIMARY restores the pins as
  // stored — the worktree tab must not be re-pinned to the primary.
  const fresh = loadShell();
  fresh.window.WBView = { patch() {}, read: () => stored };
  fresh.state.$nextTick = () => {};
  fresh.state.checkouts = {};
  fresh.state.restoreView();
  assert.deepEqual(
    fresh.state.tabs.filter((t) => t.id.startsWith("file:")).map((t) => [t.id, t.checkout]),
    [["file:owner/repo@wt-a:a.txt", "wt-a"], ["file:owner/repo:b.txt", null]],
  );
});

// --- the slot (ADR-0037 §3c) ----------------------------------------------
// `app.ts` reaches `WBViewer` as a bare global; the real one needs a DOM, so
// the fake records what the shell told it to paint and answers a wide canvas.
function slotShell(width = 1280, seed = null) {
  const { state, window } = loadShell();
  let stored = seed;
  window.WBView = { patch: (v) => (stored = { ...stored, ...v }), read: () => stored };
  state.$nextTick = (fn) => fn();
  state.openSlug = "o/r";
  const painted = [];
  const real = globalThis.WBViewer;
  globalThis.WBViewer = {
    setActive: (id, slot) => painted.push([id, slot ?? null]),
    width: () => width,
    open() {},
    close() {},
    jumpTo() {},
    find() {},
    repath() {},
  };
  const open = (path, ftype = "code") =>
    state.openTab({ project: "o/r", path, title: path, ftype, content: "x", checkout: null });
  // The opens resolve their bytes on a microtask, so the fake stays until
  // those landed; restoring it under them would read `open` off `undefined`.
  const restore = async () => {
    await new Promise((r) => setImmediate(r));
    globalThis.WBViewer = real;
  };
  return { state, painted, open, stored: () => stored, restore };
}
const A = "file:o/r:a.js";
const B = "file:o/r:b.js";

test("pinning a tab paints it beside the active one; activating it focuses the right pane", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    assert.deepEqual(sh.painted.at(-1), [A, { id: B, mirror: false, focus: false, ratio: null }]);
    sh.state.activate(B);
    assert.deepEqual(sh.painted.at(-1), [A, { id: B, mirror: false, focus: true, ratio: null }]);
    assert.deepEqual(sh.state.slot, { kind: "pin", id: B }, "clicking the pinned tab does not unpin it");
    assert.equal(sh.state.lastLeft, A);
  } finally {
    await sh.restore();
  }
});

test("a tab opened from the tree while pinned becomes the left the pin returns to", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    // `openTab` sets `active` without `activate`: the fold is where lastLeft
    // lives. Its paint comes after the bytes resolve (a microtask here).
    sh.open("c.js");
    await new Promise((r) => setImmediate(r));
    assert.deepEqual(sh.painted.at(-1), ["file:o/r:c.js", { id: B, mirror: false, focus: false, ratio: null }]);
    sh.state.activate(B);
    assert.equal(sh.painted.at(-1)[0], "file:o/r:c.js", "the pane the operator was reading, not an earlier one");
  } finally {
    await sh.restore();
  }
});

test("closing the pinned tab while it is ACTIVE clears the slot and falls back to a neighbour", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    sh.state.activate(B);
    sh.state.closeTab(B);
    assert.equal(sh.state.slot, null);
    assert.equal(sh.state.lastLeft, A);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
  } finally {
    await sh.restore();
  }
});

test("a rename re-keys the pin and repaints so the viewer follows the new id", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    sh.state.repathTabs("b.js", "c.js");
    assert.deepEqual(sh.state.slot, { kind: "pin", id: "file:o/r:c.js" });
    assert.deepEqual(sh.painted.at(-1), [A, { id: "file:o/r:c.js", mirror: false, focus: false, ratio: null }]);
  } finally {
    await sh.restore();
  }
});

test("a mirror doubles the active code tab and steps aside for a markdown tab", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("R.md", "markdown");
    sh.state.activate(A);
    sh.state.toggleMirror();
    assert.deepEqual(sh.painted.at(-1), [A, { id: A, mirror: true, focus: false, ratio: null }]);
    sh.state.activate("file:o/r:R.md");
    assert.deepEqual(sh.painted.at(-1), ["file:o/r:R.md", null]);
    assert.deepEqual(sh.state.slot, { kind: "mirror" }, "the mirror waits for the next code tab");
    sh.state.toggleMirror();
    assert.equal(sh.state.slot, null);
  } finally {
    await sh.restore();
  }
});

test("closing the pinned tab clears the slot; a paneless tab keeps it waiting", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    sh.state.activate("consoles");
    assert.deepEqual(sh.painted.at(-1), [null, null]);
    assert.deepEqual(sh.state.slot, { kind: "pin", id: B });
    sh.state.activate(A);
    assert.equal(sh.painted.at(-1)[1].id, B);
    sh.state.closeTab(B);
    assert.equal(sh.state.slot, null);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
    assert.equal(sh.stored().split, null, "the store is told, explicitly, that there is no slot");
  } finally {
    await sh.restore();
  }
});

test("the slot and its ratio persist per client and come back after the tabs do", async () => {
  const sh = slotShell();
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    sh.state.splitRatio = 0.6;
    sh.state.persistView();
    assert.deepEqual(sh.stored().split, { kind: "pin", project: "o/r", path: "b.js", checkout: null, ratio: 0.6 });

    // A fresh shell over the same record, the way a reload is. `$nextTick` is
    // a sink here (the restore's reads would hit the harness's throwing
    // fetch), so the paint is asked for by hand once the restore has run.
    const fresh = slotShell(1280, sh.stored());
    try {
      fresh.state.$nextTick = () => {};
      fresh.state.restoreView();
      assert.deepEqual(fresh.state.slot, { kind: "pin", id: B });
      assert.equal(fresh.state.splitRatio, 0.6);
      assert.equal(fresh.state.active, A);
      fresh.state.syncViewer();
      assert.deepEqual(fresh.painted.at(-1), [A, { id: B, mirror: false, focus: false, ratio: 0.6 }]);
    } finally {
      await fresh.restore();
    }
  } finally {
    await sh.restore();
  }
});

test("under the width floor the shell paints single and keeps the slot for a wider canvas", async () => {
  const sh = slotShell(800);
  try {
    sh.open("a.js");
    sh.open("b.js");
    sh.state.activate(A);
    sh.state.pinTab(B);
    assert.deepEqual(sh.painted.at(-1), [A, null]);
    assert.deepEqual(sh.state.slot, { kind: "pin", id: B });
    assert.equal(sh.state.splitAvailable(), false);
  } finally {
    await sh.restore();
  }
});

// ADR-0063 §3: a NEW console opens in the checkout selected at the moment of
// the click, and in the primary when none is — the console keeps it afterwards
// (the title comes from the daemon's announcement, never from this selection).
// A console is born in the primary whatever the Files chip shows (ADR-0063,
// amendment 2026-09-16 b): the selection is what the panels LOOK at; only the
// console's own title switcher moves it.
test("newConsole opens the agent in the primary even under a selected checkout", () => {
  const { state } = loadShell();
  const calls = [];
  const real = globalThis.WBConsole;
  // `app.ts` reaches `WBConsole` as a bare global; the real one needs a DOM.
  globalThis.WBConsole = { open: (o) => calls.push(o), count: () => 0 };
  try {
    state.active = "consoles";
    state.openSlug = "o/r";
    state.checkouts = { "o/r": "wt-a" };
    state.newConsole("claude");
    state.checkouts = {};
    state.newConsole("codex");
  } finally {
    globalThis.WBConsole = real;
  }
  assert.deepEqual(calls, [
    { repo: "o/r", agent: "claude", checkout: null },
    { repo: "o/r", agent: "codex", checkout: null },
  ]);
});

// The login body is a pure fold over `login` + `security`, extracted so the
// "keep me signed in" box (ADR-0032 amendment 2026-09-16) can be asserted here:
// the daemon treats an absent `remember` as a standard session, so the box
// must only ever ADD the field, never send `remember=false`.
test("loginBody sends remember=true only when the box is checked", () => {
  const { state } = loadShell();
  assert.equal(state.login.remember, false, "opt-in: off by default");
  state.login.code = " 123456 ";
  assert.equal(state.loginBody(), "code=123456", "trimmed code, no remember");
  state.login.remember = true;
  assert.equal(state.loginBody(), "code=123456&remember=true");
  state.security.passwordSet = true;
  state.login.password = "pw";
  assert.equal(state.loginBody(), "code=123456&password=pw&remember=true");
});

test("logOff resets the remember box along with the credentials", async () => {
  const { state } = loadShell();
  state.security.policy = "session";
  state.login.remember = true;
  state.login.passwordRequired = true;
  state.$nextTick = () => {};
  await state.logOff();
  assert.equal(state.authed, false);
  assert.equal(state.login.remember, false, "the box does not survive a log-off");
  assert.equal(state.login.passwordRequired, true, "the server-told flag does");
});

// The Security, Settings and What's new dialogs' open flags are their own
// (wb-security-dialog.ts, wb-settings-dialog.ts, wb-release-dialogs.ts), so the
// shortcuts ask the modal stack for them (ADR-0073 amendment of 2026-10-05).
// They block in the same cases as the flag did: while the dialog is open, under
// another modal too, and not once it closes.
test("the shortcuts are blocked while the Security, Settings or What's new dialog is on the modal stack", () => {
  const keydowns = [];
  const { state, window } = loadShell({
    document: { addEventListener: (type, fn) => type === "keydown" && keydowns.push(fn) },
  });
  // The Ctrl/Cmd+Shift+F listener: the one that opens the files search.
  const filesKey = keydowns.find((fn) => /shiftKey/.test(String(fn)) && /openFileSearch/.test(String(fn)));
  assert.ok(filesKey, "app.ts registers the Ctrl+Shift+F listener at load");
  const searches = [];
  state.authed = true;
  state.openSlug = "o/r";
  state.openFileSearch = () => searches.push("files");
  window.getShell = () => state;
  const press = () =>
    filesKey({ key: "F", ctrlKey: true, metaKey: false, shiftKey: true, altKey: false, preventDefault() {} });
  const scrimEl = { querySelector: () => null };

  assert.equal(state.consoleShortcutsBlocked(), false, "no modal: not blocked");
  press();
  let expected = ["files"];
  assert.deepEqual(searches, expected);

  for (const flag of [
    WBSecurityDialog.openFlag,
    WBSettingsDialog.openFlag,
    WBReleaseDialogs.whatsNewFlag,
  ]) {
    state.modalOpened(flag, scrimEl);
    assert.equal(state.consoleShortcutsBlocked(), true, `${flag} open: blocked`);
    assert.equal(state.consoleShortcutsBlocked(true), true, "a terminal key too");
    press();
    assert.deepEqual(searches, expected, `${flag}: Ctrl+Shift+F is blocked`);

    state.modalOpened("confirmModal.open", scrimEl);
    assert.equal(state.consoleShortcutsBlocked(), true, `a confirm over ${flag}: still blocked`);
    press();
    assert.deepEqual(searches, expected);

    state.modalClosed("confirmModal.open");
    state.modalClosed(flag);
    assert.equal(state.consoleShortcutsBlocked(), false, `${flag} closed: not blocked`);
    press();
    expected = [...expected, "files"];
    assert.deepEqual(searches, expected);
  }
});

// The release view is read again each time the tab comes back (ADR-0056 §7).
// These pin the two rules that make a repeated read safe: a failed read keeps
// what the page knew, and a newer release undoes the dismissal of an older one.
test("loadRelease keeps the last view when the read fails", async () => {
  const { state } = loadShell();
  const known = { ...WBRelease.EMPTY, latest: "v0.1.0-rc.26", severity: "notable", gap: [{}] };
  state.release = known;
  WBRelease.read = async () => null;
  await state.loadRelease();
  assert.equal(state.release, known);
});

test("loadRelease shows a newer release again after the older one was dismissed", async () => {
  const { state } = loadShell();
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

test("returning to the tab reads the release view again", () => {
  const { state } = loadShell();
  const calls = [];
  for (const name of ["maybeRefreshBoard", "loadRepos", "rereadDesk", "resumeSockets", "loadRelease"]) {
    state[name] = () => calls.push(name);
  }
  state.onTabVisible();
  // Each read happens once; their order is not what the tab depends on.
  // `loadRepos` reads the sessions, the fleet, the change set and the branch.
  assert.deepEqual(calls.toSorted(), [
    "loadRelease",
    "loadRepos",
    "maybeRefreshBoard",
    "rereadDesk",
    "resumeSockets",
  ]);
});

test("resumeSockets resumes the file tree socket with the others", () => {
  const { state } = loadShell();
  const resumed = [];
  for (const name of ["_runsSub", "_changesSub", "_presenceSub", "_treeSub"]) {
    state[name] = { resume: (stale) => resumed.push([name, stale]) };
  }
  state.resumeSockets(true);
  assert.deepEqual(resumed, [
    ["_runsSub", true],
    ["_changesSub", true],
    ["_presenceSub", true],
    ["_treeSub", true],
  ]);
});

// ADR-0051 §5: the same chord walks the columns while two or more consoles
// are open, and the fences otherwise. The listener itself is a sink in the
// harness; the decision lives in `arrowStep`.
function arrowShell() {
  const { state } = loadShell();
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
  return { state, calls, box, done };
}

test("Alt+Shift+←/→ walks the columns while they are open and the fences otherwise", () => {
  const { state, calls, box, done } = arrowShell();
  try {
    state.active = "consoles";
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
  const { state, calls, box, done } = arrowShell();
  try {
    state.active = "consoles";
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
  const { state, window } = loadShell();
  const calls = [];
  window.WBNotes = {
    putBack: () => calls.push("putBack"),
    keepOnTop: (id) => calls.push("keepOnTop:" + id),
    list: () => [{ id: "a", onTop: false, away: false }],
  };
  state.$nextTick = (fn) => fn();
  state.active = "consoles";
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
  const { state } = loadShell();
  const calls = [];
  state.$nextTick = () => {};
  state.activate = (tab) => {
    calls.push("activate:" + tab);
    state.active = tab;
  };
  state.active = "code";

  state.openConsoleRunMenu();
  // The menu sits in the Consoles tab's toolbar: on another tab it is hidden.
  assert.deepEqual(calls, ["activate:consoles"]);
  assert.equal(state.agentMenu, true);
  assert.equal(state.consoleRunOpen, true);
});

test("keeping a card on top from another tab opens the Consoles tab first", () => {
  const { state, window } = loadShell();
  const calls = [];
  window.WBNotes = { keepOnTop: (id) => calls.push("keepOnTop:" + id) };
  const ticks = [];
  state.$nextTick = (fn) => ticks.push(fn);
  state.activate = (tab) => {
    calls.push("activate:" + tab);
    state.active = tab;
  };
  state.active = "code";

  state.toggleOnTop({ id: "a", onTop: false, away: false });
  // The card is placed only after the tab is shown: a hidden tab measures 0×0.
  assert.deepEqual(calls, ["activate:consoles"]);
  ticks.forEach((fn) => fn());
  assert.deepEqual(calls, ["activate:consoles", "keepOnTop:a"]);
});

// A shell whose `toggle` side effects are recorders: each method `toggle`
// reaches is replaced, so the test sees the calls `toggle` itself makes.
function toggleShell() {
  const { state } = loadShell();
  const calls = [];
  const record = (name) => (...args) => {
    calls.push([name, ...args]);
  };
  for (const name of [
    "wakePeerFor",
    "loadAgents",
    "ensureWorktreeListing",
    "refreshSpend",
    "destroyTree",
    "mountTree",
    "destroyRunsSub",
    "mountRunsSub",
    "destroyChangesSub",
    "mountChangesSub",
    "loadBoard",
    "hydrateRuns",
    "loadChanges",
    "loadSync",
  ]) {
    state[name] = record(name);
  }
  state.projectRuns = () => [];
  state.planHeadings = () => [];
  state.currentRun = () => null;
  state.$nextTick = (fn) => fn();
  return { state, calls, named: (name) => calls.filter((c) => c[0] === name) };
}

test("opening a row asks to wake its peer, and closing it does not", () => {
  const { state, named } = toggleShell();
  state.toggle("peer:wsl/owner/repo");
  assert.deepEqual(named("wakePeerFor"), [["wakePeerFor", "peer:wsl/owner/repo"]]);
  // CONTROL: the same row again closes it, and a closing row wakes nothing.
  state.toggle("peer:wsl/owner/repo");
  assert.equal(state.openSlug, null);
  assert.equal(named("wakePeerFor").length, 1);
});

test("a row on a host that cannot answer stays closed, and still wakes it", () => {
  const { state, named } = toggleShell();
  const peer = "01KY0000000000000000000000";
  const ref = `${peer}/owner/repo`;
  let groups = [{ daemon: peer, local: false, state: "unreachable" }];
  state.fleetGroups = () => groups;
  state.toggle(ref);
  assert.equal(state.openSlug, null);
  assert.equal(named("mountTree").length, 0);
  assert.deepEqual(named("wakePeerFor"), [["wakePeerFor", ref]]);
  // CONTROL: once the host answers, the same click opens the row.
  groups = [{ daemon: peer, local: false, state: "reachable" }];
  state.toggle(ref);
  assert.equal(state.openSlug, ref);
  assert.equal(named("mountTree").length, 1);
});

test("opening a row remounts the run-completion subscription", () => {
  const { state, calls } = toggleShell();
  state.toggle("owner/repo");
  const changes = calls
    .map((c) => c[0])
    .filter((n) => n === "destroyChangesSub" || n === "mountChangesSub");
  // The old socket closes before the new one opens, so a nudge for the
  // project that WAS open never reloads the new one.
  assert.deepEqual(changes, ["destroyChangesSub", "mountChangesSub"]);
});

test("emitCreate sends the directory the create lands in", () => {
  const { state, window } = loadShell();
  const emitted = [];
  window.WB = { emit: (action, detail) => emitted.push({ action, ...detail }) };
  state.openSlug = "owner/repo";
  const root = { title: "root", parent: null };
  const src = { title: "src", parent: root, data: { folder: true } };
  const file = { title: "main.rs", parent: src, data: {} };
  state.emitCreate(file, "file");
  state.emitCreate(src, "folder");
  state.emitCreate(null, "file");
  assert.deepEqual(
    emitted.map((e) => [e.action, e.project, e.path, e.kind]),
    [
      ["create", "owner/repo", "src", "file"],
      ["create", "owner/repo", "src", "folder"],
      ["create", "owner/repo", "", "file"],
    ],
  );
});

test("the create action asks for the name through the shell's prompt", async () => {
  const listeners = [];
  const { window } = loadShell({
    document: {
      addEventListener: (type, fn) => type === "workbench:action" && listeners.push(fn),
    },
  });
  const written = [];
  window.WBDaemon = { write: (verb, payload) => written.push({ verb, payload }) };
  const asked = [];
  const shell = {
    askPrompt: async (opts) => {
      asked.push(opts);
      return null;
    },
    checkoutOf: () => null,
  };
  window.getShell = () => shell;
  const prompted = [];
  window.prompt = (msg) => {
    prompted.push(msg);
    return null;
  };
  assert.ok(listeners.length > 0, "app.ts subscribes to workbench:action at load");
  await Promise.all(
    listeners.map((fn) => fn({ detail: { action: "create", project: "owner/repo", path: "src", kind: "file" } })),
  );
  assert.deepEqual(
    asked.map((o) => o.title),
    ["New file in src"],
    "the name is asked once, through askPrompt",
  );
  assert.deepEqual(prompted, [], "the browser's prompt is only the no-shell fallback");
  // A cancelled prompt creates nothing.
  assert.deepEqual(written, []);
});

test("wakePeerFor wakes the daemon of a sleeping peer's row only", () => {
  const { state } = loadShell();
  const peer = "01KY0000000000000000000000";
  const woken = [];
  state.wakePeer = (daemon) => woken.push(daemon);
  let groups = [{ daemon: peer, state: "asleep", nudgeable: true }];
  state.fleetGroups = () => groups;
  state.wakePeerFor(`${peer}/owner/repo`);
  assert.deepEqual(woken, [peer]);
  // CONTROLS: a local row and an awake peer are not woken.
  state.wakePeerFor("owner/repo");
  groups = [{ daemon: peer, state: "online", nudgeable: true }];
  state.wakePeerFor(`${peer}/owner/repo`);
  assert.deepEqual(woken, [peer]);
});

test("askPrompt settles with the trimmed name the prompt submits", async () => {
  const { state } = loadShell();
  const answer = state.askPrompt({ title: "New file in src" });
  assert.equal(state.promptModal.open, true);
  assert.equal(state.promptModal.title, "New file in src");
  // A name that cannot be one directory entry keeps the dialog open.
  state.promptModal.value = "a/b";
  state.promptSubmit();
  assert.equal(state.promptModal.open, true);
  assert.equal(state.promptModal.error, "name cannot contain / or \\");
  state.promptModal.value = "  notes.md ";
  state.promptSubmit();
  assert.equal(state.promptModal.open, false);
  assert.equal(await answer, "notes.md");
});

// ---- folds the tree, the Changes panel and the plan viewer rely on ----------

test("isFolder reads Wunderbaum's data bag, a lazy flag or a child list", () => {
  // Wunderbaum copies a source key it does not define into `node.data`, so the
  // listing's `folder: true` arrives there, and a lazy folder has no children
  // until it is read.
  const rows = [
    ["a collapsed folder from the listing", { data: { folder: true }, children: null }, true],
    ["a lazy folder not read yet", { data: {}, lazy: true, children: null }, true],
    ["an expanded folder", { data: {}, children: [] }, true],
    ["a file", { data: {}, children: null }, false],
    ["no node (empty tree space)", null, false],
  ];
  for (const [why, node, want] of rows) assert.equal(s.isFolder(node), want, why);
});

test("the Move item is withheld inside .git and .ralphy, except for a note in the notes directory", () => {
  // The context menu is where the UI's mirror of the daemon's carve-out
  // (`fswrite::is_note_in_notes_dir`) decides what to offer: exactly
  // `.ralphy/notes/<name>.note`, nothing deeper or differently named.
  const own = loadShell().state;
  let items = null;
  own.renderMenu = (_x, _y, list) => {
    items = list;
  };
  const node = (rel) =>
    rel.split("/").reduce((parent, title) => ({ title, parent, data: {}, children: null }), {
      title: "root",
    });
  const offersMove = (rel) => {
    own.showMenu(0, 0, node(rel));
    return items.some((i) => i.label?.startsWith("Move"));
  };
  const rows = [
    [".ralphy/notes/idea.note", true],
    ["src/app.js", true],
    [".ralphy/notes/sub/idea.note", false],
    [".ralphy/notes/.note", false],
    [".ralphy/notes/idea.md", false],
    [".ralphy/drafts/idea.note", false],
    [".git/notes/idea.note", false],
  ];
  for (const [rel, want] of rows) assert.equal(offersMove(rel), want, rel);
});

// Every act the Changes panel dispatches, each answering with a refusal.
const CHANGE_ACTS = [
  ["syncFetch", (st) => st.syncFetch("o/r")],
  ["syncPull", (st) => st.syncPull("o/r")],
  ["syncPush", (st) => st.syncPush("o/r")],
  ["stagePaths", (st) => st.stagePaths("o/r", ["a.txt"])],
  ["unstagePaths", (st) => st.unstagePaths("o/r", ["a.txt"])],
  ["discardRow", (st) => st.discardRow("o/r", { path: "a.txt", status: "modified" })],
  [
    "commitStaged",
    (st) => {
      st.commitMsgSlug = "o/r";
      st.commitMsg = "a message";
      return st.commitStaged("o/r");
    },
  ],
];

test("a refused Changes act lands in the Changes panel and still flashes", async () => {
  for (const [act, run] of CHANGE_ACTS) {
    const { state: st, window } = loadShell();
    const said = `The daemon refused ${act}.`;
    window.WBDaemon.observe = async () => ({ status: "error", message: said });
    st.loadChanges = () => {};
    st.loadSync = () => {};
    st.askConfirm = async () => true;
    const flashed = [];
    st._flashAction = (msg) => flashed.push(msg);
    await run(st);
    assert.equal(st.changesError, said, `${act}: the panel shows the refusal`);
    assert.deepEqual(flashed, [said], `${act}: the flash still carries it`);
  }
});

test("one remote act at a time: the busy slot refuses a second act and frees itself on every exit", async () => {
  const { state: st, window } = loadShell();
  st.loadChanges = () => {};
  st.loadSync = () => {};
  st._flashAction = () => {};
  const sent = [];
  const answers = [];
  window.WBDaemon.observe = (verb) => {
    sent.push(verb);
    return new Promise((resolve) => answers.push(resolve));
  };
  const acts = [st.syncFetch("o/r")];
  assert.equal(st.syncBusy, "fetch");
  acts.push(st.syncPull("o/r"), st.syncPush("o/r"));
  const whileBusy = [...sent];
  for (const answer of answers) answer({ status: "ok" });
  await Promise.all(acts);
  assert.deepEqual(whileBusy, ["sync.fetch"], "an act that finds the slot taken sends nothing");
  assert.equal(st.syncBusy, null, "an answered act frees the slot");

  window.WBDaemon.observe = async () => ({ status: "error", message: "The push was refused." });
  await st.syncPush("o/r");
  assert.equal(st.syncBusy, null, "a refused act frees the slot");

  window.WBDaemon.observe = async (verb) => {
    sent.push(verb);
    throw new Error("the socket closed");
  };
  await st.syncPull("o/r");
  assert.equal(st.syncBusy, null, "an act whose transport threw frees the slot");
  assert.deepEqual(sent, ["sync.fetch", "sync.pull"], "the freed slot takes the next act");
});

test("the plan prose renders only for the issue its trailer names", () => {
  const own = loadShell().state;
  const plan = (issue) =>
    ["## Feasible: yes", "## Steps", "1. do the thing", `<!-- ralphy-plan: issue=${issue} -->`].join(
      "\n",
    );
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: plan(42) }), true);
  assert.equal(own.planProseIsCurrent({ planIssue: 7, active: 42, planMd: plan(7) }), true);
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: plan(41) }), false);
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: "## Feasible: yes" }), false);

  // `marked` and `DOMPurify` are vendor globals the harness does not load; an
  // identity pair shows which section text reached them.
  const saved = { marked: globalThis.marked, DOMPurify: globalThis.DOMPurify };
  globalThis.marked = { parse: (text) => text };
  globalThis.DOMPurify = { sanitize: (html) => html };
  try {
    assert.notEqual(own.renderPlanSection({ active: 42, planMd: plan(42) }, "Feasible: yes"), "");
    assert.equal(
      own.renderPlanSection({ active: 42, planMd: plan(41) }, "Feasible: yes"),
      "",
      "a plan written for another issue renders nothing",
    );
  } finally {
    globalThis.marked = saved.marked;
    globalThis.DOMPurify = saved.DOMPurify;
  }
});

test("spendView shows the spend document only for the project that is open", () => {
  const own = loadShell().state;
  own.spend = { loading: false, error: "", doc: { total: "~$1.00" }, slug: "owner/a" };
  assert.equal(own.spendView().kind, "empty", "no project open");
  own.openSlug = "owner/a";
  assert.equal(own.spendView().kind, "ready");
  assert.equal(own.spendView().total, "~$1.00");
  // A document read for another project is stale: the pane waits for its own.
  own.openSlug = "owner/b";
  assert.equal(own.spendView().kind, "loading");
});

// `/api/repos` carries `head` beside `branch` (#510): a detached HEAD has no
// branch, and the row must still name its commit.
test("loadRepos keeps the head of a detached repo for the row title", async () => {
  const { state } = loadShell();
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  state.loadChanges = () => {};
  state.loadSync = () => {};
  const row = {
    slug: "o/r",
    path: "/r",
    reachable: true,
    branch: null,
    head: { kind: "detached", sha: "abc1234" },
    dirty: false,
    remote: null,
  };
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, json: async () => [row] });
  try {
    await state.loadRepos();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(state.projects.length, 1);
  assert.equal(state.rowTitle(state.projects[0]), "o/r · abc1234");
});

// The sidebar dot says GitHub only when github.com is the remote's host.
test("loadRepos marks a row github only for a remote hosted on github.com", async () => {
  const { state } = loadShell();
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  state.loadChanges = () => {};
  state.loadSync = () => {};
  const row = (slug, remote) => ({ slug, path: "/" + slug, reachable: true, branch: "main", dirty: false, remote });
  const rows = [
    row("o/gh", "git@github.com:o/gh.git"),
    row("o/fake", "https://github.com.evil.example/o/fake"),
    row("o/none", null),
  ];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, json: async () => rows });
  try {
    await state.loadRepos();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    state.projects.map((p) => [p.slug, p.remote]),
    [["o/gh", "github"], ["o/fake", "local"], ["o/none", "local"]],
  );
});

// --- ADR-0070 D2: a shown fact is read again on named events only ----------

// Every URL the code under test fetched, answered with an empty 200. A bare
// `fetch` in app.ts resolves to `globalThis.fetch`, not the harness window's.
// `until`, when given, names a URL the read must reach: the spy waits for it
// (up to 2 s) instead of a fixed number of event-loop turns, because a read
// scheduled with `setTimeout` can land after any number of turns.
async function withFetchSpy(fn, until = null) {
  const urls = [];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    urls.push(String(url));
    return { ok: true, status: 200, json: async () => [] };
  };
  try {
    await fn(urls);
    // Let the reads chained behind the first await land.
    for (let i = 0; i < 10; i++) await new Promise((r) => setImmediate(r));
    const deadline = Date.now() + 2000;
    while (until && !urls.includes(until) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 5));
  } finally {
    globalThis.fetch = realFetch;
  }
  return urls;
}

test("a presence frame reads nothing", async () => {
  const { state, window } = loadShell();
  let beat = null;
  window.WBDaemon.subscribePresence = (onPresence) => {
    beat = onPresence;
    return { resume() {}, close() {} };
  };
  state.subscribePresence();
  const urls = await withFetchSpy(() => beat({ uptime_secs: 1 }));
  assert.deepEqual(urls, [], "the heartbeat is not a read trigger");
  assert.equal(state.uptimeText, "Running for 1s");
});

test("a hidden tab reads nothing on a push, and reads when it becomes visible", async () => {
  const { state, document } = loadShell({ document: { visibilityState: "hidden", hasFocus: () => true } });
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  // app.ts schedules the settle read with Node's global `setTimeout`. Catch
  // the timers here and run them inside the spy, so a read that the timer
  // would make is seen on every run and not only when it fires early.
  const timers = [];
  const realSetTimeout = globalThis.setTimeout;
  const hidden = await withFetchSpy(() => {
    globalThis.setTimeout = (fn) => {
      timers.push(fn);
      return timers.length;
    };
    try {
      state.onPresencePush("sessions.dirty", {});
      state.onPresencePush("repos.dirty", {});
      state.onPresencePush("desk.dirty", { tab: "other" });
      state.onPresenceOpen(true);
    } finally {
      globalThis.setTimeout = realSetTimeout;
    }
    for (const run of timers) run();
  });
  assert.deepEqual(hidden, []);
  document.visibilityState = "visible";
  const visible = await withFetchSpy(() => state.onTabVisible());
  assert.ok(visible.includes("/api/sessions"), visible.join(", "));
  assert.ok(visible.includes("/api/repos"), visible.join(", "));
  assert.ok(visible.includes("/api/desk"), visible.join(", "));
});

test("peers.dirty and repos.dirty read the project list", async () => {
  for (const verb of ["peers.dirty", "repos.dirty"]) {
    const { state } = loadShell({ document: { visibilityState: "visible", hasFocus: () => true } });
    state.checkColumnDesk = async () => {};
    const urls = await withFetchSpy(() => state.onPresencePush(verb, {}));
    assert.ok(urls.includes("/api/repos"), `${verb}: ${urls.join(", ")}`);
  }
});

test("desk.dirty from this tab is ignored, and from another tab reads the desk", () => {
  const { state, window } = loadShell({ document: { visibilityState: "visible", hasFocus: () => true } });
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  let reads = 0;
  window.WBConsole.reloadDesk = () => {
    reads += 1;
    return Promise.resolve();
  };
  state.onPresencePush("desk.dirty", { tab: WBDeskSink.tabId() });
  assert.equal(reads, 0, "this tab's own write");
  state.onPresencePush("desk.dirty", { tab: "another-tab" });
  assert.equal(reads, 1);
});

test("a reopened presence socket reads sessions, projects and the desk; the first open reads nothing", async () => {
  const { state } = loadShell({ document: { visibilityState: "visible", hasFocus: () => true } });
  // The column check after a desk read is not under test here.
  state.checkColumnDesk = async () => {};
  const first = await withFetchSpy(() => state.onPresenceOpen(false));
  assert.deepEqual(first, []);
  const again = await withFetchSpy(() => state.onPresenceOpen(true));
  for (const url of ["/api/sessions", "/api/repos", "/api/desk"]) {
    assert.ok(again.includes(url), `${url} in ${again.join(", ")}`);
  }
});

test("login reads the board, the runs and the tree again", () => {
  const { state } = loadShell();
  const calls = [];
  for (const name of ["loadRepos", "loadIdentity", "loadAgents", "restoreView", "hydrateRuns"]) {
    state[name] = () => calls.push(name);
  }
  state.maybeRefreshBoard = (why) => calls.push(`board:${why}`);
  state._treeSub = { replay: () => calls.push("tree") };
  // Closed: the runs lock the writes whether the panel shows them or not.
  state.runsOpen = false;
  state.rehydrateAfterAuth();
  for (const want of ["loadRepos", "board:login", "hydrateRuns", "tree"]) {
    assert.ok(calls.includes(want), `${want} in ${calls.join(", ")}`);
  }
});

// --- ADR-0070 D3: a failed read keeps the last good value, marked not current

// Answers each fetch with the next reply in `replies` (`{ status, body }`).
function scriptedFetch(replies) {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    const { status, body } = replies.shift() || { status: 500, body: null };
    return { ok: status === 200, status, json: async () => body };
  };
  return () => (globalThis.fetch = realFetch);
}

test("a failed project read after a good one keeps the list, marked not current", async () => {
  const { state } = loadShell();
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  state.loadChanges = () => {};
  state.loadSync = () => {};
  const restore = scriptedFetch([
    { status: 200, body: [{ slug: "a/b", path: "/ab", reachable: true }] },
    { status: 500, body: null },
  ]);
  try {
    await state.loadRepos();
    await state.loadRepos();
  } finally {
    restore();
  }
  assert.equal(state.projects.length, 1);
  assert.equal(state.projects[0].slug, "a/b");
  assert.match(state.reposError, /Not current: the daemon answered 500/);
});

test("a failed first project read is empty and says why", async () => {
  const { state } = loadShell();
  state.refreshLive = () => {};
  const restore = scriptedFetch([{ status: 500, body: null }]);
  try {
    await state.loadRepos();
  } finally {
    restore();
  }
  assert.deepEqual(state.projects, []);
  assert.equal(state.reposError, "Could not load the projects from the daemon: the daemon answered 500.");
});

test("a failed fleet read after a good one keeps the peers and their rows", async () => {
  const { state } = loadShell();
  const fleet = {
    peers: [{ daemon_id: "p1", name: "wsl", environment: "WSL: U", state: "reachable" }],
    repos: [{ key: "p1/o/r", slug: "o/r", daemon_id: "p1", reachable: true }],
  };
  const restore = scriptedFetch([
    { status: 200, body: fleet },
    { status: 502, body: null },
  ]);
  try {
    await state.loadFleet();
    state.projects = state.projects.filter((p) => !p.daemon);
    await state.loadFleet();
  } finally {
    restore();
  }
  assert.equal(state.fleetPeers.length, 1);
  assert.equal(state.projects.filter((p) => p.daemon === "p1").length, 1);
  assert.match(state.fleetError, /Not current: the daemon answered 502/);
});

test("a failed session read after a good one keeps the list, marked not current", async () => {
  const { state } = loadShell();
  const restore = scriptedFetch([
    { status: 200, body: [{ id: 1, repo: "o/r" }] },
    { status: 500, body: null },
  ]);
  try {
    await state.refreshLive();
    await state.refreshLive();
  } finally {
    restore();
  }
  assert.equal(state.liveSessions.length, 1);
  assert.match(state.sessionsError(), /Not current/);
});

// A shell whose `WBDaemon.observe` answers each call with the next reply.
function observedShell(replies) {
  const { state, window } = loadShell();
  window.WBDaemon.observe = async () => replies.shift() ?? null;
  state.openSlug = "o/r";
  state._flashAction = () => {};
  return state;
}

const CHANGES_OK = {
  status: "ok",
  changes: { changes: [{ path: "a.txt", index: " ", worktree: "M" }] },
};

test("a failed change-set read after a good one keeps the groups, marked not current", async () => {
  const state = observedShell([CHANGES_OK, { status: "error", message: "git exited 128" }]);
  await state.loadChanges("o/r");
  const before = state.changesCount["o/r"];
  assert.ok(before > 0, "the first read found a change");
  await state.loadChanges("o/r");
  assert.equal(state.changesCount["o/r"], before);
  assert.equal(state.changesRead["o/r"].current, false);
  assert.match(state.changesReadError["o/r"], /Not current: git exited 128/);
});

test("a failed board read after a good one keeps the cards, marked not current", async () => {
  const state = observedShell([
    { status: "ok", board: { issues: [{ number: 7, title: "x", labels: [] }], labels: [] } },
    { status: "error", message: "gh not authed" },
  ]);
  state.loadPlan = () => {};
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
  const state = observedShell([
    { status: "ok", board: { issues: [], labels: [{ name: "bug", color: "d73a4a" }] } },
  ]);
  state.loadPlan = () => {};
  state.kanbanSel = null;
  await state.loadBoard();
  assert.equal(state.labelColor("bug"), "#d73a4a");
  for (const name of ["constructor", "toString", "__proto__"]) {
    assert.equal(state.labelColor(name), globalThis.window.WBKanban.labelColor(name), name);
  }
});

test("a failed runs read after a good one keeps the runs, marked not current", async () => {
  const state = observedShell([{ status: "ok", runs: [] }, { status: "error", reason: "the run store is locked" }]);
  state.runsOpen = false;
  await state.hydrateRuns();
  state.runsByProject["o/r"] = [{ runid: "r1" }];
  await state.hydrateRuns();
  assert.deepEqual(state.runsByProject["o/r"], [{ runid: "r1" }]);
  assert.equal(state.runsRead["o/r"].current, false);
  assert.match(state.runsError, /Not current: the run store is locked/);
});

// ADR-0070 D3: a write acts on what the page shows, so it is locked while that
// is not current, and says why.
test("writes are locked while the change set is not current", async () => {
  const state = observedShell([CHANGES_OK, { status: "error", message: "git exited 128" }]);
  state.runsByProject["o/r"] = [];
  await state.loadChanges("o/r");
  assert.equal(state.writeLocked(), false, "a good read locks nothing");
  await state.loadChanges("o/r");
  assert.equal(state.writeLocked(), true);
  assert.match(state.writeLockReason(), /not current/);
});

test("moving a board card is locked while the board is not current", async () => {
  const state = observedShell([
    { status: "ok", board: { issues: [], labels: [] } },
    { status: "error", message: "gh not authed" },
  ]);
  state.loadPlan = () => {};
  state.kanbanSel = null;
  state.runsByProject["o/r"] = [];
  await state.loadBoard();
  assert.equal(state.labelsLocked(), false);
  await state.loadBoard();
  assert.equal(state.labelsLocked(), true);
  assert.match(state.labelLockReason(), /board shown is not current/);
});

// NEGATIVE CONTROL: two good reads lock nothing.
test("two good change-set reads lock no write", async () => {
  const state = observedShell([CHANGES_OK, CHANGES_OK]);
  state.runsByProject["o/r"] = [];
  await state.loadChanges("o/r");
  await state.loadChanges("o/r");
  assert.equal(state.writeLocked(), false);
});

// --- ADR-0070 D6: a tab on an older build than the daemon -------------------

function skewShell({ dirty = false, pageBuild = "A" } = {}) {
  const { state, window } = loadShell();
  let reloads = 0;
  window.location.reload = () => (reloads += 1);
  window.WBViewer.anyDirty = () => dirty;
  window.WBNotes.anyDirty = () => false;
  let beat = null;
  window.WBDaemon.subscribePresence = (onPresence) => {
    beat = onPresence;
    return { resume() {}, close() {} };
  };
  state.pageBuild = pageBuild;
  state.subscribePresence();
  return { state, beat: (p) => beat(p), reloads: () => reloads };
}

test("a build the page was not served with reloads a tab with no unsaved work", () => {
  const t = skewShell();
  t.beat({ uptime_secs: 1, build: "A" });
  assert.equal(t.reloads(), 0, "the same build reloads nothing");
  t.beat({ uptime_secs: 3, build: "B" });
  assert.equal(t.reloads(), 1);
});

test("with unsaved work the tab keeps the page, shows the notice and locks writes", () => {
  const t = skewShell({ dirty: true });
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 0);
  assert.equal(t.state.buildSkew, true);
  assert.equal(t.state.writeLocked(), true);
  assert.match(t.state.writeLockReason(), /older than Ralphy/);
});

test("a page with no build id never reloads for a build", () => {
  const t = skewShell({ pageBuild: "" });
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 0);
  assert.equal(t.state.buildSkew, false);
});

// --- review fixes (#511) ----------------------------------------------------

// The console module names the consoles whose records left the desk; the
// columns lose them first, then the stage does — every one of them, a console
// outside the columns too.
test("checkColumnDesk takes the consoles that left the desk out of the columns, then off the stage", () => {
  const { state } = loadShell();
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
  const { state, window, document } = loadShell();
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

test("two fleet reads close together list each peer row once", async () => {
  const { state } = loadShell();
  state.projects = [{ slug: "a/b", tree: [] }];
  const fleet = {
    peers: [{ daemon_id: "p1", name: "wsl", environment: "WSL: U", state: "reachable" }],
    repos: [{ key: "p1/o/r", slug: "o/r", daemon_id: "p1", reachable: true }],
  };
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => fleet });
  try {
    await Promise.all([state.loadFleet(), state.loadFleet()]);
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(state.projects.filter((p) => p.daemon === "p1").length, 1);
  assert.equal(state.projects.filter((p) => !p.daemon).length, 1);
});

test("fleet reads asked now share the read in flight, and a later ask reads again", async () => {
  const { state } = loadShell();
  state.projects = [];
  let reads = 0;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (url === "/api/fleet") reads += 1;
    return { ok: true, status: 200, json: async () => ({ peers: [], repos: [] }) };
  };
  try {
    await Promise.all([state.readFleetNow(), state.readFleetNow(), state.readFleetNow()]);
    assert.equal(reads, 1);
    await state.readFleetNow();
    assert.equal(reads, 2);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("the consoles get the shell's fleet read with the fleet", () => {
  let hooks = null;
  const { state, window } = loadShell();
  window.WBConsole = { ingestFleet: (_groups, h) => (hooks = h) };
  let asked = 0;
  state.readFleetNow = () => {
    asked += 1;
    return Promise.resolve();
  };
  state.shareFleet();
  hooks.read();
  assert.equal(asked, 1);
});

// --- FILES while the open project's peer is down ---------------------------

const FILES_PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAZ";
const FILES_REF = `${FILES_PEER}/o/r`;

// A shell with the peer project open and the peer in the given fleet state.
function peerFilesShell(state) {
  const loaded = loadShell();
  const s = loaded.state;
  s.projects = [];
  s.fleetPeers = [{ daemon_id: FILES_PEER, name: "corcino-mac", environment: "macOS 15", tunnel: true, state, diagnosis: `diagnosis of ${state}` }];
  s.openSlug = FILES_REF;
  s.treeMem();
  return loaded;
}

// The options `mountTree` gives Wunderbaum, from a stand-in that keeps them.
function mountedTreeOptions(s, window, document) {
  let options = null;
  // No `/ws/tree` subscription: this checks the tree's own options.
  window.WBDaemon = {};
  const realMar10 = globalThis.mar10;
  globalThis.mar10 = {
    Wunderbaum: function (o) {
      options = o;
    },
  };
  const host = { addEventListener() {} };
  document.querySelector = (sel) => (sel === ".project.open .wb-host" ? host : null);
  s.projects = [{ key: FILES_REF, slug: "o/r", daemon: FILES_PEER, tree: [] }];
  s.useDaemonTree = () => true;
  s.loadTreeLevel = () => Promise.resolve([]);
  try {
    s.mountTree();
  } finally {
    globalThis.mar10 = realMar10;
  }
  return options;
}

test("a tree level that fails to load draws no error row, closes, and asks the fleet", async () => {
  const { state, window, document } = peerFilesShell("reachable");
  const options = mountedTreeOptions(state, window, document);
  state.loadTreeLevel = () => Promise.reject(new Error("macOS 15 did not answer: os error 10054. Start its daemon."));
  let fleetReads = 0;
  state.readFleetNow = () => {
    fleetReads += 1;
    return Promise.resolve();
  };
  const closed = [];
  const node = { title: "src", data: {}, lazy: true, parent: null, setExpanded: (flag) => closed.push(flag) };
  state.relPath = () => "src";
  const result = await options.lazyLoad({ node });
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(result, false, "a rethrow makes Wunderbaum draw an error row");
  assert.deepEqual(closed, [false]);
  assert.equal(fleetReads, 1);
  assert.ok(state.treeStale.endsWith("The list shown is the last one read."), state.treeStale);
});

test("while the peer is down, only a folder read before opens", () => {
  const { state, window, document } = peerFilesShell("unreachable");
  const options = mountedTreeOptions(state, window, document);
  state.relPath = (n) => n.rel;
  state._treeCache.set(state.treeKey("read"), []);
  const folder = (rel) => ({ rel, children: null });
  assert.equal(options.beforeExpand({ flag: true, node: folder("never") }), false);
  assert.equal(options.beforeExpand({ flag: true, node: folder("read") }), undefined);
  assert.equal(options.beforeExpand({ flag: false, node: folder("never") }), undefined);
  state.fleetPeers[0].state = "reachable";
  assert.equal(options.beforeExpand({ flag: true, node: folder("never") }), undefined);
});

test("FILES names the down peer from the fleet, and the read that calls it back re-reads the tree", () => {
  const { state } = peerFilesShell("tunnel-silent");
  const down = state.openPeerDown();
  assert.equal(down?.daemon, FILES_PEER);
  assert.equal(state.peerDownText(down), "corcino-mac is not connected. The list shown is the last one read.");
  assert.equal(state.peerDownAction(down), "Try again");
  const reread = [];
  state.revalidateLevel = (rel) => reread.push(rel);
  state._tree = {};
  const folders = [
    { rel: "src", expanded: true },
    { rel: "docs", expanded: false },
  ];
  state.rawTree = () => ({ root: { visit: (fn) => folders.forEach(fn) } });
  state.isFolder = () => true;
  state.relPath = (n) => n.rel;
  state.treeStale = "Could not refresh the file list.";
  state.filesFollowFleet();
  assert.deepEqual(reread, [], "the peer is still down");
  state.fleetPeers[0].state = "reachable";
  state.filesFollowFleet();
  assert.deepEqual(reread, ["", "src"]);
  assert.equal(state.treeStale, "");
  state.filesFollowFleet();
  assert.deepEqual(reread, ["", "src"], "only the read that calls the peer back");
});

test("a new checkout's change set does not inherit the old tree's last read", () => {
  const { state } = loadShell();
  state.loadChanges = () => {};
  state.loadSync = () => {};
  state.changesRead["o/r"] = { value: true, goodAt: 5, error: "", current: true };
  state.syncRead["o/r"] = { value: true, goodAt: 5, error: "", current: true };
  state.setCheckout("o/r", "wt-a");
  assert.equal(state.changesRead["o/r"], undefined);
  assert.equal(state.syncRead["o/r"], undefined);
});

// --- second review of #511 ---------------------------------------------------

const VISIBLE = { document: { visibilityState: "visible", hasFocus: () => true } };

test("sessions.dirty on a visible tab reads the sessions", async () => {
  const { state } = loadShell(VISIBLE);
  state.LIVE_SETTLE_MS = 0;
  const urls = await withFetchSpy(() => state.onPresencePush("sessions.dirty", {}), "/api/sessions");
  assert.ok(urls.includes("/api/sessions"), urls.join(", "));
});

test("a tab that becomes visible reads the runs with the Runs panel closed, so an ended run unlocks the writes", async () => {
  const { state, window } = loadShell(VISIBLE);
  state.checkColumnDesk = async () => {};
  state.loadChanges = async () => {};
  state.loadSync = async () => {};
  window.WBDaemon.observe = async (verb) => (verb === "runs.list" ? { status: "ok", runs: [] } : null);
  state.openSlug = "o/r";
  state.runsOpen = false;
  state.runsByProject["o/r"] = [{ runid: "r1" }];
  assert.equal(state.writeLocked(), true, "the run seen before the tab was hidden locks");
  await withFetchSpy(() => state.onTabVisible());
  assert.equal(state.writeLocked(), false, "the run ended while hidden: the writes unlock");
});

test("a reopened socket reads the board and the runs", () => {
  const { state } = loadShell(VISIBLE);
  const calls = [];
  for (const name of ["loadRepos", "rereadDesk", "hydrateRuns"]) state[name] = () => calls.push(name);
  state.maybeRefreshBoard = (why) => calls.push(`board:${why}`);
  state.openSlug = "o/r";
  state.onPresenceOpen(true);
  assert.ok(calls.includes("board:reopen"), calls.join(", "));
  assert.ok(calls.includes("hydrateRuns"), calls.join(", "));
});

test("visible and login ask the open panels to read again, and read the open Spend view", () => {
  // The Settings dialog hears the event and reads only when it is open
  // (wb-settings-dialog.test.mjs); here the shell sends it each time.
  const events = [];
  const { state } = loadShell({
    ...VISIBLE,
    window: { dispatchEvent: (e) => events.push(e.type) },
  });
  const calls = [];
  for (const name of ["loadRepos", "rereadDesk", "hydrateRuns", "loadRelease", "resumeSockets", "loadIdentity", "loadAgents", "restoreView"]) {
    state[name] = () => {};
  }
  state.maybeRefreshBoard = () => {};
  state.loadSpend = () => calls.push("spend");
  const rereads = () => events.filter((t) => t === "workbench:panels-reread").length;
  state.tabs = state.tabs.filter((t) => t.id !== "spend");
  state.onTabVisible();
  assert.deepEqual(calls, [], "a closed Spend view reads nothing");
  assert.equal(rereads(), 1, "the tab becoming visible asks the open panels");
  state.tabs.push({ id: "spend", kind: "spend" });
  state.onTabVisible();
  assert.deepEqual(calls, ["spend"]);
  assert.equal(rereads(), 2);
  calls.length = 0;
  state.rehydrateAfterAuth();
  assert.deepEqual(calls, ["spend"]);
  assert.equal(rereads(), 3, "a login asks the open panels too");
});

test("the peer tick and a project-list push read no change set and no branch", async () => {
  const { state } = loadShell(VISIBLE);
  state.checkColumnDesk = async () => {};
  const git = [];
  state.loadChanges = async () => git.push("changes");
  state.loadSync = async () => git.push("sync");
  state.openSlug = "o/r";
  state.fleetPeers = [{ daemon_id: "d" }];
  await withFetchSpy(async () => {
    state.peerTick();
    state.onPresencePush("repos.dirty", {});
    state.onPresencePush("peers.dirty", {});
  });
  assert.deepEqual(git, []);
  // NEGATIVE CONTROL: the tab becoming visible does read them.
  await withFetchSpy(() => state.onTabVisible());
  assert.ok(git.includes("changes") && git.includes("sync"), git.join(", "));
});

test("a project read keeps the peer rows and the live dots until the fleet and the sessions answer", async () => {
  const { state } = loadShell(VISIBLE);
  const peer = { key: "d/x", slug: "x", daemon: "d", state: "idle" };
  state.projects = [{ slug: "a", state: "working", env: "wsl" }, peer];
  state._fleetRows = [peer];
  const realFetch = globalThis.fetch;
  // `/api/repos` answers; the fleet and the sessions never do.
  globalThis.fetch = (url) =>
    String(url) === "/api/repos"
      ? Promise.resolve({ ok: true, status: 200, json: async () => [{ slug: "a", reachable: true }] })
      : new Promise(() => {});
  try {
    await state.loadRepos({ git: false });
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    state.projects.map((p) => p.slug),
    ["a", "x"],
    "the peer row stays",
  );
  assert.equal(state.projects[0].state, "working", "the live dot stays");
  assert.equal(state.projects[0].env, "wsl");
});

test("a commit draft does not hold the build reload", () => {
  const t = skewShell();
  t.state.commitMsg = "wip: half a message";
  t.beat({ uptime_secs: 1, build: "B" });
  assert.equal(t.reloads(), 1);
});

test("the desk hold ends right before the build reload, so the saved work's desk changes go out", () => {
  const { state, window } = loadShell(VISIBLE);
  const events = [];
  window.location.reload = () => events.push("reload");
  // The sink is one module per document: its `setHold` is replaced for this
  // test only.
  const realSetHold = WBDeskSink.setHold;
  WBDeskSink.setHold = (on) => events.push(`hold:${on}`);
  try {
    let dirty = true;
    window.WBViewer.anyDirty = () => dirty;
    window.WBNotes.anyDirty = () => false;
    let beat = null;
    window.WBDaemon.subscribePresence = (onPresence) => {
      beat = onPresence;
      return { resume() {}, close() {} };
    };
    state.pageBuild = "A";
    state.subscribePresence();
    beat({ uptime_secs: 1, build: "B" });
    assert.deepEqual(events, ["hold:true"], "unsaved work: held, no reload");
    dirty = false;
    beat({ uptime_secs: 3, build: "B" });
    assert.deepEqual(events, ["hold:true", "hold:false", "reload"]);
  } finally {
    WBDeskSink.setHold = realSetHold;
  }
});

test("a failed first change-set or branch read says why", async () => {
  const changes = observedShell([{ status: "error", message: "git exited 128" }]);
  await changes.loadChanges("o/r");
  assert.match(changes.changesReadError["o/r"], /Could not read the changes: git exited 128/);
  const sync = observedShell([{ status: "error", message: "not a git repository" }]);
  await sync.loadSync("o/r");
  assert.match(sync.syncByProject["o/r"].note, /not a git repository/);
});

// ADR-0070 D3: each write action asks the lock itself, not only its button.
test("every change-set write is refused while the change set is not current", async () => {
  const { state, window } = loadShell();
  const replies = [CHANGES_OK, { status: "error", message: "git exited 128" }];
  const verbs = [];
  window.WBDaemon.observe = async (verb) => {
    verbs.push(verb);
    return replies.shift() ?? { status: "ok" };
  };
  state.openSlug = "o/r";
  state._flashAction = () => {};
  state.runsByProject["o/r"] = [];
  await state.loadChanges("o/r");
  await state.loadChanges("o/r");
  assert.equal(state.writeLocked(), true);
  verbs.length = 0;
  state.askConfirm = async () => true;
  state.commitMsgSlug = "o/r";
  state.commitMsg = "msg";
  await state.stagePaths("o/r", ["a.txt"]);
  await state.unstagePaths("o/r", ["a.txt"]);
  await state.discardRow("o/r", { path: "a.txt", index: " ", worktree: "M" });
  await state.commitStaged("o/r");
  await state.syncFetch("o/r");
  await state.syncPull("o/r");
  await state.syncPush("o/r");
  assert.deepEqual(verbs, [], "no write reached the daemon");
});

// NEGATIVE CONTROL for the refusal above: with a current change set, each
// action does reach the daemon, so the refusal is the lock and not a missing
// argument.
test("every change-set write reaches the daemon while the change set is current", async () => {
  const acts = {
    stage: (s) => s.stagePaths("o/r", ["a.txt"]),
    unstage: (s) => s.unstagePaths("o/r", ["a.txt"]),
    discard: (s) => s.discardRow("o/r", { path: "a.txt", index: " ", worktree: "M" }),
    commit: (s) => s.commitStaged("o/r"),
    fetch: (s) => s.syncFetch("o/r"),
    pull: (s) => s.syncPull("o/r"),
    push: (s) => s.syncPush("o/r"),
  };
  for (const [name, act] of Object.entries(acts)) {
    const { state, window } = loadShell();
    const replies = [CHANGES_OK];
    const verbs = [];
    window.WBDaemon.observe = async (verb) => {
      verbs.push(verb);
      return replies.shift() ?? { status: "ok" };
    };
    state.openSlug = "o/r";
    state._flashAction = () => {};
    state.runsByProject["o/r"] = [];
    await state.loadChanges("o/r");
    assert.equal(state.writeLocked(), false, name);
    verbs.length = 0;
    state.askConfirm = async () => true;
    state.askPush = async () => true;
    state.commitMsgSlug = "o/r";
    state.commitMsg = "msg";
    await act(state);
    assert.ok(verbs.length > 0, `${name} reached the daemon`);
  }
});

test("a reopened socket refreshes an open board, as the tab becoming visible does", () => {
  const { window } = loadShell();
  const ask = (trigger) =>
    window.WBKanban.shouldRefresh({ trigger, sinceMs: 60 * 60 * 1000, boardOpen: true, docVisible: true });
  assert.equal(ask("reopen"), true);
  assert.equal(ask("reopen"), ask("visible"));
});

// ADR-0070 D4: a peer the daemon cannot read lists no project, so the sidebar
// says so instead of showing a fleet with that peer missing.
test("a peer the daemon could not read is named in the sidebar", async () => {
  const { state } = loadShell(VISIBLE);
  state.projects = [];
  const restore = scriptedFetch([
    {
      status: 200,
      body: {
        peers: [
          { daemon_id: "ok", state: "online" },
          { daemon_id: "malformed:/p/peers", state: "malformed", name: "/p/peers", diagnosis: "cannot list /p/peers: access denied" },
        ],
        repos: [],
      },
    },
  ]);
  try {
    await state.loadFleet();
  } finally {
    restore();
  }
  assert.equal(state.fleetRejectNote, "Could not read a peer: cannot list /p/peers: access denied");
  // NEGATIVE CONTROL: a fleet with no bad peer says nothing.
  assert.equal(state.fleetRejectText([{ state: "online" }]), "");
});

// The context menu builds each item from elements: a label can name a file,
// and a file name such as `<img src=x>` must reach the menu as text.
test("renderMenu sets a label as text, never as markup", () => {
  const created = [];
  const element = (tag) => {
    const el = {
      tag,
      children: [],
      style: {},
      className: "",
      textContent: "",
      innerHTMLWrites: [],
      append(...kids) {
        this.children.push(...kids);
      },
      set innerHTML(v) {
        this.innerHTMLWrites.push(v);
      },
    };
    created.push(el);
    return el;
  };
  const menu = element("div");
  const { state } = loadShell({
    document: { getElementById: () => menu, createElement: element },
  });
  // `renderMenu` reads the BARE viewport globals a browser has; lend them.
  globalThis.innerWidth = 1440;
  globalThis.innerHeight = 900;
  try {
    state.renderMenu(0, 0, [{ icon: "bi-x", label: "<img src=x>", run() {} }]);
  } finally {
    delete globalThis.innerWidth;
    delete globalThis.innerHeight;
  }
  const button = menu.children[0];
  assert.equal(button.tag, "button");
  const [icon, label] = button.children;
  assert.equal(icon.tag, "i");
  assert.equal(icon.className, "bi bi-x");
  assert.equal(label.tag, "span");
  assert.equal(label.textContent, "<img src=x>");
  assert.ok(!created.some((el) => el.tag === "img"), "an img element was created");
  for (const el of created) {
    for (const w of el.innerHTMLWrites) {
      assert.ok(!w.includes("<img"), `innerHTML took the label: ${w}`);
    }
  }
});

test("Remove project names a remoteless repo by its folder, not its path- slug", async () => {
  const { state } = loadShell();
  const asked = [];
  state.askConfirm = async (o) => {
    asked.push(o.message);
    return false;
  };
  await state.removeProject({ slug: "path-8ee0b8b587ea7891", name: "widget", path: "/home/me/widget/" });
  await state.removeProject({ slug: "owner/repo", path: "/home/me/elsewhere" });
  assert.deepEqual(asked, [
    "Remove “widget” from Ralphy? Files on disk are kept.",
    "Remove “owner/repo” from Ralphy? Files on disk are kept.",
  ]);
});

test("projectLabel and projectTitle never print a path- key or a daemon id", () => {
  const { state } = loadShell();
  state.projects = [
    { slug: "path-8ee0b8b587ea7891", name: "widget", path: "C:/Dev/widget" },
    { key: "01KYPEER/path-1234", slug: "path-1234", name: "gadget", path: "/home/me/gadget", daemon: "01KYPEER", env: "WSL: Ubuntu" },
    { slug: "owner/repo", name: "owner/repo", path: "C:/Dev/repo" },
  ];
  const ref = (p) => state.repoRef(p);
  const [local, peer, forge] = state.projects;
  assert.equal(state.projectLabel(ref(local)), "widget");
  assert.equal(state.projectTitle(ref(local)), "C:/Dev/widget");
  assert.equal(state.projectLabel(ref(peer)), "gadget · WSL: Ubuntu");
  assert.equal(state.projectTitle(ref(peer)), "/home/me/gadget · WSL: Ubuntu");
  assert.equal(state.projectLabel(ref(forge)), "owner/repo");
  assert.equal(state.projectTitle(ref(forge)), "owner/repo");
  state.openSlug = ref(local);
  assert.equal(state.consoleMenuRepoName(), "widget");
  assert.equal(state.columnRepoLabel(ref(peer)), "gadget");
});

test("a read failure shows the words for a daemon code, never the code", async () => {
  const state = observedShell([
    { status: "error", message: "unknown repo" },
    { status: "error", message: "unknown checkout" },
    { status: "error", reason: "unknown repo" },
  ]);
  await state.loadChanges("o/r");
  assert.equal(state.changesReadError["o/r"], "Could not read the changes: the project is not in the list");
  await state.loadSync("o/r");
  assert.equal(state.syncByProject["o/r"].note, "Could not read the branch: the worktree does not exist");
  state.runsOpen = false;
  await state.hydrateRuns();
  assert.equal(state.runsError, "Could not read the runs: the project is not in the list.");
});

test("an unreadable run is counted, and its id is never shown", async () => {
  const state = observedShell([
    { status: "ok", runs: [], unreadable: [{ runid: "01JX4ABCDEF", reason: "malformed" }] },
  ]);
  state.runsOpen = false;
  await state.hydrateRuns();
  assert.equal(
    state.runsError,
    "Could not read 1 saved run. The file is damaged or from another version of Ralphy.",
  );
});

test("run and peer states show as words, and the dot says what it means", () => {
  const { state } = loadShell();
  assert.equal(state.runStateWord("sleep"), "usage limit — sleeping");
  assert.equal(state.issueRunningLabel({ state: "hitl", agent: "claude" }), "Running · waiting on human (claude)");
  assert.equal(state.peerStateWord("version-mismatch"), "version mismatch");
  assert.equal(state.peerStateWord("asleep"), "asleep");
  assert.equal(state.dotTitle("offline"), "The folder cannot be reached");
  assert.equal(state.dotTitle("idle"), "No console is open");
});
