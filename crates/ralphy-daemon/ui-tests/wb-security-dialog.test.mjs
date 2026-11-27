// Unit tests for assets/ui/wb-security-dialog.ts, the Security dialog's Alpine
// component. It is built with `loadComponent`, so every test here also fails
// when the component reads a shell() name it does not list in `uses`, assigns a
// shell() field, or writes inside the security fact (ADR-0073 D4). The
// security fact is the Alpine store `connection`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  assembledPage,
  fixture,
  loadComponent,
  loadShell,
  partMarkup,
  runIn as evalIn,
  withoutComments,
} from "./harness.mjs";
import { WBSecurityDialog } from "../assets/ui/wb-security-dialog.ts";

// The part that holds the dialog's markup.
const PART = "wb-security-dialog.part.html";

// `state` is the scope the dialog's code sees; `shell` is the shell() object
// around it, whose `$store.connection` holds the security fact a test sets
// and reads.
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
  globalThis.fetch = (url, init = {}) => { // own: restores inside the call; a test swaps routes more than once
    asked.push([url, init.body ?? null]);
    const reply = routes[url];
    if (!reply) throw new Error(`no route for ${url}`);
    return Promise.resolve({ ok: reply.ok ?? true, status: reply.status ?? 200, headers: { get: () => null }, json: () => Promise.resolve(reply.body) });
  };
  try {
    await fn(asked);
  } finally {
    globalThis.fetch = real; // own: the restore of the swap above
  }
}

