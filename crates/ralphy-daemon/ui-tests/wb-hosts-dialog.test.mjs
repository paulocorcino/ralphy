// Unit tests for assets/ui/wb-hosts-dialog.ts, the Hosts dialog's Alpine
// component. It is built with `loadComponent`, so every test here also fails
// when the component reads a shell() name it does not list in `uses`, or
// assigns a shell() field (ADR-0073 D4). The fold itself is tested in
// wb-hosts.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, UI, withoutComments } from "./harness.mjs";

const ALIASES = [
  { alias: "svrapp", hostname: "10.0.0.5", user: "deploy", port: 2222 },
  { alias: "web", hostname: "web.lan", user: "me", port: 22 },
];

const VPS_ID = "01TUNNELPEER0000000000000A";
const line = (o) => JSON.stringify(o) + "\n";

// `state` is the scope the dialog's code sees; `shell` is the shell() object
// around it, where a test sets the fleet and reads what a shell() method did.
function dialog(opts = {}) {
  // No DOM here: the scroll to an opened row is checked in the browser.
  const { scope: state, data, shell, window } = loadComponent(
    "wbHostsDialog",
    Object.assign({ magics: { $nextTick: () => {} } }, opts),
  );
  const calls = [];
  const replies = { "host.aliases": { status: "ok", aliases: ALIASES } };
  const scripts = {};
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      return replies[verb] || { status: "ok" };
    },
    spawn: (verb, payload, onStatus) => {
      calls.push({ verb, payload });
      for (const st of scripts[verb] || []) onStatus(st);
      return 1;
    },
  };
  const reloads = [];
  // What a real reload brings back after an add: the new tunnel peer's group.
  shell.loadRepos = async () => {
    reloads.push(1);
    shell.projects = [{ key: VPS_ID + "/me/app", slug: "me/app", path: "/srv/app", daemon: VPS_ID }];
    shell.fleetPeers = [
      { daemon_id: VPS_ID, name: "vps", environment: "Linux", state: "reachable", tunnel: true },
    ];
  };
  const verbs = () => calls.map((c) => c.verb);
  return { state, data, shell, window, calls, replies, scripts, reloads, verbs };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("dialog: the dialog opens with a fleet of one and lists the SSH config hosts", async () => {
  const { state, shell, verbs } = dialog();
  shell.fleetPeers = [];
  state.openAddHost();
  assert.equal(state.addHost.open, true);
  assert.deepEqual(verbs(), ["host.aliases"]);
  await tick();
  assert.deepEqual(state.addHost.aliases.map((a) => a.alias), ["svrapp", "web"]);
});

test("dialog: an unknown host shows its key first, and Cancel writes nothing", async () => {
  const { state, replies, verbs } = dialog();
  replies["host.key"] = {
    status: "ok",
    key: { state: "unknown", keys: [{ type: "ssh-ed25519", fingerprint: "SHA256:abc" }] },
  };
  state.openAddHost();
  state.addHostType("address", "10.0.0.5");
  await state.addHostNext();
  assert.equal(state.addHost.step, "identity");
  assert.deepEqual(verbs(), ["host.aliases", "host.key"], "nothing signs in before the key is trusted");
  state.addHostCancelTrust();
  assert.equal(state.addHost.step, "connection");
  assert.ok(!verbs().includes("host.trust"));
});

test("dialog: Trust sends the fingerprint shown, then runs the checks", async () => {
  const { state, replies, calls, verbs } = dialog();
  replies["host.key"] = {
    status: "ok",
    key: { state: "unknown", keys: [{ type: "ssh-ed25519", fingerprint: "SHA256:abc" }] },
  };
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  await state.addHostTrust();
  assert.deepEqual(calls.find((c) => c.verb === "host.trust").payload, {
    destination: "svrapp",
    fingerprint: "SHA256:abc",
  });
  assert.equal(verbs().at(-1), "host.check");
  assert.equal(state.addHost.step, "checks");
});

