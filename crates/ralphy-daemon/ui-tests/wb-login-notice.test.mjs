// Unit tests for the login notices (assets/ui/wb-login-notice.ts), and the
// shared replies of the login routes, as the Rust route tests wrote them:
// the revoke reply (fixtures/api-security-logins-revoke.json), the
// `logins.dirty` push (fixtures/logins.dirty.json), the notices reply
// (fixtures/api-audit-logins-notices.json) and the dismiss reply
// (fixtures/api-audit-logins-dismiss.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { bindingNames, partMarkup } from "./harness.mjs";
import { wbLoginNotice } from "../assets/ui/wb-login-notice.ts";
import { connectionStore } from "../assets/ui/wb-connection-store.ts";

const HERE = dirname(fileURLToPath(import.meta.url));

// The part that holds the notices.
const PART = "wb-login-notice.part.html";

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

// The notice on every open page (assets/ui/wb-login-notice.ts), built as
// Alpine builds it, with a window, a document and a fetch that answers each
// call with the next of `replies` and keeps what was sent. `init` is not run.
// The test files share one process, so `t.after` puts the globals back.
const GLOBALS = ["window", "document", "fetch"];

function page(t, replies = []) {
  const saved = GLOBALS.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  t.after(() => {
    for (const [name, d] of saved) {
      if (d) Object.defineProperty(globalThis, name, d);
      else delete globalThis[name];
    }
  });
  const sent = [];
  globalThis.window = { addEventListener() {}, dispatchEvent() {} };
  globalThis.document = { visibilityState: "visible", addEventListener() {} };
  globalThis.fetch = async (url, init) => {
    sent.push([init.method, url, init.body]);
    const { status, body } = replies.shift();
    return { ok: status >= 200 && status < 300, status, json: async () => body };
  };
  return { data: wbLoginNotice(), sent };
}

const ok = (body) => ({ status: 200, body });

test("a notice shows the device and what it did", async (t) => {
  const reply = fixture("api-audit-logins-notices");
  const { data, sent } = page(t, [ok(reply)]);
  await data.read();
  assert.deepEqual(sent, [["GET", "/api/audit/logins/notices", undefined]]);
  assert.equal(data.notices.length, 1);
  const [notice] = data.notices;
  const device = data.deviceOf(notice);
  for (const part of ["Android", "Chrome", "phone", "203.0.113.5"]) {
    assert.ok(device.includes(part), `${device} names ${part}`);
  }
  assert.ok(data.didOf(notice).startsWith("Opened a console · "), data.didOf(notice));
  assert.ok(
    data.didOf({ ...notice, did: "file.write" }).startsWith("Command file.write · "),
    data.didOf({ ...notice, did: "file.write" }),
  );
  // A use with no device cookie, from a page that never reported facts.
  assert.equal(data.deviceOf({ ...notice, device: null, facts: null }), "A device that sent no facts");
  // The markup shows both lines, and each name it reads is the component's.
  const markup = partMarkup(PART);
  assert.match(markup, /x-text="deviceOf\(n\)"/);
  assert.match(markup, /x-text="didOf\(n\)"/);
  for (const [, attr, value] of markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)) {
    if (attr === "x-data" || attr === "x-for") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || n === "n") continue;
      assert.ok(n in data, `${attr}="${value}" reads ${n}`);
    }
  }
});

test("revoke ends that login and closes the notice", async (t) => {
  const [notice] = fixture("api-audit-logins-notices").notices;
  const other = { ...notice, login: "BBBBBBBBBBBBBBBBBBBBBB" };
  const { data, sent } = page(t, [
    { status: 500, body: "revoke failed" },
    ok(fixture("api-security-logins-revoke")),
  ]);
  data.notices = [notice, { ...notice, device: null, facts: null }, other];
  // A refusal keeps the notice and says the act failed.
  await data.revoke(notice);
  assert.equal(data.notices.length, 3);
  assert.match(data.failureOf(notice), /^Could not end this login: /);
  await data.revoke(notice);
  assert.deepEqual(sent[1], ["POST", "/api/security/logins/revoke", `login=${notice.login}`]);
  // Every notice of that login goes; another login's stays.
  assert.deepEqual(data.notices, [other]);
  assert.equal(data.failureOf(notice), "");
  assert.match(partMarkup(PART), /@click="revoke\(n\)"[^>]*>End this login</);
});

test("this was me closes the notice", async (t) => {
  const [notice] = fixture("api-audit-logins-notices").notices;
  const bare = { ...notice, device: null, facts: null };
  const { data, sent } = page(t, [
    ok(fixture("api-audit-logins-dismiss")),
    ok(fixture("api-audit-logins-dismiss")),
  ]);
  data.notices = [notice, bare];
  await data.dismiss(notice);
  assert.deepEqual(sent[0], [
    "POST",
    "/api/audit/logins/dismiss",
    `login=${notice.login}&device=${notice.device}`,
  ]);
  // Only that pair goes: the same login on another device stays.
  assert.deepEqual(data.notices, [bare]);
  await data.dismiss(bare);
  assert.deepEqual(sent[1], ["POST", "/api/audit/logins/dismiss", `login=${notice.login}&device=`]);
  assert.deepEqual(data.notices, []);
  assert.match(partMarkup(PART), /@click="dismiss\(n\)"[^>]*>This was me</);
});

test("a logins push reads the notices again", (t) => {
  page(t);
  const sent = [];
  window.dispatchEvent = (e) => sent.push(e.type);
  document.querySelector = () => null;
  const store = connectionStore({ messages: { flash() {} }, detached: { dirty: () => false, close() {} } });
  store.onPresencePush("logins.dirty", fixture("logins.dirty"));
  assert.deepEqual(sent, ["workbench:logins-reread"]);
});
