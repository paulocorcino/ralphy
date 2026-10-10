// Unit tests for assets/ui/wb-console-name.ts — the console name (ADR-0066
// §§2–4). Runs the real source against an empty window: the module is pure
// and touches nothing at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBConsoleName } from "../assets/ui/wb-console-name.ts";

function load() {
  return WBConsoleName;
}

test("prefixOf: the last slug segment, home for no repo", () => {
  const N = load();
  assert.equal(N.prefixOf("owner/fincal"), "fincal");
  assert.equal(N.prefixOf("~"), "home");
  assert.equal(N.prefixOf(""), "home");
  assert.equal(N.prefixOf(null), "home");
  assert.equal(N.prefixOf("acme/api"), "api");
  assert.equal(N.prefixOf("other/api"), "api");
});

test("defaultName: the lowest free number of the exact form", () => {
  const N = load();
  assert.equal(N.defaultName("fincal", []), "fincal #1");
  assert.equal(N.defaultName("fincal", ["fincal #1", "fincal #3", "fincal #6"]), "fincal #2");
  assert.equal(N.defaultName("fincal", ["backend", "fincal #1"]), "fincal #2");
  assert.equal(N.defaultName("fincal", ["fincal #01"]), "fincal #1", "a leading zero is not #1");
  assert.equal(N.defaultName("a.b", ["axb #1"]), "a.b #1", "the prefix is not a pattern");
});

test("renameValue: trim, cut to 40, default when empty, duplicates allowed", () => {
  const N = load();
  assert.equal(N.renameValue("  x  ", "fincal", []), "x");
  assert.equal(N.renameValue("   ", "fincal", ["fincal #1"]), "fincal #2");
  assert.equal(N.renameValue("", "fincal", []), "fincal #1");
  assert.equal(N.renameValue("a".repeat(41), "fincal", []), "a".repeat(40));
  const out = N.renameValue("🚀".repeat(41), "fincal", []);
  assert.equal(Array.from(out).length, 40, "the cut is by code point");
  assert.equal(N.renameValue("fincal #1", "fincal", ["fincal #1"]), "fincal #1");
});

test("consoleLabel and labelParts: one builder, name then label", () => {
  const N = load();
  assert.equal(N.consoleLabel("fincal #1", "claude"), "fincal #1 (claude)");
  assert.deepEqual(N.labelParts("home #1", "console"), { name: "home #1", tag: "(console)" });
  // A member a detached fence's popup sent with no agent.
  assert.equal(N.consoleLabel("home #2", null), "home #2 (console)");
});

test("tooltipLines: ref, environment, session name, empty ones left out", () => {
  const N = load();
  const ref = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/fincal";
  const env = "WSL: Ubuntu-22.04";
  const sess = "wb-fincal-1";
  assert.deepEqual(N.tooltipLines(ref, env, sess), [ref, env, sess]);
  assert.deepEqual(N.tooltipLines(null, env, sess), [env, sess]);
  assert.deepEqual(N.tooltipLines(ref, null, sess), [ref, sess]);
  assert.deepEqual(N.tooltipLines(ref, env, null), [ref, env]);
  assert.deepEqual(N.tooltipLines("~", "Windows", null), ["Windows"]);
});

test("nameDesk: desk order, named records kept, same result twice, no mutation", () => {
  const N = load();
  const prefix = (r) => N.prefixOf(r === "~" ? "~" : r.split("/").pop());
  const desk = [
    { id: "a", repo: "owner/fincal" },
    { id: "b", repo: "~" },
    { id: "c", repo: "owner/fincal", consoleName: "fincal #1" },
    { id: "d", repo: "owner/fincal" },
  ];
  const before = JSON.stringify(desk);
  const out = N.nameDesk(desk, prefix);
  assert.deepEqual(
    out.map((r) => r.consoleName),
    ["fincal #2", "home #1", "fincal #1", "fincal #3"],
  );
  assert.deepEqual(N.nameDesk(out, prefix), out, "idempotent");
  assert.equal(JSON.stringify(desk), before, "the input is unchanged");
});

test("nameDesk: one count per prefix across repos, kinds and environments", () => {
  const N = load();
  const prefix = (r) => N.prefixOf(r.split("/").pop());
  const out = N.nameDesk(
    [
      { id: "a", repo: "acme/api", agent: "claude" },
      { id: "b", repo: "other/api", agent: "" },
      { id: "c", repo: "01KY0000000000000000000000/acme/api", agent: "codex" },
    ],
    prefix,
  );
  assert.deepEqual(out.map((r) => r.consoleName), ["api #1", "api #2", "api #3"]);
});
