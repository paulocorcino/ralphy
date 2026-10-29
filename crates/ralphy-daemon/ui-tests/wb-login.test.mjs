// Unit tests for assets/ui/wb-login.ts, the Alpine component of the login
// gate and its TOTP boxes. The component is built with `loadComponent`, so
// every test here also fails when it reads a shell() name it does not list in
// `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { assembledPage, bindingNames, loadComponent, loadShell, partMarkup, withoutComments } from "./harness.mjs";

// The part that holds the login gate's markup.
const PART = "wb-login.part.html";

// One page with the shell and the gate, and a window that delivers the events
// it is sent, as the browser does. `init()` adds the listeners, as Alpine does
// when it builds the component.
function page(opts = {}) {
  const listeners = {};
  const loaded = loadShell({
    ...opts,
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => ((listeners[e.type] || []).forEach((fn) => fn(e)), true),
    },
  });
  const gate = loadComponent("wbLogin", { from: loaded });
  gate.scope.init();
  return { loaded, shell: loaded.state, gate: gate.scope };
}

// ---- the component and its markup ---------------------------------------------

test("wbLogin lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbLogin");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbLogin reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbLogin");
  const markup = partMarkup(PART);
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variable of the digit boxes.
  const loopVars = new Set(["i"]);
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component, `x-ref` a reference.
    if (attr === "x-data" || attr === "x-ref") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read >= 8, `the markup was read: ${read} names`);
});

test("nothing outside the login gate names its state", () => {
  // The gate is the last element of the page that binds state: what comes
  // before it is outside. Only the bindings are read; the page's text says
  // "login" in other places.
  const outside = withoutComments(assembledPage("index.html", { skip: PART }));
  const read = new Set();
  for (const m of outside.matchAll(/\s(?:x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)) {
    for (const n of bindingNames(m[1])) read.add(n);
  }
  for (const name of ["login", "submitLogin", "otpInput", "otpKeydown", "otpPaste", "loginBody", "rehydrateAfterAuth"]) {
    assert.ok(!read.has(name), name);
  }
  assert.ok(read.size > 100, `the bindings outside were read: ${read.size} names`);
});

// ---- the login ------------------------------------------------------------------

// The login body is a pure fold over `login` + `security`, extracted so the
// "keep me signed in" box (ADR-0032 amendment 2026-09-16) can be asserted here:
// the daemon treats an absent `remember` as a standard session, so the box
// must only ever ADD the field, never send `remember=false`.
test("loginBody sends remember=true only when the box is checked", () => {
  const { scope: state, shell } = loadComponent("wbLogin");
  assert.equal(state.login.remember, false, "opt-in: off by default");
  state.login.code = " 123456 ";
  assert.equal(state.loginBody(), "code=123456", "trimmed code, no remember");
  state.login.remember = true;
  assert.equal(state.loginBody(), "code=123456&remember=true");
  shell.$store.connection.securityChanged({ passwordSet: true });
  state.login.password = "pw";
  assert.equal(state.loginBody(), "code=123456&password=pw&remember=true");
});

test("logOff resets the remember box along with the credentials", async () => {
  const { shell, gate } = page();
  shell.$store.connection.securityChanged({ policy: "session" });
  gate.login.remember = true;
  gate.login.passwordRequired = true;
  await shell.$store.connection.logOff();
  assert.equal(shell.$store.connection.authed, false);
  assert.equal(gate.login.remember, false, "the box does not survive a log-off");
  assert.equal(gate.login.passwordRequired, true, "the server-told flag does");
});

test("a session read tells the gate whether the login asks for a password", () => {
  const { loaded, gate } = page();
  loaded.window.dispatchEvent(
    new CustomEvent("workbench:auth-read", { detail: { authed: false, password: true, policy: "session" } }),
  );
  assert.equal(gate.login.passwordRequired, true);
});

// --- ADR-0070 D2: a shown fact is read again on named events only ----------

test("login reads the board, the runs and the tree again", () => {
  const { loaded, shell: state, gate } = page();
  const files = loadComponent("wbFiles", { from: loaded }).scope;
  files.init();
  const calls = [];
  for (const name of ["loadRepos", "loadAgents"]) state.$store.projects[name] = () => calls.push(name);
  loaded.window.addEventListener("workbench:logged-in", () => calls.push("logged-in"));
  state.$store.connection.loadIdentity = () => calls.push("loadIdentity");
  state.$store.runs.hydrateRuns = () => calls.push("hydrateRuns");
  loaded.window.addEventListener("workbench:board-refresh", (e) => calls.push(`board:${e.detail.trigger}`));
  files._treeSub = { replay: () => calls.push("tree") };
  // Closed: the runs lock the writes whether the panel shows them or not.
  state.runsOpen = false;
  // The refusal the gated read left goes before the read again.
  state.$store.projects.reposFailed("the daemon answered 401");
  gate.rehydrateAfterAuth();
  for (const want of ["loadRepos", "logged-in", "board:login", "hydrateRuns", "tree"]) {
    assert.ok(calls.includes(want), `${want} in ${calls.join(", ")}`);
  }
  assert.equal(state.$store.projects.reposError, "");
});
