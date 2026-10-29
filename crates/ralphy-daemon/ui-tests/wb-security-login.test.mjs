// Unit tests for `toggleRequireLogin` in assets/ui/wb-security-dialog.ts: the
// switch that puts the login gate on or off. They are here, not in
// wb-security-dialog.test.mjs, so a change of the dialog's markup and a change
// of this switch never edit one file.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadComponent } from "./harness.mjs";

// The dialog on the page of the harness, with a `fetch` that records each
// request and a login screen that records its calls. `enrolled` says whether a
// TOTP seed is armed, `unreachable` makes `fetch` throw, and `shown` is a
// message of the switch already shown before the toggle.
async function toggle({ enrolled, requireLogin, ok = true, code = "", unreachable = false, shown = "" }) {
  const { scope, shell } = loadComponent("wbSecurityDialog", { magics: { $nextTick: (fn) => fn() } });
  const connection = shell.$store.connection;
  connection.securityChanged({ totpEnrolled: enrolled, requireLogin });
  const calls = [];
  connection.logOff = async () => calls.push("log-off");
  connection.probeSession = async () => calls.push("probe");
  scope.askFreshCode = async () => code;
  const requests = [];
  const real = globalThis.fetch;
  globalThis.fetch = async (url, init) => {
    requests.push([url, init.body]);
    if (unreachable) throw new TypeError("Failed to fetch");
    return { ok, status: ok ? 200 : 401, headers: { get: () => null } };
  };
  const box = { checked: requireLogin };
  scope.securityOpen = true;
  scope.securityForm.requireLoginError = shown;
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
  assert.equal(t.scope.securityForm.requireLoginError, "Enroll two-factor above first.");
  t.scope.closeSecurity();
  assert.equal(t.scope.securityForm.requireLoginError, "", "closing the dialog clears the message");
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
  const t = await toggle({ enrolled: true, requireLogin: true, code: "123456", shown: "An old message." });
  assert.deepEqual(t.requests, [["/api/security/require-login", "enable=false&code=123456"]]);
  assert.equal(t.connection.security.requireLogin, false);
  assert.equal(t.box.checked, false);
  assert.deepEqual(t.calls, ["probe"]);
  assert.equal(t.scope.securityForm.requireLoginError, "", "a new toggle clears the old message");
});

// A refusal to turn login off is a step-up refusal; every other failure is
// said under the switch.
test("a refused or failed request leaves the setting as it was and says why", async () => {
  const rejected = "Code rejected. Enter the current code from your authenticator app.";
  const unreachable = "Cannot reach the daemon. Try again.";
  // (case, toggle options, stepUpError, requireLoginError)
  const rows = [
    ["turning off is refused", { requireLogin: true, code: "123456", ok: false }, rejected, ""],
    ["turning on is refused", { requireLogin: false, ok: false }, "", "The daemon refused (401)."],
    ["turning off cannot reach the daemon", { requireLogin: true, code: "123456", unreachable: true }, "", unreachable],
    ["turning on cannot reach the daemon", { requireLogin: false, unreachable: true }, "", unreachable],
  ];
  for (const [name, opts, stepUpError, requireLoginError] of rows) {
    const t = await toggle({ enrolled: true, ...opts });
    assert.equal(t.requests.length, 1, name);
    assert.equal(t.connection.security.requireLogin, opts.requireLogin, name);
    assert.equal(t.box.checked, opts.requireLogin, name);
    assert.deepEqual(t.calls, [], `${name}: no log off, no session probe`);
    assert.equal(t.scope.securityForm.stepUpError, stepUpError, name);
    assert.equal(t.scope.securityForm.requireLoginError, requireLoginError, name);
  }
});
