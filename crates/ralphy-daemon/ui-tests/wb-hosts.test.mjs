// Unit tests for assets/ui/wb-hosts.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-hosts.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBHosts;
}

const ALIASES = [
  { alias: "svrapp", hostname: "10.0.0.5", user: "deploy", port: 2222 },
  { alias: "web", hostname: "web.lan", user: "me", port: 22 },
];

test("picking an alias fills user and port, and the alias is the destination", () => {
  const H = load();
  let s = H.next(H.initial(), { type: "aliases", aliases: ALIASES });
  s = H.next(s, { type: "pick", alias: "svrapp" });
  assert.equal(s.user, "deploy");
  assert.equal(s.port, "2222");
  assert.equal(H.destination(s), "svrapp");
  s = H.next(s, { type: "pick", alias: "" });
  assert.equal(s.alias, "");
  assert.equal(s.user, "");
});

test("a typed address becomes user@host, or ssh:// when the port is not 22", () => {
  const H = load();
  const typed = (address, user, port) =>
    H.destination(Object.assign(H.initial(), { address, user, port }));
  assert.equal(typed("10.0.0.5", "me", "2222"), "ssh://me@10.0.0.5:2222");
  assert.equal(typed("10.0.0.5", "me", "22"), "me@10.0.0.5");
  assert.equal(typed("10.0.0.5", "me", ""), "me@10.0.0.5");
  assert.equal(typed("10.0.0.5", "", ""), "10.0.0.5");
  assert.equal(typed("", "me", "22"), "");
});

test("feed carries a split line to the next chunk and drops non-events", () => {
  const H = load();
  const line = JSON.stringify({ event: "check", id: "ralphy", status: "pass" });
  const first = H.feed("", "banner text\n" + line.slice(0, 10));
  assert.deepEqual(first.events, []);
  const second = H.feed(first.rest, line.slice(10) + "\n[1,2]\n");
  assert.equal(second.events.length, 1);
  assert.equal(second.events[0].id, "ralphy");
  assert.equal(second.rest, "");
});

test("an unknown key goes to the identity step; cancel goes back with no keys", () => {
  const H = load();
  const keys = [{ type: "ssh-ed25519", fingerprint: "SHA256:abc" }];
  let s = H.next(H.initial(), { type: "key", key: { state: "unknown", keys } });
  assert.equal(s.step, "identity");
  assert.deepEqual(s.keys, keys);
  s = H.next(s, { type: "cancel-trust" });
  assert.equal(s.step, "connection");
  assert.deepEqual(s.keys, []);
  for (const state of ["known", "proxied"]) {
    assert.equal(H.next(H.initial(), { type: "key", key: { state } }).step, "checks");
  }
  assert.equal(H.next(s, { type: "trusted" }).step, "checks");
});

test("check events upsert by id, fixed turns a fix into a pass, ready follows", () => {
  const H = load();
  const ev = (event) => ({ type: "event", event });
  let s = H.next(H.initial(), ev({ event: "check", id: "ralphy", status: "copy", text: "old", command: "ralphy update" }));
  assert.equal(s.checks.length, 1);
  assert.equal(s.checks[0].command, "ralphy update");
  assert.equal(H.ready(s), false);
  s = H.next(s, ev({ event: "check", id: "ralphy", status: "pass", text: "Ralphy 0.1" }));
  assert.equal(s.checks.length, 1);
  assert.equal(H.ready(s), true);
  s = H.next(s, ev({ event: "check", id: "autostart", status: "fix", command: "ralphy daemon install" }));
  assert.equal(H.ready(s), true, "a fix is what Connect does");
  s = H.next(s, ev({ event: "fixed", id: "autostart" }));
  assert.equal(s.checks[1].status, "pass");
  s = H.next(s, ev({ event: "note", text: "Restarted the daemon on svrapp." }));
  assert.deepEqual(s.lines, ["Restarted the daemon on svrapp."]);
  assert.equal(H.ready(H.initial()), false, "no checks yet");
});

