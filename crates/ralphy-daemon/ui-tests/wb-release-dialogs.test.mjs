// Unit tests for assets/ui/wb-release-dialogs.ts, the Alpine component of the
// About, What's new and update dialogs. It is built with `loadComponent`, so
// every test here also fails when the component reads a shell() name it does
// not list in `uses`, assigns a shell() field, or writes inside the release
// fact (ADR-0073 D4). The release fact itself (`loadRelease`) is tested in
// app.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";
import { WBRelease } from "../assets/ui/wb-release.ts";
import { WBReleaseDialogs } from "../assets/ui/wb-release-dialogs.ts";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// `state` is the scope the dialogs' code sees; `shell` is the shell() object
// around it, where a test sets the sessions and the fleet, and reads what a
// shell() method did.
function dialogs(opts = {}) {
  const { scope: state, data, shell, window } = loadComponent("wbReleaseDialogs", opts);
  return { state, data, shell, window };
}

// Evaluate a markup expression the way Alpine does: names resolve on the
// scope. `with` needs sloppy mode, which `new Function` gives.
const evalIn = (scope, expr) => new Function("scope", `with (scope) { ${expr}; }`)(scope);
const valueIn = (scope, expr) => new Function("scope", `with (scope) { return ${expr}; }`)(scope);

// The element that holds the dialogs' component, up to its `>`.
const wrapperTag = () => {
  const html = withoutComments(HTML);
  const at = html.indexOf('x-data="wbReleaseDialogs"');
  return html.slice(html.lastIndexOf("<div", at), html.indexOf(">", at) + 1);
};
const handlerOf = (event) => wrapperTag().match(new RegExp(`@${event}\\.window="([^"]*)"`))[1];

// The account menu row whose text ends with `label`, up to its `>`.
const menuRow = (label) => {
  const html = withoutComments(HTML);
  const at = html.indexOf(label);
  const start = html.lastIndexOf("<button", at);
  return html.slice(start, html.indexOf(">", start) + 1);
};

test("the What's new panel warns that the update closes the open consoles", () => {
  const { state, shell } = dialogs();
  shell.liveSessions = [];
  assert.equal(state.releaseConsoleWarning, "", "no console, no warning");
  shell.liveSessions = [{ id: "a" }];
  assert.match(state.releaseConsoleWarning, /^1 console is open\. The update closes it/);
  shell.liveSessions = [{ id: "a" }, { id: "b" }, { id: "c" }];
  assert.match(state.releaseConsoleWarning, /^3 consoles are open\. The update closes them/);
});

test("opening What's new counts the consoles again", () => {
  const { state, shell } = dialogs();
  let polled = 0;
  shell.refreshLive = () => polled++;
  state.openWhatsNew();
  assert.equal(polled, 1);
});

// The update the page asks for (ADR-0056 §11). A refusal must leave the
// question open with the reason, and a daemon that comes back on the same build
// after a gap has rolled back — the page must say so, not wait or reload.
test("the question names the WSL peers the update takes after this daemon", async () => {
  const { state, shell, window } = dialogs();
  window.fetch = async () => ({ ok: false });
  shell.refreshLive = async () => {};
  shell.fleetPeers = [
    { daemon_id: "wsl-id", name: "Ubuntu", environment: "wsl", nudgeable: true },
    { daemon_id: "srv-id", name: "server", environment: "linux", nudgeable: false },
  ];
  // One console here, two in WSL, one on the other peer.
  shell.liveSessions = [
    { daemon_id: "here", name: "app #1" },
    { daemon_id: "wsl-id", name: "api #1" },
    { daemon_id: "wsl-id", name: "api #2" },
    { daemon_id: "srv-id", name: "web #1" },
  ];
  await state.beginUpdate();
  assert.equal(state.relUpdate.phase, "confirm");
  assert.deepEqual(state.relUpdate.peers, ["Ubuntu"], "only a peer reached through wsl.exe is updated");
  assert.deepEqual(state.relUpdate.consoles, ["app #1"], "only this daemon's consoles close for sure");
  assert.match(state.updatePeerText, /If it takes a new version, its 2 consoles close as well\.$/);
});

// `ralphy update` in a terminal restarts only this daemon, so a peer's
// consoles are not in the warning.
test("the console warning counts only the consoles this daemon hosts", () => {
  const { state, shell } = dialogs();
  shell.fleetPeers = [{ daemon_id: "wsl-id", nudgeable: true }];
  shell.liveSessions = [{ daemon_id: "here" }, { daemon_id: "wsl-id" }, { daemon_id: "wsl-id" }];
  assert.match(state.releaseConsoleWarning, /^1 console is open\./);
});

test("a refused update keeps the question open and says why", async () => {
  const { state } = dialogs();
  state.relUpdate = { phase: "confirm", code: "123456", needCode: true, consoles: [], error: "" };
  WBRelease.update = async () => ({ ok: false, status: 401, message: "invalid credentials" });
  await state.confirmUpdate();
  assert.equal(state.relUpdate.phase, "confirm");
  assert.match(state.relUpdate.error, /Code rejected/);
  assert.equal(state.relUpdate.code, "", "a rejected code is not offered again");

  WBRelease.update = async () => ({ ok: false, status: 500, message: "Error: checksum mismatch" });
  state.relUpdate.code = "654321";
  await state.confirmUpdate();
  assert.equal(state.relUpdate.error, "Error: checksum mismatch", "the update's own words reach the page");
});

