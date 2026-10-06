// Unit tests for assets/ui/wb-add-project-dialog.ts, the Add a project
// dialog's Alpine component (#501). It is built with `loadComponent`, so every
// test here also fails when the component reads a shell() name it does not list
// in `uses`, or assigns a shell() field (ADR-0073 D4). The fold itself is tested
// in wb-add-project.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, UI, withoutComments } from "./harness.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

const WSL_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

const DEV = {
  dir: { path: "C:\\Dev\\", root: null, added: false },
  entries: [
    { name: "ralphy", repo: true, added: true },
    { name: "fincal", repo: true, added: false },
    { name: "notes", repo: false, added: false },
    { name: "locked", repo: false, added: false, error: "cannot read this folder" },
  ],
};

// `state` is the scope the dialog's code sees; `shell` is the shell() object
// around it, where a test sets the projects and reads what a shell() method did.
function dialog(opts = {}) {
  const { scope: state, data, shell, window } = loadComponent(
    "wbAddProjectDialog",
    Object.assign({ magics: { $nextTick: (fn) => fn(), $refs: {} } }, opts),
  );
  const calls = [];
  const replies = {};
  window.WBDaemon.observe = async (verb, payload) => {
    calls.push([verb, payload]);
    return replies[verb];
  };
  const reloads = [];
  shell.loadRepos = async () => reloads.push("repos");
  shell.loadFleet = async () => reloads.push("fleet");
  const toggled = [];
  shell.toggle = (ref) => {
    toggled.push(ref);
    shell.openSlug = ref;
  };
  return { state, data, shell, window, calls, replies, reloads, toggled };
}

function ready(state, text) {
  state.addProject = Object.assign(state.addProject, { open: true });
  state.addProjectStep({ type: "text", text, peers: [] });
  state.addProjectStep({ type: "sent", seq: 1 });
  state.addProjectStep({ type: "reply", seq: 1, reply: Object.assign({ status: "ok", more: 0 }, DEV) });
}

test("dialog: an add selects the new project after the list reloads", async () => {
  const { state, shell, calls, replies, reloads, toggled } = dialog();
  ready(state, "C:\\Dev\\fincal");
  replies["project.add"] = { status: "ok", slug: "o/fincal", path: "C:/Dev/fincal" };
  shell.projects = [{ slug: "o/fincal" }];
  await state.addProjectSubmit();
  assert.deepEqual(calls, [["project.add", { daemon: "", path: "C:\\Dev\\fincal", init: false }]]);
  assert.deepEqual(reloads, ["repos"]);
  assert.equal(state.addProject.open, false);
  assert.deepEqual(toggled, ["o/fincal"]);
});

test("dialog: a peer add selects the peer's row", async () => {
  const { state, shell, replies, reloads, toggled } = dialog();
  ready(state, "C:\\Dev\\fincal");
  state.addProject.daemon = WSL_ID;
  replies["project.add"] = { status: "ok", slug: "o/fincal", path: "/home/me/fincal" };
  shell.projects = [{ slug: "o/fincal", key: `${WSL_ID}/o/fincal`, daemon: WSL_ID }];
  await state.addProjectSubmit();
  assert.deepEqual(reloads, ["repos", "fleet"]);
  assert.deepEqual(toggled, [`${WSL_ID}/o/fincal`]);
});

test("dialog: a refused add keeps the dialog open with the reason", async () => {
  const { state, replies, toggled } = dialog();
  ready(state, "C:\\Dev\\fincal");
  replies["project.add"] = { status: "error", message: "o/fincal is already added from C:/Other/fincal" };
  await state.addProjectSubmit();
  assert.equal(state.addProject.open, true);
  assert.equal(state.addProject.adding, false);
  assert.equal(state.addProject.text, "C:\\Dev\\fincal");
  assert.equal(state.addProject.error, "Could not add the project: o/fincal is already added from C:/Other/fincal.");
  assert.deepEqual(toggled, []);
});

test("dialog: a WSL path with no peer calls no verb", () => {
  const { state, calls } = dialog();
  const listings = [];
  state.addProjectList = (delay) => listings.push(delay);
  state.addProject.open = true;
  state.addProjectText("\\\\wsl.localhost\\Debian\\home");
  assert.deepEqual(listings, []);
  assert.deepEqual(calls, []);
  state.addProjectText("C:\\Dev\\");
  assert.deepEqual(listings, [150], "an ordinary path is listed after the debounce");
});

test("dialog: the second click of a double click is dropped", () => {
  const { state } = dialog();
  state.addProjectList = () => {};
  ready(state, "C:\\Dev\\");
  // The listing of fincal arrived between the two clicks.
  state.addProjectPick({ name: "fincal" }, { detail: 1 });
  state.addProjectStep({ type: "sent", seq: 2 });
  state.addProjectStep({
    type: "reply",
    seq: 2,
    reply: { status: "ok", more: 0, dir: { path: "C:\\Dev\\fincal\\", root: "C:\\Dev\\fincal", added: false }, entries: [{ name: "src", repo: false, added: false }] },
  });
  state.addProjectPick({ name: "src" }, { detail: 2 });
  assert.equal(state.addProject.text, "C:\\Dev\\fincal\\");
});

