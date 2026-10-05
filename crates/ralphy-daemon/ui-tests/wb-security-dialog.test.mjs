// Unit tests for assets/ui/wb-security-dialog.js, the Security dialog's Alpine
// component. It is built with `loadComponent`, so every test here also fails
// when the component reads a shell() name it does not list in `uses`, assigns a
// shell() field, or writes inside the security fact (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// `state` is the scope the dialog's code sees; `shell` is the shell() object
// around it, where a test sets the security fact and reads what a shell()
// method did.
function dialog(opts = {}) {
  const { scope: state, data, shell, window } = loadComponent(
    "wbSecurityDialog",
    Object.assign({ magics: { $nextTick: (fn) => fn() } }, opts),
  );
  return { state, data, shell, window };
}

// The dialog calls the bare `fetch`. `routes` maps a URL to its reply; `asked`
// collects each request as `[url, body]`.
async function withFetch(routes, fn) {
  const real = globalThis.fetch;
  const asked = [];
  globalThis.fetch = async (url, init = {}) => {
    asked.push([url, init.body ?? null]);
    const reply = routes[url];
    if (!reply) throw new Error(`no route for ${url}`);
    return { ok: reply.ok ?? true, status: reply.status ?? 200, headers: { get: () => null }, json: async () => reply.body };
  };
  try {
    await fn(asked);
  } finally {
    globalThis.fetch = real;
  }
}

// Evaluate a markup expression the way Alpine does: names resolve on the
// scope. `with` needs sloppy mode, which `new Function` gives.
const evalIn = (scope, expr) => new Function("scope", `with (scope) { ${expr}; }`)(scope);

// The element that holds the dialog's component, up to its `>`.
const wrapperTag = () => {
  const html = withoutComments(HTML);
  const at = html.indexOf('x-data="wbSecurityDialog"');
  return html.slice(html.lastIndexOf("<div", at), html.indexOf(">", at) + 1);
};

// Step-up (ADR-0032 amendment E, audit F2): lowering the auth posture costs a
// fresh code once a TOTP seed is armed, the current password once one is
// enrolled. These are the pure folds behind the Security modal's requests —
// the harness serves no network, so the bodies are what can be pinned.
test("step-up asks for a code only once a TOTP seed is armed", async () => {
  const { state, shell } = dialog();
  assert.equal(state.stepUpNeeded(), false, "nothing armed: nothing to ask");
  assert.equal(await state.askFreshCode("x"), "", "resolves at once, no prompt");
  assert.equal(state.stepUp.open, false);

  shell.security.totpEnrolled = true;
  const asked = state.askFreshCode("rotate the access token");
  assert.equal(state.stepUp.open, true, "the prompt opens");
  assert.equal(state.stepUp.label, "rotate the access token");
  state.stepUp.code = "12345";
  state.submitStepUp();
  assert.equal(state.stepUp.open, true, "five digits do not submit");
  state.stepUp.code = " 123456 ";
  state.submitStepUp();
  assert.equal(await asked, "123456", "six digits, trimmed, hand the code back");
  assert.equal(state.stepUp.open, false);

  const cancelled = state.askFreshCode("turn login off");
  state.cancelStepUp();
  assert.equal(await cancelled, null, "cancel resolves null so the action stops");
});

test("step-up bodies carry the code only when there is one", () => {
  const { state } = dialog();
  assert.equal(state.stepUpBody({}, ""), "", "no seed armed: an empty body");
  assert.equal(state.stepUpBody({}, "123456"), "code=123456");
  assert.equal(state.stepUpBody({ enable: "false" }, "123456"), "enable=false&code=123456");
  assert.equal(state.stepUpBody({ enable: "true" }, ""), "enable=true", "enabling never sends a code");
});

test("password bodies always carry the field and add `current` once enrolled", () => {
  const { state, shell } = dialog();
  assert.equal(state.passwordBody("new"), "password=new", "first-time set: no current");
  assert.equal(state.passwordBody(""), "password=", "the clear is an EMPTY field, never an absent one");
  shell.security.passwordSet = true;
  state.securityForm.passwordCurrent = "old";
  assert.equal(state.passwordBody("new"), "password=new&current=old");
  assert.equal(state.passwordBody(""), "password=&current=old");
});

test("step-up refusals name the throttle's wait and a rejected code", () => {
  const { state } = dialog();
  state.noteStepUpRefusal({ status: 429, headers: { get: () => "17" } });
  assert.match(state.securityForm.stepUpError, /wait 17 s/);
  state.noteStepUpRefusal({ status: 401 });
  assert.match(state.securityForm.stepUpError, /Code rejected/);
  state.notePasswordRefusal({ status: 401 });
  assert.match(state.securityForm.stepUpError, /Current password rejected/);
});

// ADR-0073 D4, amendment of 2026-10-05: the security fact is a shell() value,
// and the dialog may not write inside it.
test("the security fact is read-only from the dialog's scope", () => {
  const { state, shell } = dialog();
  assert.equal(state.security.passwordSet, false, "reads are unchanged");
  assert.throws(
    () => {
      state.security.passwordSet = true;
    },
    (e) => e instanceof TypeError && /wbSecurityDialog/.test(e.message) && /security\.passwordSet/.test(e.message),
  );
  assert.throws(() => {
    delete state.security.policy;
  }, TypeError);
  assert.equal(shell.security.passwordSet, false, "the write did not reach shell()");
  assert.equal(shell.security.policy, "session", "the delete did not reach shell()");
});

