// Unit tests for assets/ui/wb-wording.ts: the run catalog's issue statuses.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { issueStatus, statusWording } from "../assets/ui/wb-wording.ts";

const catalog = JSON.parse(
  readFileSync(new URL("../../../assets/wording/run.json", import.meta.url), "utf8"),
);
const statuses = catalog.values["issue-status"];

test("statusWording gives the catalog text and icon of each issue status", () => {
  const keys = Object.keys(statuses);
  assert.equal(keys.length, 10);
  for (const name of keys) {
    const key = issueStatus(name);
    assert.equal(key, name);
    const wording = statusWording(key);
    assert.equal(wording.text, statuses[name].text);
    assert.equal(wording.icon, statuses[name].icon);
  }
});

test("issueStatus gives null for a name the catalog lacks", () => {
  assert.equal(issueStatus("nope"), null);
  assert.equal(issueStatus("toString"), null);
  assert.equal(issueStatus("run-outcome"), null);
});
