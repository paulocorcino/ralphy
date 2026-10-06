// Unit tests for assets/ui/wb-add-project.ts — imports the real module with no
// DOM. The dialog's own calls are tested in wb-add-project-dialog.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBAddProject } from "../assets/ui/wb-add-project.ts";

// The module keeps no state, so one import serves every test.
function load() {
  return WBAddProject;
}

const WSL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const PEERS = [
  { daemon_id: WSL_ID, name: "ubuntu", environment: "WSL: Ubuntu", state: "reachable" },
  { daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAX", name: "vps", environment: "Linux", state: "unreachable", diagnosis: "The host does not answer." },
];

// Send one listing for the current text and feed its reply.
function listed(P, s, reply) {
  const seq = s.seq + 1;
  s = P.next(s, { type: "sent", seq });
  return P.next(s, { type: "reply", seq, reply: Object.assign({ status: "ok", more: 0 }, reply) });
}

function typed(P, s, text) {
  return P.next(s, { type: "text", text, peers: PEERS });
}

const DEV = {
  dir: { path: "C:\\Dev\\", root: null, added: false },
  entries: [
    { name: "ralphy", repo: true, added: true },
    { name: "fincal", repo: true, added: false },
    { name: "notes", repo: false, added: false },
    { name: "locked", repo: false, added: false, error: "cannot read this folder" },
  ],
};

test("opening asks for no path, and the start folder fills the field", () => {
  const P = load();
  let s = P.next(P.initial(), { type: "open" });
  assert.deepEqual(P.request(s), { daemon: "" });
  s = listed(P, s, { start: "C:\\Dev", dir: { path: "C:\\Dev\\", root: null, added: false }, entries: [] });
  assert.equal(s.text, "C:\\Dev\\");
  assert.deepEqual(P.request(s), { daemon: "", path: "C:\\Dev\\" });

  let p = P.next(P.initial(), { type: "open" });
  p = listed(P, p, { start: "/home/me", dir: { path: "/home/me/", root: null, added: false }, entries: [] });
  assert.equal(p.text, "/home/me/");
});

test("the start folder is not a choice until the operator makes one", () => {
  const P = load();
  const home = { start: "/home/me", dir: { path: "/home/me/", root: null, added: false }, entries: [] };
  let s = listed(P, P.next(P.initial(), { type: "open" }), home);
  assert.deepEqual(P.primary(s), { label: "Add project", disabled: true });
  s = listed(P, typed(P, s, "/home/me/"), home);
  assert.deepEqual(P.primary(s), { label: "Create repository and add", disabled: false });
});

test("the button names what adding the typed folder does", () => {
  const P = load();
  const cases = [
    ["C:\\Dev\\fincal", "Add project", false],
    ["C:\\Dev\\FINCAL", "Add project", false],
    ["C:\\Dev\\ralphy", "Already in Projects", true],
    ["C:\\Dev\\notes", "Create repository and add", false],
    ["C:\\Dev\\typo", "Create folder and repository", false],
    ["C:\\Dev\\typo ", "A folder name cannot end with a space or a dot", true],
    ["C:\\Dev\\typo.", "A folder name cannot end with a space or a dot", true],
    ["C:\\Dev\\locked", "Cannot read this folder", true],
  ];
  for (const [text, label, disabled] of cases) {
    let s = typed(P, P.next(P.initial(), { type: "open" }), text);
    s = listed(P, s, DEV);
    assert.deepEqual(P.primary(s), { label, disabled }, text);
  }
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\notes"), DEV);
  assert.deepEqual(P.addPayload(s), { daemon: "", path: "C:\\Dev\\notes", init: true });
  s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fincal"), DEV);
  assert.deepEqual(P.addPayload(s), { daemon: "", path: "C:\\Dev\\fincal", init: false });
});

test("a missing last folder is created only by a click, with its path shown", () => {
  const P = load();
  const s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\my notes"), DEV);
  assert.deepEqual(P.primary(s), { label: "Create folder and repository", disabled: false });
  assert.equal(P.help(s), "Ralphy creates the folder C:\\Dev\\my notes and a git repository in it.");
  assert.deepEqual(P.addPayload(s), { daemon: "", path: "C:\\Dev\\my notes", init: true, create: true });
  assert.equal(P.enterAdds(s), false, "Enter does not create a folder");
  const notes = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\notes"), DEV);
  assert.equal(P.enterAdds(notes), true);

  // Inside a repo, a new folder would get a repository inside another one.
  const inside = {
    dir: { path: "/home/me/fincal/", root: "/home/me/fincal", added: false },
    entries: [{ name: "src", repo: false, added: false }],
  };
  const deep = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "/home/me/fincal/new"), inside);
  assert.deepEqual(P.primary(deep), { label: "This folder does not exist", disabled: true });
});

