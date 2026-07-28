// Unit tests for assets/ui/wb-agents.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-agents.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBAgents;
}

const NEEDS_REPO = "select a repo first — an agent needs one to work in";

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

test("live counts are scoped to the open repo and the row's agent; attach targets the lowest id", () => {
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
  const claude = rows.find((r) => r.kind === "claude");
  assert.equal(claude.live, 2);
  assert.equal(claude.action, "attach");
  assert.equal(claude.sessionId, 3);

  const codex = rows.find((r) => r.kind === "codex");
  assert.equal(codex.live, 0);
  assert.equal(codex.action, "launch");
  assert.equal(codex.sessionId, null);

  // The plain console counts its own sessions, not the agents' — but it never
  // offers to reach one, because `openConsoleItem` always launches a free shell.
  const plain = rows.find((r) => r.plain);
  assert.equal(plain.live, 1);
  assert.equal(plain.action, "launch");
  assert.equal(plain.sessionId, null);
});

test("the plain console row never advertises an action it does not take", () => {
  // Its `openSlug || "~"` scope is the daemon's own label for a repo-less shell.
  const rows = load().menuRows({
    roster: ROSTER,
    sessions: [{ id: 5, agent: "console", repo: "~" }],
    openSlug: "",
  });
  const plain = rows.find((r) => r.plain);
  assert.equal(plain.live, 1, "a home-dir console counts against the plain row");
  assert.equal(plain.action, "launch");
  assert.equal(plain.sessionId, null);
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
  assert.equal(row.action, "launch");
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