test("a name check to copy blocks Connect and asks for a name", () => {
  const H = load();
  const s = H.next(H.initial(), {
    type: "event",
    event: { event: "check", id: "name", status: "copy", command: "ralphy host add svrapp --name <name>" },
  });
  assert.equal(H.ready(s), false);
  assert.equal(H.needsName(s), true);
  const taken = H.next(H.initial(), {
    type: "event",
    event: { event: "check", id: "name", status: "copy", command: "ralphy daemon setup --name <other-name> --avatar 1" },
  });
  assert.equal(H.needsName(taken), false, "a name taken on the host is fixed there");
});

test("an unreachable host opens the help panel; a refused key does not", () => {
  const H = load();
  const failed = (kind) =>
    H.next(H.initial(), { type: "event", event: { event: "failed", kind, message: "m" } });
  assert.equal(failed("unreachable").help, true);
  assert.equal(failed("auth_refused").help, false);
  assert.deepEqual(failed("auth_refused").failure, { kind: "auth_refused", message: "M", keyLine: null }, "shown alone, it starts a sentence");
  const s = H.next(H.initial(), { type: "key", key: { state: "unreachable", reason: "refused" } });
  assert.equal(s.help, true);
  assert.equal(s.failure.kind, "unreachable");
  assert.equal(H.ready(s), false);
});

test("a refused key hands over the peer key line to copy", () => {
  const H = load();
  const line = "ssh-ed25519 BODY ralphy-peer@anvil";
  const s = H.next(H.initial(), {
    type: "event",
    event: { event: "failed", kind: "auth_refused", message: "refused", key_line: line },
  });
  assert.equal(s.failure.keyLine, line);
  assert.equal(H.ready(s), false);
});

test("an install event offers the install until the checks run again", () => {
  const H = load();
  let s = H.next(H.initial(), { type: "event", event: { event: "check", id: "ralphy", status: "copy", command: "ralphy host install svrapp" } });
  assert.equal(H.needsInstall(s), false, "no offer, no button");
  s = H.next(s, {
    type: "event",
    event: { event: "install", version: "v0.1.0-rc.30", target: "linux-x64", source: "release", folder: "~/.ralphy/bin" },
  });
  assert.equal(H.needsInstall(s), true);
  assert.equal(H.installText(s), "Ralphy v0.1.0-rc.30 for linux-x64, downloaded from its release, into ~/.ralphy/bin on the host.");
  const local = H.next(s, { type: "event", event: { event: "install", version: "v1", target: "t", source: "this-computer", folder: "f" } });
  assert.match(H.installText(local), /from this computer/);
  const again = H.next(H.next(s, { type: "check-again" }), {
    type: "event",
    event: { event: "check", id: "ralphy", status: "copy", command: "download Ralphy by hand" },
  });
  assert.equal(H.needsInstall(again), false, "a new run that makes no offer shows no button");
  const passed = H.next(s, { type: "event", event: { event: "check", id: "ralphy", status: "pass" } });
  assert.equal(H.needsInstall(passed), false, "an installed Ralphy needs no button");
});

test("check again empties the list and the failure", () => {
  const H = load();
  let s = H.next(H.initial(), { type: "event", event: { event: "check", id: "ralphy", status: "pass" } });
  s = H.next(s, { type: "event", event: { event: "failed", kind: "unreachable", message: "x" } });
  s = H.next(s, { type: "check-again" });
  assert.deepEqual(s.checks, []);
  assert.equal(s.failure, null);
  assert.equal(s.help, false);
});

test("add exit 0 closes the dialog; any other code keeps it open with a failure", () => {
  const H = load();
  const open = Object.assign(H.initial(), { open: true });
  assert.equal(H.next(open, { type: "exit", verb: "host.add", code: 0 }).open, false);
  assert.equal(H.next(open, { type: "exit", verb: "host.check", code: 0 }).open, true);
  const failed = H.next(open, { type: "event", event: { event: "failed", kind: "other", message: "no" } });
  const s = H.next(failed, { type: "exit", verb: "host.add", code: 1 });
  assert.equal(s.open, true);
  assert.equal(s.failure.message, "No");
  const silent = H.next(open, { type: "exit", verb: "host.add", code: 2 });
  assert.match(silent.failure.message, /code 2/);
});