test("a subfolder of a repo adds the repo root, and says so", () => {
  const P = load();
  const inside = {
    dir: { path: "/home/me/fincal/", root: "/home/me/fincal", added: false },
    entries: [{ name: "src", repo: false, added: false }],
  };
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "/home/me/fincal/src"), inside);
  assert.deepEqual(P.primary(s), { label: "Add fincal", disabled: false });
  assert.match(P.help(s), /^Part of fincal\. Ralphy adds all of fincal\.$/);

  // The same folder when its repo is already added.
  const added = Object.assign({}, inside, { dir: Object.assign({}, inside.dir, { added: true }) });
  s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "/home/me/fincal/src"), added);
  assert.deepEqual(P.primary(s), { label: "Already in Projects", disabled: true });

  // A text that ends with a separator chooses the listed folder itself.
  s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "/home/me/fincal/"), inside);
  assert.deepEqual(P.primary(s), { label: "Add project", disabled: false });
});

test("a missing parent folder is refused by the daemon and labelled", () => {
  const P = load();
  let s = typed(P, P.next(P.initial(), { type: "open" }), "C:\\Nope\\x");
  s = P.next(s, { type: "sent", seq: 1 });
  s = P.next(s, { type: "reply", seq: 1, reply: { status: "error", message: "this folder does not exist" } });
  assert.deepEqual(P.primary(s), { label: "This folder does not exist", disabled: true });
});

test("a stale reply never replaces a newer one", () => {
  const P = load();
  let s = typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\f");
  s = P.next(s, { type: "sent", seq: 1 });
  s = typed(P, s, "C:\\Dev\\fincal");
  s = P.next(s, { type: "sent", seq: 2 });
  s = P.next(s, { type: "reply", seq: 2, reply: Object.assign({ status: "ok", more: 0 }, DEV) });
  const newer = s;
  s = P.next(s, { type: "reply", seq: 1, reply: { status: "ok", more: 0, dir: DEV.dir, entries: [] } });
  assert.equal(s, newer, "the older reply changed nothing");
  assert.equal(P.primary(s).label, "Add project");
});

test("a reply to an older text labels nothing until the new text is listed", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fincal"), DEV);
  s = typed(P, s, "C:\\Dev\\ralph");
  assert.deepEqual(P.primary(s), { label: "Add project", disabled: true });
});

test("Loading… shows only for the newest request that is still waiting", () => {
  const P = load();
  let s = P.next(P.initial(), { type: "open" });
  s = P.next(s, { type: "sent", seq: 1 });
  s = P.next(s, { type: "sent", seq: 2 });
  assert.equal(P.next(s, { type: "slow", seq: 1 }).loading, false);
  s = P.next(s, { type: "slow", seq: 2 });
  assert.equal(s.loading, true);
  s = P.next(s, { type: "reply", seq: 2, reply: Object.assign({ status: "ok", more: 0 }, DEV) });
  assert.equal(s.loading, false);

  // A reply faster than the slow timer: the timer fires late and shows nothing.
  for (const reply of [Object.assign({ status: "ok", more: 0 }, DEV), { status: "error", message: "unknown verb" }]) {
    let f = P.next(P.next(P.initial(), { type: "open" }), { type: "sent", seq: 1 });
    f = P.next(f, { type: "reply", seq: 1, reply });
    assert.equal(P.next(f, { type: "slow", seq: 1 }).loading, false, reply.status);
  }
});

test("a peer with an older Ralphy says to update it and hides the Folder field", () => {
  const P = load();
  let s = P.next(P.next(P.initial(), { type: "open", daemon: WSL_ID }), { type: "sent", seq: 1 });
  assert.equal(P.listable(s), true);
  s = P.next(s, { type: "reply", seq: 1, reply: { status: "error", message: "unknown verb" } });
  assert.match(P.help(s), /^Update Ralphy on this computer/);
  assert.equal(P.listable(s), false);
  assert.equal(P.listable(P.next(s, { type: "where", daemon: "" })), true, "another Where brings the field back");
});

test("a pasted WSL path moves to the WSL peer and becomes a Linux path", () => {
  const P = load();
  let s = typed(P, P.next(P.initial(), { type: "open" }), "\\\\wsl.localhost\\Ubuntu\\home\\me\\code");
  assert.equal(s.daemon, WSL_ID);
  assert.equal(s.text, "/home/me/code");
  assert.deepEqual(P.request(s), { daemon: WSL_ID, path: "/home/me/code" });
  s = typed(P, P.next(P.initial(), { type: "open" }), "\\\\wsl$\\ubuntu\\srv");
  assert.equal(s.daemon, WSL_ID, "the distro name matches without regard to case");
  assert.equal(s.text, "/srv");
});

