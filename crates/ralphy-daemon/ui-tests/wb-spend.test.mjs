// Unit tests for assets/ui/wb-spend.js — runs the real source with no DOM.
// This file lives OUTSIDE assets/ui on purpose: lib.rs embeds all of
// assets/ui into the daemon binary via include_dir!, so a test there would ship.
//
// The Rust side pins this module's SOURCE TEXT (usage.rs); text pins survive an
// inverted condition. These are the behavioural half — every assertion below
// reds against a plausibly-broken implementation, which is the point of having
// both.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-spend.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBSpend;
}

const WB = load();

// One ledger row in the shape `/api/usage?project=` serves.
function row(over = {}) {
  return {
    project: "acme/widget",
    issue: 251,
    phase: "execute",
    agent: "claude",
    model: "claude-opus-4-8",
    outcome: "done",
    actor_name: "dev",
    ralphy_version: "0.0.0",
    ts: "2026-07-30T12:00:00+00:00",
    tokens: { input: 10, output: 1, cache_read: 2, cache_creation: 3 },
    ...over,
  };
}

function ledger(over = {}) {
  return WB.ledger({ project: "acme/widget", records: [], interactive: [], ...over });
}

test("a ledger row maps every column, with the four counts RAW", () => {
  const [r] = ledger({ records: [row()] }).rows;
  assert.equal(r.kind, "ledger");
  assert.equal(r.issue, "#251");
  assert.equal(r.actor, "dev");
  assert.equal(r.version, "0.0.0");
  assert.equal(r.when, "2026-07-30T12:00:00+00:00");
  // Unabbreviated and unrounded: the daemon's `k`/`M` vocabulary is a SUMMARY
  // one, and the grid exists for the detailed read.
  assert.deepEqual(r.tokens, {
    input: "10",
    cache_read: "2",
    cache_creation: "3",
    output: "1",
  });
});

test("an interactive row reads `—` for the four fields it has no counterpart for", () => {
  const [r] = ledger({
    interactive: [
      {
        project: "acme/widget",
        agent: "cursor",
        model: "composer-2.5",
        session_id: "i1",
        tokens: null,
        last_ts: "2026-07-30T11:00:00+00:00",
        unpriced_cause: "unmetered",
      },
    ],
  }).rows;
  assert.equal(r.kind, "interactive");
  assert.deepEqual([r.issue, r.phase, r.outcome, r.version], ["—", "—", "—", "—"]);
  assert.equal(r.when, "2026-07-30T11:00:00+00:00");
  // `tokens: null` is "the vendor keeps no count anywhere" (ADR-0042 D11) and
  // must never render as `0`, which would claim a measurement nobody made.
  assert.deepEqual(Object.values(r.tokens), ["—", "—", "—", "—"]);
  assert.equal(r.unpriced, "unmetered");
});

test("a lower_bound row marks EVERY count and says so in words (ADR-0043 D10)", () => {
  const [floor] = ledger({ records: [row({ lower_bound: true })] }).rows;
  assert.deepEqual(Object.values(floor.tokens), ["≥ 10", "≥ 2", "≥ 3", "≥ 1"]);
  assert.equal(floor.boundNote, " (lower bound)");
  assert.equal(floor.lowerBound, true);

  // The negative control — this is what reds if the condition is inverted, and
  // it is exactly what the Rust source pin cannot see.
  const [exact] = ledger({ records: [row()] }).rows;
  assert.deepEqual(Object.values(exact.tokens), ["10", "2", "3", "1"]);
  assert.equal(exact.boundNote, "");
  assert.equal(ledger({ records: [row()] }).anyLowerBound, false);
  assert.equal(ledger({ records: [row({ lower_bound: true })] }).anyLowerBound, true);
});

test("the unpriced filter keeps rows with a cause and only those", () => {
  const records = [
    row({ issue: 1 }),
    row({ issue: 2, unpriced_cause: "recoverable" }),
    row({ issue: 3, unpriced_cause: "no_price" }),
  ];
  assert.equal(ledger({ records }).rows.length, 3);
  const only = ledger({ records, unpricedOnly: true });
  assert.deepEqual(
    only.rows.map((r) => r.unpriced),
    ["recoverable", "no_price"],
  );
  assert.equal(only.unpricedOnly, true);
  // A row that priced carries the EMPTY string, never a cause word — the
  // absence is the "this one is fine" answer.
  assert.equal(ledger({ records }).rows[0].unpriced, "");
});

test("the cap keeps the MOST RECENT rows, not the first ones written", () => {
  // The ledger is served oldest-first, so slicing the head would cap a busy
  // project to its very first phase lines and hide everything recent.
  const records = Array.from({ length: WB.LEDGER_CAP + 3 }, (_, i) => row({ issue: i }));
  const view = ledger({ records });
  assert.equal(view.rows.length, WB.LEDGER_CAP);
  assert.equal(view.truncated, 3);
  assert.equal(view.rows[0].issue, "#3", "the three oldest rows are the ones dropped");
  assert.equal(view.rows[view.rows.length - 1].issue, `#${WB.LEDGER_CAP + 2}`);
  // An interactive record is appended after the run records, so head-slicing
  // would make a capped project show none at all.
  const withSession = ledger({
    records,
    interactive: [{ project: "acme/widget", agent: "claude", session_id: "i1", tokens: null }],
  });
  assert.ok(
    withSession.rows.some((r) => r.kind === "interactive"),
    "the interactive session must survive the cap",
  );
});

test("a peer's rows are counted, so the pane can name what the Overview omits", () => {
  const records = [
    row({ issue: 1, daemon_id: "local" }),
    row({ issue: 2, daemon_id: "peer" }),
  ];
  // `/api/spend` is local only (PRD #355, Out of Scope), so a peer row is in
  // this grid and in NONE of the Overview's figures.
  assert.equal(ledger({ records, daemonId: "local" }).peers, 1);
  // With no daemon id known, nothing is claimed rather than everything guessed.
  assert.equal(ledger({ records }).peers, 0);
  assert.equal(ledger({ records: [row({ daemon_id: "local" })], daemonId: "local" }).peers, 0);
});

test("the pane's states are named, and the peer banner survives every one", () => {
  const missing = [{ daemon_id: "p", environment: "WSL", why: "connecting" }];
  assert.equal(WB.ledger({ missing }).kind, WB.EMPTY);
  assert.equal(WB.ledger({ missing }).missing.length, 1, "reachable with no project open");
  assert.equal(WB.ledger({ project: "a/b", loading: true, missing }).kind, WB.LOADING);
  assert.equal(WB.ledger({ project: "a/b", error: "boom", missing }).missing.length, 1);
  assert.equal(WB.ledger({ project: "a/b", error: "boom" }).message, "boom");
  assert.deepEqual(
    WB.LEDGER_COLUMNS.map((c) => c.key),
    [
      "kind",
      "project",
      "issue",
      "phase",
      "agent",
      "model",
      "outcome",
      "actor",
      "version",
      "when",
      "input",
      "cache_read",
      "cache_creation",
      "output",
    ],
  );
});
