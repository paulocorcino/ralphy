// The shared replies of the login routes, as the Rust route tests wrote them:
// the revoke reply (fixtures/api-security-logins-revoke.json), the
// `logins.dirty` push (fixtures/logins.dirty.json), the notices reply
// (fixtures/api-audit-logins-notices.json) and the dismiss reply
// (fixtures/api-audit-logins-dismiss.json).
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

test("the notices reply lists each notice with its pair, its action and its device facts", () => {
  const { notices } = fixture("api-audit-logins-notices");
  assert.ok(Array.isArray(notices) && notices.length > 0);
  for (const notice of notices) {
    assert.equal(typeof notice.login, "string");
    assert.ok(notice.device === null || typeof notice.device === "string");
    assert.equal(typeof notice.did, "string");
    assert.equal(typeof notice.at, "string");
    assert.ok(notice.facts === null || typeof notice.facts.os === "string");
  }
});

test("the dismiss reply says the notice is dismissed", () => {
  assert.deepEqual(fixture("api-audit-logins-dismiss"), { dismissed: true });
});
