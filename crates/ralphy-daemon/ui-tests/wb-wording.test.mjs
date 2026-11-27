// Unit tests for assets/ui/wb-wording.ts: the run catalog's issue statuses (the wording itself is pinned by wb-runs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { issueStatus } from "../assets/ui/wb-wording.ts";

test("issueStatus gives null for a name the catalog lacks", () => {
  assert.equal(issueStatus("nope"), null);
  assert.equal(issueStatus("toString"), null);
  assert.equal(issueStatus("run-outcome"), null);
});