test("dialog: the checks render from output chunks, and Check again runs them again", async () => {
  const { state, replies, scripts, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  const ralphy = line({ event: "check", id: "ralphy", label: "Ralphy", status: "copy", text: "old", command: "ralphy update" });
  scripts["host.check"] = [
    { status: "spawned", pid: 1 },
    { status: "output", chunk: line({ event: "connected", os: "Linux" }) + ralphy.slice(0, 12) },
    { status: "output", chunk: ralphy.slice(12) },
    { status: "exited", code: 0 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  assert.equal(state.addHost.os, "Linux");
  assert.equal(state.addHost.checks.length, 1);
  assert.equal(state.addHost.checks[0].command, "ralphy update");
  assert.equal(state.hostReady(), false);
  scripts["host.check"] = [{ status: "spawned", pid: 2 }];
  state.addHostCheckAgain();
  assert.equal(calls.filter((c) => c.verb === "host.check").length, 2);
  assert.deepEqual(state.addHost.checks, [], "the old list is gone before the new run reports");
});

test("dialog: Install runs host.install with the dialog's payload, then checks again", async () => {
  const { state, replies, scripts, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    {
      status: "output",
      chunk:
        line({ event: "check", id: "ralphy", status: "copy", text: "missing", command: "ralphy host install svrapp" }) +
        line({ event: "install", version: "v0.1.0-rc.30", target: "linux-x64", source: "this-computer", folder: "~/.ralphy/bin" }),
    },
    { status: "exited", code: 0 },
  ];
  scripts["host.install"] = [
    { status: "output", chunk: line({ event: "note", text: "Installed Ralphy in ~/.ralphy/bin on svrapp." }) },
    { status: "exited", code: 0 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  assert.equal(state.hostNeedsInstall(), true);
  scripts["host.check"] = [
    { status: "output", chunk: line({ event: "check", id: "ralphy", status: "pass" }) },
    { status: "exited", code: 0 },
  ];
  state.addHostInstall();
  const install = calls.find((c) => c.verb === "host.install");
  assert.deepEqual(install.payload, { destination: "svrapp" });
  assert.equal(calls.filter((c) => c.verb === "host.check").length, 2, "the checks run again after the install");
  assert.equal(state.hostNeedsInstall(), false);
  assert.equal(state.addHost.open, true, "the dialog stays open for Connect");
});

test("dialog: with the check box on, a missing Ralphy is installed once", async () => {
  const { state, replies, scripts, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    {
      status: "output",
      chunk:
        line({ event: "check", id: "ralphy", status: "copy", command: "ralphy host install svrapp" }) +
        line({ event: "install", version: "v1", target: "linux-x64", source: "release", folder: "~/.ralphy/bin" }),
    },
    { status: "exited", code: 0 },
  ];
  // The install fails: the checks are not run again, and nothing loops.
  scripts["host.install"] = [{ status: "exited", code: 1 }];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  state.addHostType("autoInstall", true);
  await state.addHostNext();
  const hostVerbs = () => calls.map((c) => c.verb).filter((v) => v !== "host.aliases");
  assert.deepEqual(hostVerbs(), ["host.key", "host.check", "host.install"]);
  state.addHostCheckAgain();
  assert.equal(calls.filter((c) => c.verb === "host.install").length, 1, "a failed install is not repeated");
});

test("dialog: a failed install keeps the offer and runs no checks", async () => {
  const { state, replies, scripts, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    {
      status: "output",
      chunk:
        line({ event: "check", id: "ralphy", status: "copy", command: "ralphy host install svrapp" }) +
        line({ event: "install", version: "v1", target: "linux-x64", source: "release", folder: "~/.ralphy/bin" }),
    },
    { status: "exited", code: 0 },
  ];
  scripts["host.install"] = [
    { status: "output", chunk: line({ event: "failed", kind: "other", message: "the copy on svrapp is not the binary that was sent" }) },
    { status: "exited", code: 1 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  state.addHostInstall();
  assert.equal(calls.filter((c) => c.verb === "host.check").length, 1);
  assert.match(state.addHost.failure.message, /^The copy on svrapp/);
});

test("dialog: Connect adds the host and reloads the tree with no page reload", async () => {
  const { state, shell, replies, scripts, reloads, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    { status: "output", chunk: line({ event: "check", id: "ralphy", status: "pass" }) },
    { status: "exited", code: 0 },
  ];
  scripts["host.add"] = [
    { status: "output", chunk: line({ event: "added", name: "vps", daemon_id: VPS_ID, port: 7401 }) },
    { status: "exited", code: 0 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  state.addHostType("signIn", "key");
  state.addHostType("keyFile", "C:/keys/id");
  await state.addHostNext();
  state.addHostConnect();
  assert.deepEqual(calls.find((c) => c.verb === "host.add").payload, {
    destination: "svrapp",
    identity: "C:/keys/id",
  });
  assert.equal(reloads.length, 1);
  assert.equal(state.addHost.step, "done", "a last step says the host was added");
  assert.equal(state.hostAddedText(), "Added vps. Its projects are now in the list of projects.");
  state.closeAddHost();
  assert.equal(state.addHost.open, false);
  await tick();
  const group = shell.fleetGroups().find((g) => g.daemon === VPS_ID);
  assert.equal(shell.groupHost(group), "vps");
  assert.equal(shell.groupLabel(group), "Linux");
});

test("dialog: a failed Connect keeps the dialog open and does not reload", async () => {
  const { state, replies, scripts, reloads } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    { status: "output", chunk: line({ event: "check", id: "ralphy", status: "pass" }) },
    { status: "exited", code: 0 },
  ];
  scripts["host.add"] = [
    { status: "output", chunk: line({ event: "failed", kind: "other", message: "restart failed" }) },
    { status: "exited", code: 1 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  state.addHostConnect();
  assert.equal(reloads.length, 0);
  assert.equal(state.addHost.open, true);
  assert.equal(state.addHost.failure.message, "Restart failed");
});

test("dialog: a Connect that saved the host but ended non-zero still reloads the list", async () => {
  const { state, replies, scripts, reloads } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [
    { status: "output", chunk: line({ event: "check", id: "ralphy", status: "pass" }) },
    { status: "exited", code: 0 },
  ];
  scripts["host.add"] = [
    { status: "output", chunk: line({ event: "added", name: "vps", daemon_id: VPS_ID, port: 7401 }) },
    { status: "output", chunk: line({ event: "failed", kind: "other", message: "the tunnel does not reach its daemon" }) },
    { status: "exited", code: 1 },
  ];
  state.openAddHost();
  await tick();
  state.addHostPick("svrapp");
  await state.addHostNext();
  state.addHostConnect();
  assert.equal(reloads.length, 1);
  assert.equal(state.addHost.open, true);
});

test("dialog: Remove host runs host.remove with the daemon id and the token choice", () => {
  const { state, scripts, calls, reloads } = dialog();
  const g = { daemon_id: VPS_ID, name: "vps", tunnel: true };
  scripts["host.remove"] = [
    { status: "output", chunk: line({ event: "note", text: "Forgot vps on this computer." }) },
    { status: "exited", code: 0 },
  ];
  state.openRemoveHost(g);
  assert.equal(state.removeHost.rotate, false, "the token option starts unchecked");
  state.removeHost.rotate = true;
  state.confirmRemoveHost();
  assert.deepEqual(calls.at(-1), { verb: "host.remove", payload: { host: VPS_ID, rotate_token: true } });
  assert.equal(reloads.length, 1);
  assert.equal(state.removeHost.open, false);

  state.openRemoveHost(g);
  state.confirmRemoveHost();
  assert.deepEqual(calls.at(-1).payload, { host: VPS_ID, rotate_token: false });
});

test("dialog: Back from the checks keeps the fields; it waits while a command runs", async () => {
  const { state, replies, scripts } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  scripts["host.check"] = [{ status: "exited", code: 0 }];
  state.openAddHost();
  await tick();
  state.addHostType("address", "10.1.1.4");
  await state.addHostNext();
  assert.equal(state.addHost.step, "checks");
  state.addHostStep({ type: "busy", value: true });
  state.addHostBack();
  assert.equal(state.addHost.step, "checks");
  state.addHostStep({ type: "busy", value: false });
  state.addHostBack();
  assert.equal(state.addHost.step, "connection");
  assert.equal(state.addHost.address, "10.1.1.4");
  const html = readFileSync(join(UI, "index.html"), "utf8");
  assert.match(html, /@click="addHostBack\(\)"[^>]*>Back</);
  assert.match(html, /@click="addHostNext\(\)"[^>]*>Connect</);
  assert.match(html, /'Save' : 'Add host'/);
  assert.match(html, /@click="closeAddHost\(\)">Done</);
});

test("dialog: a removed host leaves the list before the fleet read answers", () => {
  const { state, shell, scripts } = dialog();
  const other = { daemon_id: "01OTHERPEER000000000000000", name: "mac", tunnel: true };
  shell.fleetPeers = [{ daemon_id: VPS_ID, name: "vps", tunnel: true }, other];
  shell._fleetRows = [{ key: VPS_ID + "/me/app", slug: "me/app", daemon: VPS_ID }];
  shell.projects = [{ key: "me/local", slug: "me/local" }, ...shell._fleetRows];
  // The fleet read has not answered yet: the reload changes nothing.
  shell.loadRepos = async () => {};
  scripts["host.remove"] = [{ status: "exited", code: 0 }];
  state.openRemoveHost({ daemon_id: VPS_ID, name: "vps", tunnel: true });
  state.confirmRemoveHost();
  assert.deepEqual(state.sshHosts().map((h) => h.name), ["mac"]);
  assert.deepEqual(shell.projects.map((p) => p.slug), ["me/local"]);
  assert.deepEqual(shell._fleetRows, []);
});

test("dialog: a failed Remove host stays open with the line to remove by hand", () => {
  const { state, scripts, reloads } = dialog();
  scripts["host.remove"] = [
    { status: "output", chunk: line({ event: "note", text: "Forgot vps on this computer." }) },
    { status: "output", chunk: line({ event: "failed", kind: "unreachable", message: "vps did not answer. The key line remains" }) },
    { status: "exited", code: 1 },
  ];
  state.openRemoveHost({ daemon_id: VPS_ID, name: "vps", tunnel: true });
  state.confirmRemoveHost();
  assert.equal(reloads.length, 0);
  assert.equal(state.removeHost.open, true);
  assert.equal(state.removeHost.busy, false);
  assert.deepEqual(state.removeHost.lines, ["Forgot vps on this computer."]);
  assert.match(state.removeHost.failure.message, /key line remains/);
});

test("dialog: Hosts opens on the list when a host is paired over SSH, else on the form", () => {
  const { state, shell } = dialog();
  shell.fleetPeers = [{ daemon_id: "wsl", name: "ubuntu", environment: "WSL: Ubuntu", state: "reachable", tunnel: false }];
  state.openAddHost();
  assert.equal(state.hostTab(), "add", "a WSL daemon is not a host of this list");
  assert.deepEqual(state.sshHosts(), []);
  shell.fleetPeers.push({ daemon_id: VPS_ID, name: "vps", environment: "Linux", state: "reachable", tunnel: true, destination: "root@vps" });
  state.openAddHost();
  assert.equal(state.addHost.tab, "hosts");
  assert.deepEqual(state.sshHosts().map((h) => h.name), ["vps"]);
  state.addHostEdit(state.sshHosts()[0]);
  assert.equal(state.hostTab(), "add");
  assert.equal(state.addHostPayload().destination, "root@vps");
  state.addHostCancel();
  assert.equal(state.addHost.open, true, "Cancel leaves an edit for the list");
  assert.equal(state.hostTab(), "hosts");
  state.addHostTab("add");
  state.addHostCancel();
  assert.equal(state.addHost.open, false, "Cancel closes a new host's form");
  state.openAddHost();
  // The last host removed: no list is left to show.
  state.addHostTab("hosts");
  shell.fleetPeers = shell.fleetPeers.filter((p) => !p.tunnel);
  assert.equal(state.hostTab(), "add");
});

test("dialog: a typed password goes with the check, only when signing in without a key file", async () => {
  const { state, replies, calls } = dialog();
  replies["host.key"] = { status: "ok", key: { state: "known" } };
  state.openAddHost();
  await tick();
  state.addHostType("address", "10.1.1.4");
  state.addHostType("user", "root");
  state.addHostType("password", "s3cret");
  assert.equal(state.hostPasswordAllowed(), true);
  await state.addHostNext();
  assert.deepEqual(calls.find((c) => c.verb === "host.key").payload, { destination: "root@10.1.1.4" });
  assert.deepEqual(calls.find((c) => c.verb === "host.check").payload, {
    destination: "root@10.1.1.4",
    password: "s3cret",
  });
  state.addHostType("signIn", "key");
  assert.equal(state.addHostPayload().password, undefined);
  state.closeAddHost();
  assert.equal(state.addHost.password, "");
});

test("dialog: over plain http from the network the password is not sent", () => {
  const { state } = dialog({
    window: { location: { protocol: "http:", host: "192.168.1.5:7257", hostname: "192.168.1.5", pathname: "/", search: "" } },
  });
  state.openAddHost();
  state.addHostType("address", "10.1.1.4");
  state.addHostType("password", "s3cret");
  assert.equal(state.hostPasswordAllowed(), false);
  assert.equal(state.addHostPayload().password, undefined);
});

test("dialog: the password field is a text field when the browser can hide its characters", () => {
  const { state, window } = dialog();
  window.CSS = { supports: (prop, value) => prop === "-webkit-text-security" && value === "disc" };
  assert.equal(state.hostSecretType(), "text");
  window.CSS = { supports: () => false };
  assert.equal(state.hostSecretType(), "password");
  delete window.CSS;
  assert.equal(state.hostSecretType(), "password");
  state.hostSecretShown = true;
  assert.equal(state.hostSecretType(), "text", "the eye button shows the characters");
  state.openAddHost();
  assert.equal(state.hostSecretType(), "password", "the dialog opens with the password hidden");
});

test("dialog: a host row's tooltip holds the state, not the name the row prints", () => {
  const { state } = dialog();
  const up = { name: "vps", state: "reachable", diagnosis: "Peer Linux answered the handshake." };
  assert.equal(state.hostRowTitle(up), "Connected");
  const closed = { name: "vps", state: "tunnel-closed", diagnosis: "The tunnel to vps is closed. The daemon is opening it again." };
  assert.equal(state.hostRowTitle(closed), closed.diagnosis);
  assert.equal(state.hostRowTitle({ name: "vps", state: "tunnel-silent" }), "not answering");
});

// ---- the component's contract with shell() and the page ----

const HTML = readFileSync(join(UI, "index.html"), "utf8");

test("the dialog lists at most 12 shell() names, and shell() has each one", () => {
  const { data, shell } = dialog();
  assert.ok(data.uses.length <= 12, `${data.uses.length} names: ${data.uses.join(", ")}`);
  for (const name of data.uses) assert.ok(name in shell, name);
  // The dialog's members left shell(): one owner for each name.
  for (const name of Object.keys(data)) {
    if (name !== "uses") assert.ok(!(name in shell), `shell() still has ${name}`);
  }
});

test("every name the dialog's markup reads is its own or in its uses list", () => {
  const { state } = dialog();
  const markup = componentMarkup(HTML, "wbHostsDialog");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // `x-data` names the component; `x-for` adds its loop variables.
  const loopVars = new Set();
  for (const [, attr, value] of attrs) {
    if (attr !== "x-for") continue;
    const [left] = value.split(/\s+in\s+/);
    for (const v of bindingNames(left)) loopVars.add(v);
  }
  let read = 0;
  for (const [, attr, value] of attrs) {
    if (attr === "x-data") continue;
    const expr = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
    for (const name of bindingNames(expr)) {
      if (loopVars.has(name) || name.startsWith("$")) continue;
      assert.doesNotThrow(() => state[name], `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read > 100, `the markup was read: ${read} names`);
});

// Every test above runs on this scope, so these two refusals cover them all.
test("the dialog's scope refuses a shell() name outside uses, and any shell() write", () => {
  const { state, shell } = dialog();
  assert.ok("projects" in shell);
  assert.throws(() => state.projects, /reads projects/);
  assert.throws(() => {
    state.fleetPeers = [];
  }, /assigns fleetPeers/);
  assert.deepEqual(shell.fleetPeers, [], "the write did not reach shell()");
});

test("the sidebar button asks the dialog to open with workbench:hosts-open", () => {
  const html = withoutComments(HTML);
  const opener = html.match(/<button class="side-refresh side-add-host"[^>]*>/)[0];
  assert.match(opener, /@click="\$dispatch\('workbench:hosts-open'\)"/);
  assert.match(html, /<div class="hosts-dialog" x-data="wbHostsDialog" @workbench:hosts-open\.window="openAddHost\(\)">/);
  // Nothing outside the component names its state.
  const start = HTML.indexOf('x-data="wbHostsDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["addHost", "removeHost", "hostSecretShown", "openAddHost"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});
