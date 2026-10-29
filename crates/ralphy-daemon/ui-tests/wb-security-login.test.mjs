// Unit tests for `toggleRequireLogin` in assets/ui/wb-security-dialog.ts: the
// switch that puts the login gate on or off. They are here, not in
// wb-security-dialog.test.mjs, so a change of the dialog's markup and a change
// of this switch never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// The dialog on the page of the harness, with a `fetch` that records each
// request and a login screen that records its calls. `enrolled` says whether a
// TOTP seed is armed.
async function toggle({ enrolled, requireLogin, ok = true, code = "" }) {
  const { scope, shell } = loadComponent("wbSecurityDialog", { magics: { $nextTick: (fn) => fn() } });
  const connection = shell.$store.connection;
  connection.securityChanged({ totpEnrolled: enrolled, requireLogin });
  const calls = [];
  connection.logOff = async () => calls.push("log-off");
  connection.probeSession = async () => calls.push("probe");
  scope.askFreshCode = async () => code;
  const requests = [];
  const real = globalThis.fetch;
  globalThis.fetch = async (url, init) => (requests.push([url, init.body]), { ok, status: ok ? 200 : 401, headers: { get: () => null } });
  const box = { checked: requireLogin };
  scope.securityOpen = true;
  try {
    await scope.toggleRequireLogin({ target: box });
  } finally {
    globalThis.fetch = real;
  }
  return { scope, connection, box, calls, requests };
}

test("turning login on with no TOTP seed is refused locally and unticks the box", async () => {
  const t = await toggle({ enrolled: false, requireLogin: false });
  assert.deepEqual(t.requests, [], "nothing is posted");
  assert.equal(t.connection.security.requireLogin, false);
  assert.equal(t.box.checked, false);
});

test("turning login on sets the policy, closes the dialog and logs off", async () => {
  const t = await toggle({ enrolled: true, requireLogin: false });
  assert.deepEqual(t.requests, [["/api/security/require-login", "enable=true"]]);
  assert.equal(t.connection.security.requireLogin, true);
  assert.equal(t.connection.security.policy, "session");
  assert.equal(t.box.checked, true);
  assert.equal(t.scope.securityOpen, false);
  assert.deepEqual(t.calls, ["log-off"]);
});

test("a cancelled code re-ticks the box and posts nothing", async () => {
  const t = await toggle({ enrolled: true, requireLogin: true, code: null });
  assert.deepEqual(t.requests, []);
  assert.equal(t.connection.security.requireLogin, true);
  assert.equal(t.box.checked, true);
});

test("turning login off posts the code and asks the daemon who is signed in", async () => {
  const t = await toggle({ enrolled: true, requireLogin: true, code: "123456" });
  assert.deepEqual(t.requests, [["/api/security/require-login", "enable=false&code=123456"]]);
  assert.equal(t.connection.security.requireLogin, false);
  assert.equal(t.box.checked, false);
  assert.deepEqual(t.calls, ["probe"]);
});

test("a refused request leaves the setting as it was and the box in step", async () => {
  const t = await toggle({ enrolled: true, requireLogin: true, code: "123456", ok: false });
  assert.equal(t.connection.security.requireLogin, true);
  assert.equal(t.box.checked, true);
  assert.deepEqual(t.calls, [], "no log off, no session probe");
  assert.equal(t.scope.securityForm.stepUpError, "Code rejected. Enter the current code from your authenticator app.");
});