test("a WSL path with no peer says to add the host first, and adds nothing", () => {
  const P = load();
  const s = typed(P, P.next(P.initial(), { type: "open" }), "\\\\wsl.localhost\\Debian\\home");
  assert.equal(s.daemon, "");
  assert.equal(s.wslMissing, "Debian");
  assert.match(P.help(s), /^Add Debian \(WSL\) as a host first\.$/);
  assert.equal(P.primary(s).disabled, true);
});

test("changing Where resets the field to that daemon's start folder", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fincal"), DEV);
  s = P.next(s, { type: "where", daemon: WSL_ID });
  assert.equal(s.text, "");
  assert.equal(s.listing, null);
  assert.deepEqual(P.request(s), { daemon: WSL_ID });
});

test("a peer that does not answer is shown but cannot be chosen", () => {
  const P = load();
  const places = P.places(PEERS);
  assert.deepEqual(places.map((p) => [p.label, p.disabled]), [
    ["This computer", false],
    ["ubuntu (WSL: Ubuntu)", false],
    ["vps (Linux)", true],
  ]);
  assert.equal(places[2].reason, "The host does not answer.");
});

test("picking a folder goes down one level; `..` goes up", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fi"), DEV);
  s = P.next(s, { type: "pick", name: "fincal" });
  assert.equal(s.text, "C:\\Dev\\fincal\\");
  s = typed(P, s, "C:\\Dev\\fincal\\..");
  assert.equal(s.text, "C:\\Dev\\");

  // The drive list: a pick is the drive root.
  let d = typed(P, P.next(P.initial(), { type: "open" }), "");
  d = listed(P, d, { dir: { path: "", root: null, added: false }, entries: [{ name: "D:", repo: false, added: false }] });
  d = P.next(d, { type: "pick", name: "D:" });
  assert.equal(d.text, "D:\\");
});

test("a pick before the next listing arrives names a folder of the rows on screen", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\"), DEV);
  s = P.next(s, { type: "pick", name: "fincal" });
  // The rows are still those of C:\Dev\: the same row again is the same folder.
  s = P.next(s, { type: "pick", name: "fincal" });
  assert.equal(s.text, "C:\\Dev\\fincal\\");
  s = P.next(s, { type: "pick", name: "notes" });
  assert.equal(s.text, "C:\\Dev\\notes\\");

  // A name typed after the rows were listed: a row is still a folder of C:\Dev\.
  s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fi"), DEV);
  s = typed(P, s, "C:\\Dev\\fin");
  s = P.next(s, { type: "pick", name: "fincal" });
  assert.equal(s.text, "C:\\Dev\\fincal\\");
});

test("the `..` row goes up one folder, to the drive list above a drive root", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\"), DEV);
  const rows = P.entries(s);
  assert.deepEqual(rows[0], { name: "..", up: true, repo: false, added: false });
  assert.equal(rows.length, DEV.entries.length + 1);
  s = P.next(s, { type: "pick", name: "..", up: true });
  assert.equal(s.text, "C:\\");
  assert.equal(P.request(s).path, "C:\\");

  s = listed(P, s, { dir: { path: "C:\\", root: null, added: false }, entries: [{ name: "Dev", repo: false, added: false }] });
  assert.equal(P.entries(s)[0].name, "..");
  s = P.next(s, { type: "pick", name: "..", up: true });
  assert.equal(s.text, "");
  assert.equal(P.request(s).path, "");

  // No row above `/`, and none while a name is being typed.
  let l = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "/"), { dir: { path: "/", root: null, added: false }, entries: [] });
  assert.equal(P.entries(l).length, 0);
  let f = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fi"), DEV);
  assert.equal(P.entries(f).length, DEV.entries.length);
});

test("the arrows move the highlight and wrap around", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\"), DEV);
  s = P.next(s, { type: "move", by: 1 });
  assert.equal(s.active, 0);
  s = P.next(s, { type: "move", by: -1 });
  assert.equal(s.active, P.entries(s).length - 1);
});

test("while adding, the controls are locked; a failure keeps the path", () => {
  const P = load();
  let s = listed(P, typed(P, P.next(P.initial(), { type: "open" }), "C:\\Dev\\fincal"), DEV);
  s = P.next(s, { type: "adding" });
  assert.deepEqual(P.primary(s), { label: "Adding…", disabled: true });
  assert.equal(typed(P, s, "C:\\Other").text, "C:\\Dev\\fincal");
  assert.equal(P.next(s, { type: "where", daemon: WSL_ID }).daemon, "");
  s = P.next(s, { type: "addFailed", message: "o/fincal is already added from C:/Dev/fincal" });
  assert.equal(s.open, true);
  assert.equal(s.text, "C:\\Dev\\fincal");
  assert.equal(s.error, "o/fincal is already added from C:/Dev/fincal");
});
