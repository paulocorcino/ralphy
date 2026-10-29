// Unit tests for assets/ui/wb-attention.ts, the attention list on the rail:
// the count in the tab title, and the row's one action, "Go to the console".
// The list itself is the daemon's (`GET /api/attention`); the reply here is
// the shared one the Rust route test wrote (fixtures/api-attention.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { bindingNames, UI, withoutComments } from "./harness.mjs";
import { wbAttention } from "../assets/ui/wb-attention.ts";
import { connectionStore } from "../assets/ui/wb-connection-store.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const PEER = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

function fixture(name) {
  return JSON.parse(readFileSync(join(HERE, "fixtures", name + ".json"), "utf8"));
}

// A peer's copy of a request, as the daemon the page talks to joins it.
function fromPeer(request) {
  return {
    ...request,
    id: `${PEER}/${request.id}`,
    console: `${PEER}/${request.console}`,
    repo: `${PEER}/${request.repo}`,
  };
}

// The component as Alpine builds it, with a window, a document, a fetch that
// answers each read with the next of `replies`, the stores it reads, and the
// console's `reveal`. `init` is not run: each test calls what it needs. The
// test files share one process, so `t.after` puts the globals back.
const GLOBALS = ["window", "document", "WBConsole", "fetch"];

function page(t, { replies = [], sessions = [], projects = [], tabOf = () => undefined } = {}) {
  const saved = GLOBALS.map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  t.after(() => {
    for (const [name, d] of saved) {
      if (d) Object.defineProperty(globalThis, name, d);
      else delete globalThis[name];
    }
  });
  const reveals = [];
  const activated = [];
  globalThis.window = { addEventListener() {}, dispatchEvent() {}, setInterval: () => 0 };
  globalThis.document = { title: "Ralphy" };
  globalThis.WBConsole = { reveal: (id) => reveals.push(id) };
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => replies.shift() });
  const tabs = {
    active: "files",
    tabOf,
    activate(id) {
      activated.push(id);
      tabs.active = id;
    },
  };
  const data = wbAttention();
  data.$store = { projects: { liveSessions: sessions, projects, repoRef: (p) => p.slug }, tabs };
  data.$nextTick = (fn) => fn();
  return { data, reveals, activated };
}

test("the title shows the count of open requests", async (t) => {
  const one = fixture("api-attention").requests[0];
  assert.equal(one.kind, "console-waiting");
  const { data } = page(t, { replies: [{ requests: [one, fromPeer(one)] }, { requests: [] }] });
  await data.read();
  assert.equal(data.count(), 2);
  assert.equal(document.title, "(2) Ralphy");
  await data.read();
  assert.equal(data.count(), 0);
  assert.equal(document.title, "Ralphy");
});

test("go to the console focuses its window", (t) => {
  const one = fixture("api-attention").requests[0];
  const session = (repo, record) => ({ id: 1, repo, agent: "claude", kind: "agent", started_at: 1, daemon_id: "", environment: "", record });
  const { data, reveals, activated } = page(t, {
    sessions: [session(one.repo, "w-local"), session(`${PEER}/${one.repo}`, "w-peer")],
  });
  data.open = true;
  data.goTo(fromPeer(one));
  assert.deepEqual(reveals, ["w-peer"]);
  assert.deepEqual(activated, ["consoles"]);
  assert.equal(data.open, false);
  data.goTo(one);
  assert.deepEqual(reveals, ["w-peer", "w-local"]);
});

test("go to the console shows a console in its own tab there", (t) => {
  const one = fixture("api-attention").requests[0];
  const { data, reveals, activated } = page(t, {
    sessions: [{ id: 1, repo: one.repo, agent: "claude", kind: "agent", started_at: 1, daemon_id: "", environment: "", record: "w-local" }],
    tabOf: (id) => (id === "w-local" ? { id: "console:w-local" } : undefined),
  });
  data.goTo(one);
  assert.deepEqual(activated, ["console:w-local"]);
  assert.deepEqual(reveals, []);
});

test("a row names the console's project, not its slug", (t) => {
  const one = { ...fixture("api-attention").requests[0], repo: "path-d087a294b65ada0a" };
  const { data } = page(t, { projects: [{ slug: "path-d087a294b65ada0a", name: "plain" }] });
  assert.equal(data.nameOf(one), "plain");
  assert.equal(data.nameOf({ ...one, repo: "owner/other" }), "owner/other");
});