test("the password is dropped once signed in, on a refusal and on close", () => {
  const H = load();
  const typed = Object.assign(H.initial(), { open: true, step: "checks", password: "s3cret" });
  const event = (e) => ({ type: "event", event: e });
  assert.equal(H.next(typed, event({ event: "connected", os: "Linux" })).password, "");
  const refused = H.next(typed, event({ event: "failed", kind: "password_refused", message: "the host h refused the password" }));
  assert.equal(refused.password, "");
  assert.equal(refused.step, "connection", "back to the field where it is typed again");
  assert.equal(refused.failure.message, "The host h refused the password");
  const keys = H.next(typed, event({ event: "failed", kind: "auth_refused", message: "m" }));
  assert.equal(keys.step, "connection", "a refused key asks for the password");
  const closed = H.next(typed, { type: "close" });
  assert.equal(closed.open, false);
  assert.equal(closed.password, "");
  assert.equal(H.next(typed, event({ event: "failed", kind: "other", message: "m" })).step, "checks");
});

test("the help panel has a tab per system with the commands that work", () => {
  const H = load();
  const tabs = H.helpTabs();
  assert.deepEqual(tabs.map((t) => t.id), ["windows", "macos", "linux"]);
  const commands = (id) => tabs.find((t) => t.id === id).steps.map((st) => st.command).filter(Boolean);
  const texts = (id) => tabs.find((t) => t.id === id).steps.map((st) => st.text || "").join(" ");
  assert.ok(commands("windows").includes("Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0"));
  assert.ok(commands("windows").includes("Get-NetFirewallRule -Name OpenSSH-Server-In-TCP"));
  assert.ok(commands("windows").includes("whoami"));
  assert.match(texts("windows"), /PIN/);
  assert.match(texts("macos"), /Remote Login/);
  assert.match(texts("macos"), /System Settings/);
  assert.match(texts("macos"), /System Preferences/);
  assert.ok(commands("linux").some((c) => c.includes("openssh-server")));
  assert.doesNotMatch(JSON.stringify(tabs), /launchctl/);
  const wrong = JSON.stringify(H.WRONG_ADDRESS);
  assert.match(wrong, /refused/);
  assert.match(wrong, /ipconfig getifaddr en0/);
  assert.equal(H.next(H.initial(), { type: "help-tab", tab: "linux" }).helpTab, "linux");
});

// ---- the shell: thin methods over WBDaemon, driven with stubs ----

import { loadShell, UI } from "./harness.mjs";

const VPS_ID = "01TUNNELPEER0000000000000A";
const line = (o) => JSON.stringify(o) + "\n";

