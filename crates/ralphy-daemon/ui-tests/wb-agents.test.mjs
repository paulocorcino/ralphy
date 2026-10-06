// Unit tests for assets/ui/wb-agents.ts — imports the real module with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBAgents } from "../assets/ui/wb-agents.ts";

// The fold keeps no state, so one import serves every test.
function load() {
  return WBAgents;
}

const NEEDS_REPO = "Open a project before you start an agent.";

const ROSTER = [
  { id: "claude", label: "claude", accelerator: "1" },
  { id: "codex", label: "codex", accelerator: "2" },
  { id: "gemini", label: "gemini", accelerator: "7" },
];

test("roster order is preserved and the console row comes last on digit 0", () => {
  const rows = load().menuRows({
    roster: ROSTER,
    sessions: [],
    openSlug: "repo",
  });
  assert.deepEqual(
    rows.map((r) => r.kind),
    ["claude", "codex", "gemini", "console"],
  );
  assert.deepEqual(
    rows.map((r) => r.digit),
    ["1", "2", "7", "0"],
  );
  const last = rows[rows.length - 1];
  assert.equal(last.plain, true);
  assert.equal(last.label, "console");
});

test("an empty roster still yields the plain console row", () => {
  const rows = load().menuRows({ roster: [], sessions: [], openSlug: "x" });
  assert.equal(rows.length, 1);
  assert.equal(rows[0].kind, "console");
  const falsy = load().menuRows({ sessions: [], openSlug: "x" });
  assert.equal(falsy.length, 1);
  assert.equal(falsy[0].kind, "console");
});

test("with no open repo every agent row is disabled and says why; the console row is not", () => {
  const rows = load().menuRows({ roster: ROSTER, sessions: [], openSlug: "" });
  for (const row of rows.filter((r) => !r.plain)) {
    assert.equal(row.disabled, true, `${row.kind} must be disabled`);
    assert.equal(row.title, NEEDS_REPO);
  }
  const console_ = rows.find((r) => r.plain);
  assert.equal(console_.disabled, false);
  assert.equal(console_.title, "");
});

test("live counts are scoped to the open repo and the row's agent, and stay a readout", () => {
  const rows = load().menuRows({
    roster: ROSTER,
    sessions: [
      { id: 7, agent: "claude", repo: "mine" },
      { id: 3, agent: "claude", repo: "mine" },
      { id: 9, agent: "claude", repo: "other" },
      { id: 11, agent: "console", repo: "mine" },
    ],
    openSlug: "mine",
  });
  const api = load();
  const claude = rows.find((r) => r.kind === "claude");
  assert.equal(claude.live, 2);
  // The menu is "New console": a live row still LAUNCHES on click, and the
  // row carries nothing that would let a click become an attach.
  assert.equal(api.consoleIntent(claude), "launch");
  assert.equal("action" in claude, false);
  assert.equal("sessionId" in claude, false);

  const codex = rows.find((r) => r.kind === "codex");
  assert.equal(codex.live, 0);
  assert.equal(api.consoleIntent(codex), "launch");

  // The plain console counts its own sessions, not the agents'.
  const plain = rows.find((r) => r.plain);
  assert.equal(plain.live, 1);
  assert.equal(api.consoleIntent(plain), "launch");
});
// The plain console row counts the bare shells only: a home-dir console
// counts (its `openSlug || "~"` scope is the daemon's own label for a
// repo-less shell), and a console started from the row's "Run…" field is
// labelled by its command, so it does not.
test("the plain row's live count is the bare shells of its scope", () => {
  // [case, sessions, open slug, expected live count]
  const rows = [
    ["a home-dir console", [{ id: 5, agent: "console", repo: "~" }], "", 1],
    [
      "a command console is not a bare shell",
      [
        { id: 3, agent: "console", repo: "repo", kind: "console" },
        { id: 4, agent: "htop", repo: "repo", kind: "console" },
      ],
      "repo",
      1,
    ],
  ];
  for (const [name, sessions, openSlug, want] of rows) {
    const menu = load().menuRows({ roster: ROSTER, sessions, openSlug });
    const plain = menu.find((r) => r.plain);
    assert.equal(plain.kind, "console", name);
    assert.equal(plain.live, want, name);
  }
});