test("a run's row opens the run, and a console's row goes to the console", (t) => {
  const { data } = page(t);
  const sent = [];
  window.dispatchEvent = (e) => sent.push([e.type, e.detail]);
  const run = { id: "run:01R", kind: "run-waiting", vendor: "codex", repo: "owner/r", run: "01R", issue: "71", since: "", dismissable: false };
  assert.equal(data.actionOf(run), "run");
  assert.equal(data.kindOf(run), "Waits for you");
  data.open = true;
  data.openRun(run);
  assert.equal(data.open, false);
  assert.deepEqual(sent, [["workbench:run-select", { repo: "owner/r", runid: "01R" }]]);
  for (const kind of ["console-waiting", "console-silent"]) {
    assert.equal(data.actionOf({ ...run, kind }), "console", kind);
  }
  // Each action is one button of the row, shown by the row's action.
  const markup = attentionElement();
  assert.match(markup, /x-show="actionOf\(r\) === 'console'" @click="goTo\(r\)">Go to the console</);
  assert.match(markup, /x-show="actionOf\(r\) === 'run'" @click="openRun\(r\)">Open the run</);
});

test("only a finished run can be dismissed", async (t) => {
  // The rows as the daemon sends them: a waiting console, a waiting run, a
  // silent console, and the issues a finished run left.
  const waiting = fixture("api-attention").requests[0];
  const finished = fixture("api-attention--finished").requests;
  const run = { ...waiting, id: "run:01R", kind: "run-waiting", console: undefined, run: "01R" };
  const silent = { ...waiting, kind: "console-silent" };
  const { data } = page(t, { replies: [{ requests: [] }] });
  for (const row of [waiting, run, silent]) {
    assert.equal(data.canDismiss(row), false, row.kind);
  }
  assert.equal(finished[0].kind, "run-finished");
  assert.equal(data.canDismiss(finished[0]), true);
  assert.equal(data.actionOf(finished[0]), "none", "a finished run is no longer in the Runs panel");
  assert.equal(data.kindOf(finished[0]), "A run stopped on this issue");
  const markup = attentionElement();
  assert.match(markup, /x-show="canDismiss\(r\)" @click="dismiss\(r\)">Dismiss</);

  // "Dismiss" posts the id, then reads the list again.
  const sent = [];
  const replies = { POST: fixture("api-attention-dismiss"), GET: { requests: [finished[1]] } };
  globalThis.fetch = async (url, init) => {
    sent.push([init.method, url, init.body]);
    return { ok: true, status: 200, json: async () => replies[init.method] };
  };
  data.$store.modals = { flash: (m) => assert.fail(`flashed ${m}`) };
  await data.dismiss(finished[0]);
  assert.deepEqual(sent[0], ["POST", "/api/attention/dismiss", JSON.stringify({ id: finished[0].id })]);
  assert.equal(sent[1][0], "GET");
  assert.deepEqual(data.requests.map((r) => r.id), [finished[1].id]);
});

test("an attention.dirty push reads the list again", (t) => {
  page(t);
  const sent = [];
  window.dispatchEvent = (e) => sent.push(e.type);
  document.querySelector = () => null;
  const store = connectionStore({ messages: { flash() {} }, detached: { dirty: () => false, close() {} } });
  store.onPresencePush("attention.dirty", fixture("attention.dirty"));
  assert.deepEqual(sent, ["workbench:attention-reread"]);
});

// The rail button and its panel, from the element's `<div` to the `</div>`
// that closes it, with comments cut out.
function attentionElement() {
  const html = withoutComments(readFileSync(join(UI, "index.html"), "utf8"));
  const start = html.lastIndexOf("<div", html.indexOf('x-data="wbAttention"'));
  let depth = 0;
  for (const m of html.slice(start).matchAll(/<div\b|<\/div>/g)) {
    depth += m[0] === "</div>" ? -1 : 1;
    if (depth === 0) return html.slice(start, start + m.index + m[0].length);
  }
  throw new Error("the attention element is not closed");
}

test("every name the markup of wbAttention reads is its own", (t) => {
  const { data } = page(t);
  const markup = attentionElement();
  let read = 0;
  for (const [, attr, value] of markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)) {
    if (attr === "x-data" || attr === "x-for" || attr === ":key") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || n === "r") continue;
      assert.ok(n in data, `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read >= 8, `the markup was read: ${read} names`);
});
