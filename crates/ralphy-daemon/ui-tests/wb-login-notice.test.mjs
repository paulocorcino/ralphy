// The shared replies of the login routes, as the Rust route tests wrote them:
// the revoke reply (fixtures/api-security-logins-revoke.json) and the
// `logins.dirty` push (fixtures/logins.dirty.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

test("the revoke reply says the login is revoked", () => {
  assert.deepEqual(fixture("api-security-logins-revoke"), { revoked: true });
});

test("the logins.dirty push carries no payload", () => {
  assert.deepEqual(fixture("logins.dirty"), {});
});