test("a vendor the frontend has never heard of renders from the roster alone", () => {
  const rows = load().menuRows({
    roster: [{ id: "newvendor", label: "newvendor", accelerator: "8" }],
    sessions: [],
    openSlug: "mine",
  });
  const row = rows.find((r) => r.kind === "newvendor");
  assert.ok(row, "an unknown roster row must still produce a menu row");
  assert.equal(row.digit, "8");
  assert.equal(row.disabled, false);
  assert.equal(load().consoleIntent(row), "launch");
});

test("an unavailable row remains visible and disabled, while try-anyway still launches", () => {
  const api = load();
  const rows = api.menuRows({
    roster: [
      {
        id: "opencode",
        label: "opencode",
        accelerator: "3",
        available: false,
        reason: "not installed here",
      },
    ],
    sessions: [],
    openSlug: "peer/repo",
  });
  const row = rows.find((item) => item.kind === "opencode");
  assert.ok(row, "unavailable adapters remain in the roster");
  assert.equal(row.disabled, true);
  assert.equal(row.unavailable, true);
  assert.equal(row.title, "Not installed here.");
  assert.equal(row.tryAnyway, true);
  assert.equal(api.canLaunch(row), false);
  assert.equal(api.canLaunch(row, true), true);
  assert.equal(api.consoleIntent(row), null);
  assert.equal(api.consoleIntent(row, { tryAnyway: true }), "launch");
});

test("no-repo disablement outranks availability and cannot be bypassed", () => {
  const api = load();
  const row = api.menuRows({
    roster: [
      {
        id: "opencode",
        accelerator: "3",
        available: false,
        reason: "not installed here",
      },
    ],
    sessions: [],
    openSlug: "",
  })[0];
  assert.equal(row.needsRepo, true);
  assert.equal(row.title, NEEDS_REPO);
  assert.equal(row.tryAnyway, false);
  assert.equal(api.canLaunch(row, true), false);
  assert.equal(api.consoleIntent(row, { tryAnyway: true }), null);
});

test("repo-specific roster state replaces old rows and signals run-picker availability", () => {
  const api = load();
  const local = api.rosterState(
    [{ id: "claude", accelerator: "1", available: true }],
    "local/repo",
  );
  const peer = api.rosterState(
    [
      {
        id: "codex",
        accelerator: "2",
        available: false,
        reason: "not installed here",
      },
    ],
    "peer/repo",
  );
  assert.deepEqual(local.roster.map((row) => row.id), ["claude"]);
  assert.deepEqual(peer.roster.map((row) => row.id), ["codex"]);
  assert.equal(peer.repo, "peer/repo");
  assert.deepEqual(peer.agents, [
    {
      id: "codex",
      label: "codex",
      available: false,
      title: "Not installed here.",
    },
  ]);
  assert.equal(api.rosterUrl("peer/repo"), "/api/agents?repo=peer%2Frepo");
});

test("menuRows mutates neither argument", () => {
  const roster = structuredClone(ROSTER);
  const sessions = [
    { id: 2, agent: "claude", repo: "mine" },
    { id: 4, agent: "codex", repo: "mine" },
  ];
  const rosterBefore = structuredClone(roster);
  const sessionsBefore = structuredClone(sessions);
  load().menuRows({ roster, sessions, openSlug: "mine" });
  assert.deepEqual(roster, rosterBefore);
  assert.deepEqual(sessions, sessionsBefore);
});

test("runCommand trims the line, and a blank line runs nothing", () => {
  const { runCommand } = load();
  assert.equal(runCommand("  ping -c 2 127.0.0.1 "), "ping -c 2 127.0.0.1");
  for (const text of [undefined, null, "", "   ", 7]) {
    assert.equal(runCommand(text), null, `${JSON.stringify(text)} must run nothing`);
  }
});