test("the dialog changes the security fact only through securityChanged", async () => {
  const { state, shell } = dialog();
  const patches = [];
  const real = shell.securityChanged;
  shell.securityChanged = function (patch, password) {
    patches.push([patch, password]);
    return real.call(this, patch, password);
  };
  await withFetch(
    {
      "/api/security/state": {
        body: { token_set: false, password_set: false, totp_enrolled: false, require_login: false, remote_images: true },
      },
      "/api/security/password": { body: { password_set: true } },
      "/api/security/totp/confirm": { body: { confirmed: true } },
    },
    async () => {
      await state.openSecurity();
      assert.equal(state.securityOpen, true);
      assert.equal(shell.security.tokenSet, false);
      assert.equal(shell.security.remoteImages, true);

      state.securityForm.passwordDraft = "pw";
      state.securityForm.passwordConfirm = "pw";
      await state.savePassword();
      assert.equal(shell.security.passwordSet, true);
      assert.equal(shell._passwordValue, "pw", "the demo login checks the new password");
      assert.equal(state.securityForm.passwordDraft, "", "the form is cleared");

      state.securityForm.confirmCode = "123456";
      await state.confirmTotp();
      assert.equal(shell.security.totpEnrolled, true);
    },
  );
  assert.deepEqual(patches, [
    [{ tokenSet: false, passwordSet: false, totpEnrolled: false, requireLogin: false, remoteImages: true }, undefined],
    [{ passwordSet: true }, undefined],
    [{}, "pw"],
    [{ totpEnrolled: true }, undefined],
  ]);
});

test("the dialog lists at most 12 shell() names, and shell() has each one", () => {
  const { data, shell } = dialog();
  assert.ok(data.uses.length <= 12, `${data.uses.length} names: ${data.uses.join(", ")}`);
  for (const name of data.uses) assert.ok(name in shell, name);
  // The dialog's members left shell(): one owner for each name.
  for (const name of Object.keys(data)) {
    if (name !== "uses") assert.ok(!(name in shell), `shell() still has ${name}`);
  }
  // The form state left the fact: `security` holds only what the boot, the
  // login gate and log off read.
  assert.deepEqual(Object.keys(shell.security).sort(), [
    "passwordSet",
    "policy",
    "remoteImages",
    "requireLogin",
    "tokenSet",
    "totpEnrolled",
  ]);
});

test("every name the dialog's markup reads is its own or in its uses list", () => {
  const { state } = dialog();
  const markup = componentMarkup(HTML, "wbSecurityDialog");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  let read = 0;
  for (const [attr, value] of attrs.map((m) => [m[1], m[2]])) {
    if (attr === "x-data") continue;
    for (const name of bindingNames(value)) {
      if (name.startsWith("$")) continue;
      assert.doesNotThrow(() => state[name], `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read > 40, `the markup was read: ${read} names`);
});

test("the account menu closes itself and asks the dialog to open with workbench:security-open", async () => {
  const html = withoutComments(HTML);
  const at = html.indexOf("Security settings</span>");
  const opener = html.slice(html.lastIndexOf("<button", at), html.indexOf(">", html.lastIndexOf("<button", at)) + 1);
  const click = opener.match(/@click="([^"]*)"/)[1];
  assert.match(wrapperTag(), /@workbench:security-open\.window="openSecurity\(\)"/);

  // The opener runs in shell() scope: it closes the menu and sends the event.
  const loaded = loadShell();
  const { scope: state } = loadComponent("wbSecurityDialog", { from: loaded });
  const sent = [];
  loaded.state.avatarMenu = true;
  loaded.state.$dispatch = (name) => sent.push(name);
  evalIn(loaded.state, click);
  assert.equal(loaded.state.avatarMenu, false, "the menu closes");
  assert.deepEqual(sent, ["workbench:security-open"]);

  // The dialog's element hears it and opens.
  const handler = wrapperTag().match(/@workbench:security-open\.window="([^"]*)"/)[1];
  await withFetch({ "/api/security/state": { ok: false, status: 401 } }, async () => {
    evalIn(state, handler);
  });
  assert.equal(state.securityOpen, true);

  // Nothing outside the component names its state.
  const start = HTML.indexOf('x-data="wbSecurityDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["securityOpen", "securityForm", "stepUp", "openSecurity", "closeSecurity"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

test("log off closes the dialog with the workbench:log-off event", async () => {
  const sent = [];
  const loaded = loadShell({ document: { dispatchEvent: (e) => sent.push(e) } });
  const { scope: state } = loadComponent("wbSecurityDialog", { from: loaded });
  state.securityOpen = true;
  // `logOff` emits on the bare `WB` global, which only the page defines.
  const real = globalThis.WB;
  globalThis.WB = loaded.window.WB;
  try {
    await loaded.state.logOff();
  } finally {
    if (real === undefined) delete globalThis.WB;
    else globalThis.WB = real;
  }
  const ev = sent.find((e) => e.type === "workbench:log-off");
  assert.ok(ev, `log off sends workbench:log-off; sent ${sent.map((e) => e.type)}`);
  assert.equal(ev.bubbles, true, "sent on the document, it reaches the window only if it bubbles");

  const handler = wrapperTag().match(/@workbench:log-off\.window="([^"]*)"/)[1];
  evalIn(state, handler);
  assert.equal(state.securityOpen, false);
});
