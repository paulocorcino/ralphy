// Unit tests for assets/ui/wb-hosts.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { UI } from "./harness.mjs";

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

const VPS_ID = "01TUNNELPEER0000000000000A";

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

test("add exit 0 shows the last step; any other code keeps the checks with a failure", () => {
  const H = load();
  const open = Object.assign(H.initial(), { open: true, step: "checks" });
  const added = H.next(H.next(open, { type: "event", event: { event: "added", name: "vps" } }), {
    type: "exit",
    verb: "host.add",
    code: 0,
  });
  assert.deepEqual([added.open, added.step, H.addedText(added)], [true, "done", "Added vps. Its projects are now in the list of projects."]);
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

test("Back returns to the fields with what was typed, and clears the checks", () => {
  const H = load();
  let s = H.next(H.initial(), { type: "type", field: "address", value: "10.1.1.4" });
  s = H.next(s, { type: "type", field: "user", value: "ralphy2" });
  s = H.next(s, { type: "key", key: { state: "known" } });
  s = H.next(s, { type: "event", event: { event: "check", id: "ralphy", status: "pass" } });
  s = H.next(s, { type: "event", event: { event: "note", text: "Signed in." } });
  s = H.next(s, { type: "back" });
  assert.equal(s.step, "connection");
  assert.equal(H.destination(s), "ralphy2@10.1.1.4");
  assert.deepEqual([s.checks, s.lines, s.failure], [[], [], null]);
});

test("the install runs by itself only when asked, offered, and not tried yet", () => {
  const H = load();
  const offered = (s) =>
    [
      { event: "check", id: "ralphy", status: "copy", command: "ralphy host install vps" },
      { event: "install", version: "v1", target: "linux-x64", source: "release", folder: "~/.ralphy/bin" },
    ].reduce((x, event) => H.next(x, { type: "event", event }), s);
  const asked = H.next(H.initial(), { type: "type", field: "autoInstall", value: true });
  assert.equal(H.wantsAutoInstall(offered(H.initial())), false, "the check box starts off");
  assert.equal(H.wantsAutoInstall(offered(asked)), true);
  assert.equal(H.wantsAutoInstall(asked), false, "no offer: a newer Ralphy on the host gets none");
  const tried = H.next(offered(asked), { type: "auto-tried" });
  assert.equal(H.wantsAutoInstall(tried), false);
  assert.equal(H.wantsAutoInstall(H.next(tried, { type: "back" })), false, "Back drops the offer");
  assert.equal(
    H.wantsAutoInstall(offered(H.next(tried, { type: "key", key: { state: "known" } }))),
    true,
    "a new Connect may try again",
  );
});

test("the add button sits in the Projects header, outside any group header", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const refresh = html.indexOf('class="side-refresh"');
  const add = html.indexOf(`@click="$dispatch('workbench:hosts-open')"`);
  const count = html.indexOf('class="count"');
  assert.ok(refresh > 0 && refresh < add && add < count, `${refresh} < ${add} < ${count}`);
  assert.ok(add < html.indexOf('class="env-group"'), "not inside a group header");
});

test("each row of Your hosts has Edit and Remove, and Remove asks in that row", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const start = html.indexOf('<ul class="host-list"');
  const end = html.indexOf("</ul>", start);
  assert.ok(start > 0 && end > start);
  const list = html.slice(start, end);
  assert.match(list, /x-for="h in sshHosts\(\)"/);
  assert.match(list, /@click="addHostEdit\(h\)"[^>]*>Edit</);
  assert.match(list, /@click="openRemoveHost\(h\)"[^>]*>Remove</);
  assert.match(list, /x-show="removeHost\.open && removeHost\.daemon === h\.daemon_id"/);
  assert.match(list, /@click="confirmRemoveHost\(\)"/);
  assert.doesNotMatch(html, /scrim\('removeHost\.open'/, "no second dialog for Remove");
});

test("the Hosts dialog renders host text with x-text only", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const start = html.indexOf("scrim('addHost.open'");
  const end = html.indexOf("Login gate", start);
  assert.ok(start > 0 && end > start);
  const block = html.slice(start, end);
  assert.ok(block.includes('class="host-list"'), "the list is in the block");
  assert.doesNotMatch(block, /x-html|innerHTML/);
});

test("fields reads back the form that built a destination", () => {
  const { destination, fields } = load();
  for (const form of [
    { address: "10.0.0.5", user: "deploy", port: "2222" },
    { address: "192.168.101.3", user: "user", port: "" },
    { address: "svrapp", user: "", port: "" },
  ]) {
    const dest = destination(Object.assign({ alias: "" }, form));
    assert.deepEqual(fields(dest), form, dest);
  }
});