function shell(opts = {}) {
  const { state, window } = loadShell(opts);
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
  state.loadRepos = async () => {
    reloads.push(1);
    state.projects = [{ key: VPS_ID + "/me/app", slug: "me/app", path: "/srv/app", daemon: VPS_ID }];
    state.fleetPeers = [
      { daemon_id: VPS_ID, name: "vps", environment: "Linux", state: "reachable", tunnel: true },
    ];
  };
  const verbs = () => calls.map((c) => c.verb);
  return { state, calls, replies, scripts, reloads, verbs };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("shell: the dialog opens with a fleet of one and lists the SSH config hosts", async () => {
  const { state, verbs } = shell();
  state.fleetPeers = [];
  state.openAddHost();
  assert.equal(state.addHost.open, true);
  assert.deepEqual(verbs(), ["host.aliases"]);
  await tick();
  assert.deepEqual(state.addHost.aliases.map((a) => a.alias), ["svrapp", "web"]);
});

test("shell: an unknown host shows its key first, and Cancel writes nothing", async () => {
  const { state, replies, verbs } = shell();
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

test("shell: Trust sends the fingerprint shown, then runs the checks", async () => {
  const { state, replies, calls, verbs } = shell();
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

test("shell: the checks render from output chunks, and Check again runs them again", async () => {
  const { state, replies, scripts, calls } = shell();
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

test("shell: Install runs host.install with the dialog's payload, then checks again", async () => {
  const { state, replies, scripts, calls } = shell();
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

test("shell: a failed install keeps the offer and runs no checks", async () => {
  const { state, replies, scripts, calls } = shell();
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

test("shell: Connect adds the host and reloads the tree with no page reload", async () => {
  const { state, replies, scripts, reloads, calls } = shell();
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
  assert.equal(state.addHost.open, false);
  await tick();
  const group = state.fleetGroups().find((g) => g.daemon === VPS_ID);
  assert.equal(state.groupLabel(group), "vps: Linux");
});

test("shell: a failed Connect keeps the dialog open and does not reload", async () => {
  const { state, replies, scripts, reloads } = shell();
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

test("the add button sits in the Projects header, outside any group header", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const refresh = html.indexOf('class="side-refresh"');
  const add = html.indexOf('@click="openAddHost()"');
  const count = html.indexOf('class="count"');
  assert.ok(refresh > 0 && refresh < add && add < count, `${refresh} < ${add} < ${count}`);
  assert.ok(add < html.indexOf('class="env-group"'), "not inside a group header");
});

test("shell: Remove host runs host.remove with the daemon id and the token choice", () => {
  const { state, scripts, calls, reloads } = shell();
  const g = { daemon: VPS_ID, name: "vps", tunnel: true };
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

test("shell: a failed Remove host stays open with the line to remove by hand", () => {
  const { state, scripts, reloads } = shell();
  scripts["host.remove"] = [
    { status: "output", chunk: line({ event: "note", text: "Forgot vps on this computer." }) },
    { status: "output", chunk: line({ event: "failed", kind: "unreachable", message: "vps did not answer. The key line remains" }) },
    { status: "exited", code: 1 },
  ];
  state.openRemoveHost({ daemon: VPS_ID, name: "vps", tunnel: true });
  state.confirmRemoveHost();
  assert.equal(reloads.length, 0);
  assert.equal(state.removeHost.open, true);
  assert.equal(state.removeHost.busy, false);
  assert.deepEqual(state.removeHost.lines, ["Forgot vps on this computer."]);
  assert.match(state.removeHost.failure.message, /key line remains/);
});

test("the group menu offers Remove host on tunnel groups only", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  assert.match(html, /@contextmenu\.prevent="if \(g\.tunnel\) showGroupMenu\(/);
  assert.match(html, /class="group-menu" x-show="g\.tunnel"/);
  const { state } = shell();
  let items = null;
  state.renderMenu = (x, y, list) => (items = list);
  state.showGroupMenu(1, 2, { daemon: VPS_ID, name: "vps", tunnel: true });
  assert.deepEqual(items.map((i) => i.label), ["Remove host…"]);
  items[0].run();
  assert.equal(state.removeHost.open, true);
  assert.equal(state.removeHost.daemon, VPS_ID);
});

test("the host dialogs render host text with x-text only", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const start = html.indexOf("scrim('addHost.open'");
  const end = html.indexOf("Login gate", start);
  assert.ok(start > 0 && end > start);
  const block = html.slice(start, end);
  assert.ok(block.includes("scrim('removeHost.open'"), "both dialogs are in the block");
  assert.doesNotMatch(block, /x-html|innerHTML/);
});

test("shell: a typed password goes with the check, only when signing in without a key file", async () => {
  const { state, replies, calls } = shell();
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

test("shell: over plain http from the network the password is not sent", () => {
  const { state } = shell({
    window: { location: { protocol: "http:", host: "192.168.1.5:7257", hostname: "192.168.1.5", pathname: "/", search: "" } },
  });
  state.openAddHost();
  state.addHostType("address", "10.1.1.4");
  state.addHostType("password", "s3cret");
  assert.equal(state.hostPasswordAllowed(), false);
  assert.equal(state.addHostPayload().password, undefined);
});
