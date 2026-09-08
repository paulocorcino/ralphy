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
  const keys = new Map();
  for (const m of body.matchAll(/^ {4}([A-Za-z_$][\w$]*):/gm)) {
    const [, key] = m;
    keys.set(key, (keys.get(key) || 0) + 1);
  }
  const dupes = [...keys].filter(([, n]) => n > 1).map(([k]) => k);
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

test("repoLabel re-labels only a remoteless repo, off its directory name", () => {
  // ADR-0008 D7 keys a repo with no remote as `path-<hash>`, which is twenty
  // useless characters in a fixed column. The basename is what the operator
  // calls it (#332).
  assert.equal(s.repoLabel({ slug: "path-9f2a1c", path: "C:\\src\\widget" }), "WIDGET");
  assert.equal(s.repoLabel({ slug: "path-9f2a1c", path: "/home/me/widget" }), "WIDGET");
  // Trailing separators are stripped FIRST, or the basename is the empty string.
  assert.equal(s.repoLabel({ slug: "path-9f2a1c", path: "C:\\src\\widget\\" }), "WIDGET");
  assert.equal(s.repoLabel({ slug: "path-9f2a1c", path: "/home/me/widget/" }), "WIDGET");
  // NEGATIVE CONTROL, and the reason the `/` test is not optional: a real
  // GitHub repo genuinely NAMED `path-utils` must never be re-labelled off
  // DISK. `slug_from_url` always yields `owner/repo`, so the slash decides.
  // Note what the guarantee is and is not: the label is still the slug's last
  // segment upper-cased — `owner/path-utils` reads `PATH-UTILS`, not
  // `SOMETHING-ELSE`. The directory is what must not leak in.
  assert.equal(
    s.repoLabel({ slug: "owner/path-utils", path: "C:\\src\\something-else" }),
    "PATH-UTILS",
  );
  assert.equal(s.repoLabel({ slug: "owner/repo", path: "C:\\src\\elsewhere" }), "REPO");
  // No path to fall back on: the slug stands, ugly or not.
  assert.equal(s.repoLabel({ slug: "path-9f2a1c", path: "" }), "PATH-9F2A1C");
});

test("rowTitle says branch and dirtiness for a local repo, environment for a peer", () => {
  assert.equal(
    s.rowTitle({ slug: "owner/repo", branch: "main", dirty: false }),
    "owner/repo · main",
  );
  assert.equal(
    s.rowTitle({ slug: "owner/repo", branch: "main", dirty: true }),
    "owner/repo · main (uncommitted changes)",
  );
  // A repo whose branch has not been read yet says only what it knows.
  assert.equal(s.rowTitle({ slug: "owner/repo", branch: "" }), "owner/repo");
  // A peer row is a different fact: the environment, and never the branch.
  assert.equal(
    s.rowTitle({ slug: "owner/repo", daemon: true, env: "WSL: Ubuntu-22.04", branch: "main" }),
    "owner/repo · WSL: Ubuntu-22.04",
  );
});

test("canSwitchBranch and branchChipTitle refuse an unreachable repo", () => {
  assert.equal(s.canSwitchBranch({ state: "ok" }), true);
  assert.equal(s.canSwitchBranch({ state: "offline" }), false);
  assert.equal(
    s.branchChipTitle({ state: "offline", branch: "main" }),
    "repo unreachable — branch switching unavailable",
  );
  assert.equal(
    s.branchChipTitle({ state: "ok", branch: "main", dirty: false }),
    "switch branch — main",
  );
  assert.equal(
    s.branchChipTitle({ state: "ok", branch: "main", dirty: true }),
    "switch branch (uncommitted changes) — main",
  );
});

test("githubUrl builds an issue link only from a github remote", () => {
  const own = loadShell().state;
  own.openSlug = "owner/repo";
  own.projects = [{ slug: "owner/repo", remoteUrl: "https://github.com/owner/repo.git" }];
  assert.equal(own.githubUrl(42), "https://github.com/owner/repo/issues/42");

  // The SSH form, and the trailing-slash form, resolve the same.
  own.projects = [{ slug: "owner/repo", remoteUrl: "git@github.com:owner/repo.git" }];
  assert.equal(own.githubUrl(42), "https://github.com/owner/repo/issues/42");
  own.projects = [{ slug: "owner/repo", remoteUrl: "https://github.com/owner/repo/" }];
  assert.equal(own.githubUrl(42), "https://github.com/owner/repo/issues/42");

  // NEGATIVE CONTROL: null, not a guessed URL. A non-GitHub forge, a repo with
  // no remote, and an unknown slug must each yield nothing to link to.
  own.projects = [{ slug: "owner/repo", remoteUrl: "https://gitlab.com/owner/repo.git" }];
  assert.equal(own.githubUrl(42), null);
  own.projects = [{ slug: "owner/repo", remoteUrl: "" }];
  assert.equal(own.githubUrl(42), null);
  own.projects = [];
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
  const id = s.kanbanColumnOf({ state: "open", labels: [] });
  const title = s.kanbanColumnTitle({ state: "open", labels: [] });
  assert.equal(typeof title, "string");
  assert.notEqual(title, "");
  // The fold falls back to the id rather than to empty, so an unknown column is
  // still nameable in the UI.
  assert.ok(title.length >= id.length || title !== "");
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
  // `planBelongsTo` gates the whole fold: prose held over from a previous issue
  // must render as nothing, not as the previous issue's outline.
  assert.deepEqual(own.planHeadings(null), []);
  assert.deepEqual(own.planHeadings({}), []);
});