test("dialog: a pick by touch does not focus the field; by mouse or key it does", () => {
  const { state } = dialog();
  state.addProjectList = () => {};
  let focused = 0;
  state.$refs.addProjectFolder = { focus: () => focused++ };
  ready(state, "C:\\Dev\\");
  state._addProjectTouch = true;
  state.addProjectPick({ name: "fincal" }, { detail: 1 });
  assert.equal(focused, 0);
  state._addProjectTouch = false;
  state.addProjectPick({ name: "notes" }, { detail: 1 });
  state.addProjectPick({ name: "fincal" });
  assert.equal(focused, 2);

  const html = readFileSync(join(UI, "index.html"), "utf8");
  assert.match(html, /id="add-project-list"[^>]*@pointerdown="_addProjectTouch = \$event\.pointerType !== 'mouse'"/);
  assert.match(html, /@click="addProjectPick\(e, \$event\)"/);
});

test("dialog: the places come from the fleet of shell()", () => {
  const { state, shell } = dialog();
  shell.fleetPeers = [{ daemon_id: WSL_ID, name: "ubuntu", environment: "WSL: Ubuntu", state: "reachable" }];
  assert.ok(state.addProjectPlaces().some((p) => p.id === WSL_ID));
});

// ---- the component's contract with shell() and the page ----

const HTML = readFileSync(join(UI, "index.html"), "utf8");

test("the Add a project button comes before Hosts, after refresh", () => {
  const refresh = HTML.indexOf('class="side-refresh"');
  const add = HTML.indexOf(`@click="$dispatch('workbench:add-project-open')"`);
  const hosts = HTML.indexOf(`@click="$dispatch('workbench:hosts-open')"`);
  assert.ok(refresh > 0 && refresh < add && add < hosts, `${refresh} < ${add} < ${hosts}`);
  const tag = HTML.slice(HTML.lastIndexOf("<button", add), HTML.indexOf(">", add));
  assert.match(tag, /title="Add a project"/);
  assert.match(tag, /aria-label="Add a project"/);
  assert.match(HTML.slice(add, HTML.indexOf("</button>", add)), /x-icon="'folder-plus'"/);
});

test("the empty state offers Add a project", () => {
  const at = HTML.indexOf('class="side-empty projects-empty"');
  const block = HTML.slice(at, HTML.indexOf("</div>", at));
  assert.match(block, /x-show="!projects\.length && !reposError && !reposLoading"/);
  assert.match(block, /No projects yet/);
  assert.match(block, /@click="\$dispatch\('workbench:add-project-open'\)"/);
});

test("the dialog shows folder names and errors as text only", () => {
  const dialogHtml = componentMarkup(HTML, "wbAddProjectDialog");
  assert.ok(dialogHtml.length > 0);
  assert.doesNotMatch(dialogHtml, /x-html/);
  assert.match(dialogHtml, /role="alert" x-show="addProject.error" x-text="addProject.error"/);
});

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
  const markup = componentMarkup(HTML, "wbAddProjectDialog");
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
    // `x-ref` names a ref, not an expression.
    if (attr === "x-data" || attr === "x-ref") continue;
    const expr = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
    for (const name of bindingNames(expr)) {
      if (loopVars.has(name) || name.startsWith("$")) continue;
      assert.doesNotThrow(() => state[name], `${attr}="${value}" reads ${name}`);
      read++;
    }
  }
  assert.ok(read > 20, `the markup was read: ${read} names`);
});

test("the dialog's scope refuses a shell() name outside uses, and any shell() write", () => {
  const { state, shell } = dialog();
  assert.ok("waking" in shell);
  assert.throws(() => state.waking, /reads waking/);
  assert.throws(() => {
    state.fleetPeers = [];
  }, /assigns fleetPeers/);
  assert.deepEqual(shell.fleetPeers, [], "the write did not reach shell()");
});

test("both openers ask the dialog to open with workbench:add-project-open", () => {
  const html = withoutComments(HTML);
  for (const opener of [
    html.match(/<button class="side-refresh side-add-project"[^>]*>/)[0],
    html.match(/<button class="btn accent"[^>]*workbench:add-project-open[^>]*>/)[0],
  ]) {
    assert.match(opener, /@click="\$dispatch\('workbench:add-project-open'\)"/);
  }
  assert.match(
    html,
    /<div class="add-project-dialog" x-data="wbAddProjectDialog" @workbench:add-project-open\.window="openAddProject\(\)">/,
  );
  // Nothing outside the component names its state.
  const start = HTML.indexOf('x-data="wbAddProjectDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["addProject", "_addProjectTouch", "openAddProject", "closeAddProject"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

test("the shared reply of project.add selects the slug the reply names", async () => {
  const { state, shell, replies, toggled } = dialog();
  replies["project.add"] = fixture("project.add");
  shell.projects = [{ slug: "o/alpha" }];
  state.addProject = Object.assign(state.addProject, { open: true });
  state.addProjectStep({ type: "text", text: "/srv/alpha/", peers: [] });
  state.addProjectStep({ type: "sent", seq: 1 });
  state.addProjectStep({
    type: "reply",
    seq: 1,
    reply: { status: "ok", more: 0, entries: [], dir: { path: "/srv/alpha/", root: "/srv/alpha", added: false } },
  });
  await state.addProjectSubmit();
  assert.deepEqual(toggled, ["o/alpha"]);
});