// The element that holds the dialog's component, up to its `>`.
const wrapperTag = () => {
  const html = partMarkup(PART);
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

  shell.$store.connection.securityChanged({ totpEnrolled: true });
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

test("the step-up resolver stays out of the reactive data", async () => {
  const { state, data, shell } = dialog();
  shell.$store.connection.securityChanged({ totpEnrolled: true });
  const before = new Set(Object.values(data));
  const first = state.askFreshCode("one");
  const held = Object.values(data).filter((v) => typeof v === "function" && !before.has(v));
  assert.deepEqual(held, [], "no field of the data holds the resolver");
  const second = state.askFreshCode("two");
  assert.equal(await first, null, "a second ask answers the first with null");
  state.stepUp.code = "123456";
  state.submitStepUp();
  assert.equal(await second, "123456");
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
  shell.$store.connection.securityChanged({ passwordSet: true });
  state.securityForm.passwordCurrent = "old";
  assert.equal(state.passwordBody("new"), "password=new&current=old");
  assert.equal(state.passwordBody(""), "password=&current=old");
});

test("a password can be saved only with a draft, a matching confirmation and the current password when one is set", () => {
  const { state, shell } = dialog();
  const form = state.securityForm;
  assert.equal(state.canSavePassword(), false, "an empty draft");
  form.passwordDraft = "   ";
  form.passwordConfirm = "   ";
  assert.equal(state.canSavePassword(), false, "a blank draft");
  form.passwordDraft = "pw";
  assert.equal(state.canSavePassword(), false, "no matching confirmation");
  form.passwordConfirm = "pw";
  assert.equal(state.canSavePassword(), true);
  shell.$store.connection.securityChanged({ passwordSet: true });
  assert.equal(state.canSavePassword(), false, "a set password asks for the current one");
  form.passwordCurrent = "old";
  assert.equal(state.canSavePassword(), true);
});

test("each badge reads on or off from its fact", () => {
  const { state } = dialog();
  assert.equal(state.securityBadgeClass(true), "on");
  assert.equal(state.securityBadgeClass(false), "off");
  assert.equal(state.securityBadgeText(true, "set", "not set"), "set");
  assert.equal(state.securityBadgeText(false, "set", "not set"), "not set");
});

test("step-up refusals name the throttle's wait and a rejected code", () => {
  const { state } = dialog();
  state.noteStepUpRefusal({ status: 429, headers: { get: () => "17" } });
  assert.match(state.securityForm.stepUpError, /wait 17 s/u);
  state.noteStepUpRefusal({ status: 401 });
  assert.match(state.securityForm.stepUpError, /Code rejected/u);
  state.notePasswordRefusal({ status: 401 });
  assert.match(state.securityForm.stepUpError, /Current password rejected/u);
});

// The security fact is the Alpine store `connection`, because the boot, the
// login gate and log off read it too: the dialog does not reach it as a
// shell() name.
test("the dialog reads the security fact from the store connection", () => {
  const { state, shell } = dialog();
  assert.throws(() => state.security, ReferenceError);
  assert.equal(shell.security, undefined, "shell() holds no copy");
  assert.equal(state.$store.connection.security.passwordSet, false);
  assert.equal(state.$store.connection.security.policy, "session");
  assert.throws(
    () => {
      state.$store.connection.security.passwordSet = true;
    },
    (e) => e instanceof TypeError && /wbSecurityDialog/u.test(e.message) && /security\.passwordSet/u.test(e.message),
  );
  assert.throws(() => {
    delete state.$store.connection.security.policy;
  }, TypeError);
  assert.equal(shell.$store.connection.security.passwordSet, false, "the write did not reach the store");
  assert.equal(shell.$store.connection.security.policy, "session", "the delete did not reach the store");
});

test("the dialog changes the security fact only through securityChanged", async () => {
  const { state, shell } = dialog();
  const patches = [];
  const real = shell.$store.connection.securityChanged;
  shell.$store.connection.securityChanged = function (patch) {
    patches.push(patch);
    return real.call(this, patch);
  };
  await withFetch(
    {
      "/api/security/state": {
        body: { token_set: false, password_set: false, totp_enrolled: false, require_login: false, remote_images: true, web_content: true },
      },
      "/api/security/password": { body: { password_set: true } },
      "/api/security/totp/confirm": { body: { confirmed: true } },
    },
    async () => {
      await state.openSecurity();
      assert.equal(state.securityOpen, true);
      assert.equal(shell.$store.connection.security.tokenSet, false);
      assert.equal(shell.$store.connection.security.remoteImages, true);

      state.securityForm.passwordDraft = "pw";
      state.securityForm.passwordConfirm = "pw";
      await state.savePassword();
      assert.equal(shell.$store.connection.security.passwordSet, true);
      assert.equal(state.securityForm.passwordDraft, "", "the form is cleared");

      state.securityForm.confirmCode = "123456";
      await state.confirmTotp();
      assert.equal(shell.$store.connection.security.totpEnrolled, true);
    },
  );
  assert.deepEqual(patches, [
    {
      tokenSet: false,
      passwordSet: false,
      totpEnrolled: false,
      requireLogin: false,
      remoteImages: true,
      webContent: true,
    },
    { passwordSet: true },
    { totpEnrolled: true },
  ]);
});

test("each switch that loosens a policy posts to its own route and changes its own fact", async () => {
  // [the method, its route, its fact, whether the dialog then asks for a
  //  reload, the note under the other switch]
  const rows = [
    ["toggleRemoteImages", "/api/security/remote-images", "remoteImages", true, "webContentError"],
    ["toggleWebContent", "/api/security/web-content", "webContent", false, "remoteImagesError"],
  ];
  for (const [method, route, fact, reload, other] of rows) {
    const { state, shell } = dialog();
    const box = { target: { checked: true } };
    await withFetch({ [route]: { body: fixture("api-security-web-content") } }, async (asked) => {
      await state[method](box);
      assert.deepEqual(asked, [[route, "enable=true"]], method);
    });
    assert.equal(shell.$store.connection.security[fact], true, method);
    assert.equal(state.securityForm.remoteImagesReload, reload, method);

    // A refusal leaves the fact, and the switch goes back to it.
    const refused = dialog();
    await withFetch({ [route]: { ok: false, status: 401 } }, async () => {
      await refused.state[method](box);
    });
    assert.equal(refused.shell.$store.connection.security[fact], false, method);
    assert.equal(box.target.checked, false, method);
    // The reason is under the switch that was clicked, and only there.
    const form = refused.state.securityForm;
    const rejected = "Code rejected. Enter the current code from your authenticator app.";
    assert.equal(form[`${fact}Error`], rejected, method);
    assert.equal(form[other], "", method);
    // The next change that the daemon takes removes it.
    await withFetch({ [route]: { body: fixture("api-security-web-content") } }, async () => {
      await refused.state[method](box);
    });
    assert.equal(form[`${fact}Error`], "", method);

    // No answer at all is said there too.
    const silent = dialog();
    const real = globalThis.fetch;
    globalThis.fetch = () => Promise.reject(new TypeError("no network")); // own: restored in this loop, after withFetch swapped it before
    try {
      await silent.state[method](box);
    } finally {
      globalThis.fetch = real; // own: the restore of the swap above
    }
    assert.equal(silent.state.securityForm[`${fact}Error`], "The daemon did not answer. Try again.", method);
  }
});

test("the dialog keeps its shell() names within the limit, and the members and the fact keep one owner", () => {
  const { data, shell } = dialog();
  assert.ok(data.uses.length <= 12, `${data.uses.length} names: ${data.uses.join(", ")}`);
  for (const name of data.uses) assert.ok(name in shell, name);
  // The dialog's members left shell(): one owner for each name.
  for (const name of Object.keys(data)) {
    if (name !== "uses") assert.ok(!(name in shell), `shell() still has ${name}`);
  }
  // The form state left the fact: `security` holds only what the boot, the
  // login gate and log off read.
  assert.deepEqual(Object.keys(shell.$store.connection.security).toSorted(), [
    "passwordSet",
    "policy",
    "remoteImages",
    "requireLogin",
    "tokenSet",
    "totpEnrolled",
    "webContent",
  ]);
});

// pin: the opener sends the event and the element's handler opens the dialog —
// the wiring is only in the markup, so the test runs its text.
test("the account menu closes itself and asks the dialog to open with workbench:security-open", async () => {
  const html = withoutComments(assembledPage("index.html"));
  const at = html.indexOf("Security settings</span>");
  const opener = html.slice(html.lastIndexOf("<button", at), html.indexOf(">", html.lastIndexOf("<button", at)) + 1);
  const click = opener.match(/@click="([^"]*)"/u)[1];
  assert.match(wrapperTag(), /@workbench:security-open\.window="openSecurity\(\)"/u);

  // The opener runs in the account menu's scope: it closes the menu and sends
  // the event.
  const loaded = loadShell();
  const { scope: state } = loadComponent("wbSecurityDialog", { from: loaded });
  const { scope: menu } = loadComponent("wbAccount", { from: loaded });
  const sent = [];
  menu.avatarMenu = true;
  loaded.window.dispatchEvent = (e) => sent.push(e.type);
  evalIn(menu, click);
  assert.equal(menu.avatarMenu, false, "the menu closes");
  assert.deepEqual(sent, ["workbench:security-open"]);

  // The dialog's element hears it and opens.
  const handler = wrapperTag().match(/@workbench:security-open\.window="([^"]*)"/u)[1];
  await withFetch({ "/api/security/state": { ok: false, status: 401 } }, () => {
    evalIn(state, handler);
  });
  assert.equal(state.securityOpen, true);
});