test("the same build after a gap is a rollback, and a new build reloads the page", async () => {
  const { state, window } = dialogs();
  const view = (current) => ({ ...WBRelease.EMPTY, current });
  let reads = [null, view("v0.1.0-rc.30")];
  WBRelease.read = async () => reads.shift();
  let reloaded = false;
  Object.defineProperty(window, "location", { value: { reload: () => (reloaded = true) }, configurable: true });
  await state.awaitNewBuild("v0.1.0-rc.30", 0);
  assert.equal(state.relUpdate.phase, "error");
  assert.match(state.relUpdate.error, /did not start/);
  assert.equal(reloaded, false);

  reads = [view("v0.1.0-rc.30"), null, view("v0.1.0-rc.31")];
  await state.awaitNewBuild("v0.1.0-rc.30", 0);
  assert.equal(reloaded, true, "the new build brings its own workbench");
});

// ADR-0073 D4, amendment of 2026-10-05: the release fact is a shell() value,
// because the sidebar reads it. The dialogs read it, and change it only
// through two shell() methods.
test("the dialogs change the release fact only through markReleaseSeen and releaseWatchChanged", async () => {
  const { state, shell } = dialogs();
  shell.refreshLive = () => {};
  assert.throws(
    () => {
      state.release.disabled = true;
    },
    (e) => e instanceof TypeError && /wbReleaseDialogs/.test(e.message) && /release\.disabled/.test(e.message),
  );
  assert.throws(() => {
    state.releaseSeen = true;
  }, TypeError);
  assert.equal(shell.releaseSeen, false, "the write did not reach shell()");

  state.openWhatsNew();
  assert.equal(state.whatsNewOpen, true);
  assert.equal(shell.releaseSeen, true, "opening the panel dismisses the release it shows");

  const watched = [];
  WBRelease.setWatch = async (enable) => watched.push(enable);
  await state.setReleaseWatch(false);
  assert.deepEqual(watched, [false]);
  assert.equal(shell.release.disabled, true);
  await state.setReleaseWatch(true);
  assert.equal(shell.release.disabled, false);

  // A refused preference leaves the fact as it was: the next read says what took.
  WBRelease.setWatch = async () => {
    throw new Error("offline");
  };
  await state.setReleaseWatch(false);
  assert.equal(shell.release.disabled, false);
});

test("the dialogs list at most 12 shell() names, and shell() has each one", () => {
  const { data, shell } = dialogs();
  assert.ok(data.uses.length <= 12, `${data.uses.length} names: ${data.uses.join(", ")}`);
  for (const name of data.uses) assert.ok(name in shell, name);
  // The dialogs' members left shell(): one owner for each name.
  for (const name of Object.keys(data)) {
    if (name !== "uses") assert.ok(!(name in shell), `shell() still has ${name}`);
  }
});

test("every name the dialogs' markup reads is their own or in their uses list", () => {
  const { state } = dialogs();
  const markup = componentMarkup(HTML, "wbReleaseDialogs");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // `x-for="entry in …"` names the loop's own variable.
  const loop = new Set(attrs.filter((m) => m[1] === "x-for").flatMap((m) => bindingNames(m[2].split(" in ")[0])));
  let names = 0;
  for (const [attr, value] of attrs.map((m) => [m[1], m[2]])) {
    if (attr === "x-data") continue;
    for (const name of bindingNames(attr === "x-for" ? value.split(" in ").slice(1).join(" in ") : value)) {
      if (name.startsWith("$") || loop.has(name)) continue;
      assert.doesNotThrow(() => state[name], `${attr}="${value}" reads ${name}`);
      names++;
    }
  }
  assert.ok(names > 40, `the markup was read: ${names} names`);
});

test("the account menu closes itself and asks the dialogs to open with their events", async () => {
  const loaded = loadShell();
  const { scope: state } = loadComponent("wbReleaseDialogs", { from: loaded });
  loaded.state.refreshLive = () => {};
  for (const [label, event, flag] of [
    ["<span x-text=\"releaseSummary\"></span>", "workbench:whats-new-open", "whatsNewOpen"],
    ["<span>About</span>", "workbench:about-open", "aboutOpen"],
  ]) {
    const click = menuRow(label).match(/@click="([^"]*)"/)[1];
    // The opener runs in shell() scope: it closes the menu and sends the event.
    const sent = [];
    loaded.state.avatarMenu = true;
    loaded.state.$dispatch = (name) => sent.push(name);
    evalIn(loaded.state, click);
    assert.equal(loaded.state.avatarMenu, false, `${event}: the menu closes`);
    assert.deepEqual(sent, [event]);

    // The dialogs' element hears it and opens the one asked for.
    const real = globalThis.fetch;
    globalThis.fetch = async () => ({ ok: false });
    try {
      await valueIn(state, handlerOf(event));
    } finally {
      globalThis.fetch = real;
    }
    assert.equal(state[flag], true, `${event} opens ${flag}`);
  }

  // Nothing outside the component names its state.
  const start = HTML.indexOf('x-data="wbReleaseDialogs"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["whatsNewOpen", "aboutOpen", "relUpdate", "openWhatsNew", "openAbout", "about"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b(?!-)`), name);
  }
});

// `app.ts` asks the modal stack with this path; it must be the path the panel
// gives to `scrim()`, or the shortcuts would never see the panel open.
test("whatsNewFlag is the path the What's new panel gives to scrim()", () => {
  const markup = componentMarkup(HTML, "wbReleaseDialogs");
  const paths = [...markup.matchAll(/x-bind="scrim\('([^']+)'/g)].map((m) => m[1]);
  assert.deepEqual(paths, ["whatsNewOpen", "aboutOpen"]);
  assert.equal(WBReleaseDialogs.whatsNewFlag, paths[0]);
});
