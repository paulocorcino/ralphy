// Unit tests for assets/ui/wb-desk-history.ts — imports the real module with no
// DOM, over the daemon's own reply (`fixtures/api-desk-history.json`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBDeskHistory } from "../assets/ui/wb-desk-history.ts";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
// The module keeps no state, so one import serves every test.
function load() {
  return WBDeskHistory;
}

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

test("each version the daemon lists is one row, newest first, with its reason and counts", () => {
  const H = load();
  const rows = H.rows(fixture("api-desk-history"), (ms) => `t${ms}`);
  assert.deepEqual(
    rows.map((r) => [r.reason, r.counts, r.when]),
    [
      ["Restored", "1 console · 0 fences · 0 notes", "t1790000100000"],
      ["Before a restore", "2 consoles · 1 fence · 0 notes", "t1790000100000"],
      ["Changed", "1 console · 0 fences · 0 notes", "t1790000000000"],
    ],
  );
  assert.deepEqual(
    rows.map((r) => r.id),
    [1790000100001, 1790000100000, 1790000000000],
  );
});

test("a reply that is not a list shows no rows", () => {
  assert.deepEqual(load().rows({ error: "x" }, String), []);
});

test("a downloaded file is named for its local save time", () => {
  const savedAt = new Date(2026, 9, 4, 9, 7).getTime();
  assert.equal(load().fileName({ savedAt }), "ralphy-desk-2026-10-04-0907.json");
});

test("an upload must be a desk version file", () => {
  const H = load();
  assert.deepEqual(H.parseUpload("{ nope"), { cause: "the file is not JSON" });
  assert.deepEqual(H.parseUpload(JSON.stringify({ kind: "other", desk: {} })), {
    cause: "the file is not a desk layout saved by Ralphy",
  });
  assert.deepEqual(H.parseUpload(JSON.stringify({ kind: H.VERSION_KIND })), {
    cause: "the file is not a desk layout saved by Ralphy",
  });
  assert.deepEqual(H.parseUpload(JSON.stringify({ kind: H.VERSION_KIND, desk: [] })), {
    cause: "the file is not a desk layout saved by Ralphy",
  });
  const file = { kind: H.VERSION_KIND, id: 1, desk: { windows: [] } };
  assert.deepEqual(H.parseUpload(JSON.stringify(file)), { version: file });
});