test("Edit fills the form from the host, and Save goes back to the list", () => {
  const { initial, next } = load();
  let s = Object.assign(initial(), { open: true, aliases: ALIASES });
  s = next(s, {
    type: "edit",
    host: { daemon_id: VPS_ID, name: "vps", destination: "ssh://root@10.1.1.4:2200", identity_file: "C:/keys/vps" },
  });
  assert.equal(s.tab, "add");
  assert.deepEqual(s.editing, { daemon: VPS_ID, name: "vps" });
  assert.deepEqual([s.address, s.user, s.port, s.signIn, s.keyFile], ["10.1.1.4", "root", "2200", "key", "C:/keys/vps"]);
  assert.equal(s.password, "");
  assert.deepEqual(s.aliases, ALIASES, "the SSH config hosts stay");
  s = next(s, { type: "exit", verb: "host.add", code: 0 });
  assert.equal(s.open, true, "an edit stays in the dialog");
  assert.equal(s.tab, "hosts");
  assert.equal(s.editing, null);
  assert.equal(s.address, "");
  // A new host gets the last step instead.
  s = next(Object.assign(initial(), { open: true, tab: "add" }), { type: "exit", verb: "host.add", code: 0 });
  assert.deepEqual([s.open, s.step], [true, "done"]);
});

test("a tab starts a new form", () => {
  const { initial, next } = load();
  let s = Object.assign(initial(), { open: true, aliases: ALIASES });
  s = next(s, { type: "edit", host: { daemon_id: VPS_ID, name: "vps", destination: "user@mac" } });
  s = next(s, { type: "tab", tab: "add" });
  assert.equal(s.editing, null);
  assert.equal(s.address, "");
  assert.equal(s.tab, "add");
  assert.deepEqual(s.aliases, ALIASES);
  assert.equal(next(s, { type: "tab", tab: "hosts" }).tab, "hosts");
});

test("typing in the Host field leaves the SSH config host", () => {
  const H = load();
  let s = H.next(H.initial(), { type: "aliases", aliases: ALIASES });
  s = H.next(s, { type: "pick", alias: "svrapp" });
  s = H.next(s, { type: "host", value: "10.1.1.4" });
  assert.equal(s.alias, "");
  assert.equal(H.destination(s), "ssh://deploy@10.1.1.4:2222", "user and port stay");
});

test("the checks view shows the one blocking check first, and hides waiting checks", () => {
  const H = load();
  const ev = (event) => ({ type: "event", event: Object.assign({ event: "check" }, event) });
  let s = H.initial();
  for (const c of [
    { id: "ralphy", label: "Ralphy", status: "warn", text: "ralphy is not installed on the host. Build it there" },
    { id: "name", label: "Name", status: "pending", text: "waits for Ralphy on the host" },
    { id: "sleep", label: "Sleep", status: "copy", text: "the host sleeps", command: "sudo pmset -a sleep 0" },
    { id: "user", label: "User", status: "pass", text: "signed in as a normal user" },
  ]) {
    s = H.next(s, ev(c));
  }
  let v = H.view(s);
  assert.deepEqual(v.blocker, {
    id: "ralphy",
    title: "Ralphy is not installed on the host",
    detail: "Build it there",
    command: null,
  });
  assert.deepEqual(v.advice, [], "no advice while something blocks");
  assert.equal(v.passedText, "1 check passed");
  assert.equal(H.primary(Object.assign({}, s, { step: "checks" })), "check");

  s = H.next(s, ev({ id: "ralphy", label: "Ralphy", status: "pass", text: "installed" }));
  s = H.next(s, ev({ id: "name", label: "Name", status: "fix", text: "Ralphy names it mac" }));
  s = H.next(s, ev({ id: "autostart", label: "Start at boot", status: "fix", text: "installs autostart" }));
  v = H.view(s);
  assert.equal(v.blocker, null);
  assert.deepEqual(v.advice.map((c) => c.id), ["sleep"]);
  assert.equal(v.advice[0].text, "The host sleeps");
  assert.equal(v.passedText, "2 checks passed");
  assert.equal(v.fixText, "In the next step, Ralphy also sets up: name, start at boot.");
  assert.equal(H.primary(Object.assign({}, s, { step: "checks" })), "connect");
});

test("the main button is Install while Ralphy can be sent to the host", () => {
  const H = load();
  let s = Object.assign(H.initial(), { step: "checks" });
  s = H.next(s, { type: "event", event: { event: "check", id: "ralphy", status: "copy", command: "ralphy host install mac" } });
  assert.equal(H.primary(s), "check");
  s = H.next(s, { type: "event", event: { event: "install", version: "v1", target: "macos-x64", source: "release", folder: "f" } });
  assert.equal(H.primary(s), "install");
  assert.equal(H.primary(H.initial()), "", "no main button outside the checks");
});

test("the connection fields are never offered to autofill or a password manager", () => {
  const html = readFileSync(join(UI, "index.html"), "utf8");
  const start = html.indexOf("<!-- 1. Connection");
  const end = html.indexOf("<!-- 2. Host identity", start);
  assert.ok(start > 0 && end > start);
  const block = html.slice(start, end);
  const inputs = (block.match(/<input\b[^>]*>/g) || []).filter((i) => !/type="checkbox"/.test(i));
  assert.equal(inputs.length, 5, "host, port, user, password, key file");
  for (const input of inputs) assert.match(input, /autocomplete="off"/, input);
  assert.doesNotMatch(block, /type="password"/, "the type comes from hostSecretType()");
  assert.doesNotMatch(block, /type="radio"/);
});
