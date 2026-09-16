// Characterization tests for assets/ui/app.js — the `shell()` component.
//
// These exist to be written BEFORE app.js is split, not after. A split of the
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

// One load, many reads. `loadShell()` is ~20ms of evaluation and every test in
// this file only READS from the state, so they share one. A test that mutates
// must take its own — see `boardRowToIssue` below, which does.
const { state: s } = loadShell();

test("shell() builds the whole component off an empty document", () => {
  // The premise every other test here rests on: none of the state literal's
  // ~390 keys needs a rendered DOM to exist. If this ever fails, a fold moved
  // DOM work into construction and the split has a new constraint.
  assert.equal(typeof s, "object");
  assert.ok(Object.keys(s).length > 300, "the component is the state literal");
});

// NEGATIVE CONTROL for the whole file: a duplicate key in the state literal is
// silently legal in sloppy mode and the LAST one wins, which is how `app.js`
// carried two incompatible `changesError` declarations. `Object.keys()` cannot
// see the loser, so this reads the source.
test("the state literal declares no key twice", () => {
  const text = readFileSync(join(UI, "app.js"), "utf8");
  const body = text.slice(text.indexOf("function shell()"));
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

test("projectBadge carries a read failure into the badge, per project", () => {
  const own = loadShell().state;
  own.changesCount = { "owner/a": 3, "owner/b": null };
  own.changesReadError = { "owner/b": "could not read changes" };

  assert.deepEqual(own.projectBadge("owner/a"), {
    show: true,
    text: "3",
    zero: false,
    title: "3 changed",
  });
  // The failure the shell could not report until `changesReadError` stopped
  // colliding with the shell-wide flash string: a failed read shows `—` with
  // the reason in the title, and never reads like a clean tree.
  assert.deepEqual(own.projectBadge("owner/b"), {
    show: true,
    text: "—",
    zero: false,
    title: "could not read changes",
  });
  // NEGATIVE CONTROL: an unread project shows NOTHING — not a zero, which would
  // claim a clean tree nobody looked at.
  assert.deepEqual(own.projectBadge("owner/never-read"), {
    show: false,
    text: "",
    zero: false,
    title: "",
  });
  // And a genuine zero is a zero.
  own.changesCount["owner/c"] = 0;
  assert.equal(own.projectBadge("owner/c").zero, true);
});

test("the shell-wide changes flash is a STRING, and the per-project errors are a MAP", () => {
  // The two are different facts and they now have different names. This states
  // the shapes so the next reader cannot re-merge them: one describes the act
  // just dispatched, the other describes each project's last read.
  assert.equal(typeof s.changesError, "string");
  assert.equal(typeof s.changesReadError, "object");
  assert.notEqual(s.changesReadError, null);
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
  // The pure URL half moved to `wb-project.js` and is tested there. This is the
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
  const a = { slug: "owner/alpha", branch: "main", path: "C:\src\alpha" };
  const b = { slug: "owner/beta", branch: "main", path: "C:\src\beta" };
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
  const real = globalThis.WB;
  globalThis.WB = window.WB;
  try {
    return await run(state, calls, (a) => (answer = a));
  } finally {
    if (real === undefined) delete globalThis.WB;
    else globalThis.WB = real;
  }
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
    answer({ status: "ok", hits: [{ path: "a" }], truncated: true });
    await s.fileSearchNow();
    assert.equal(s.fileSearch.note, "First 200 matches. Narrow the search to see more.");
    answer({ status: "ok", hits: [], truncated: false });
    await s.fileSearchNow();
    assert.equal(s.fileSearch.note, "no matches");
    answer({ status: "error", reason: "unknown verb" });
    await s.fileSearchNow();
    assert.equal(s.fileSearch.note, "unknown verb");
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

// ADR-0063 §3: a NEW console opens in the checkout selected at the moment of
// the click, and in the primary when none is — the console keeps it afterwards
// (the title comes from the daemon's announcement, never from this selection).
test("newConsole opens the agent in the selected checkout and in the primary without one", () => {
  const { state } = loadShell();
  const calls = [];
  const real = globalThis.WBConsole;
  // `app.js` reaches `WBConsole` as a bare global; the real one needs a DOM.
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
    { repo: "o/r", agent: "claude", checkout: "wt-a" },
    { repo: "o/r", agent: "codex", checkout: null },
  ]);
});