// pin: log off closes the dialog —
// the handler is only in the markup, so the test runs its text.
test("log off closes the dialog with the workbench:log-off event", async () => {
  const sent = [];
  const loaded = loadShell({ window: { dispatchEvent: (e) => sent.push(e) } });
  const { scope: state } = loadComponent("wbSecurityDialog", { from: loaded });
  state.securityOpen = true;
  await loaded.state.$store.connection.logOff();
  const ev = sent.find((e) => e.type === "workbench:log-off");
  assert.ok(ev, `log off sends workbench:log-off on the window; sent ${sent.map((e) => e.type)}`);

  const handler = wrapperTag().match(/@workbench:log-off\.window="([^"]*)"/u)[1];
  evalIn(state, handler);
  assert.equal(state.securityOpen, false);
});

// pin: the modal stack sees the dialog open —
// the path is a string of the markup, and node runs no Alpine over it.
// `app.ts` asks the modal stack with this path; it must be the path the dialog
// gives to `scrim()`, or the shortcuts would never see the dialog open.
test("openFlag is the path the Security dialog gives to scrim()", () => {
  const markup = partMarkup(PART);
  const paths = [...markup.matchAll(/x-bind="\$store\.modals\.scrim\('([^']+)'/gu)].map((m) => m[1]);
  assert.deepEqual(paths, ["securityOpen"]);
  assert.equal(WBSecurityDialog.openFlag, paths[0]);
});
