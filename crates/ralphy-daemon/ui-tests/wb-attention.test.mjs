// Unit tests for assets/ui/wb-attention.ts, the attention list on the rail:
// the count in the tab title, and the row's one action, "Go to the console".
// The list itself is the daemon's (`GET /api/attention`); the reply here is
// the shared one the Rust route test wrote (fixtures/api-attention.json).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { bindingNames, partMarkup } from "./harness.mjs";
import { LINK_BEAT_MS, LINK_TRIES, wbAttention } from "../assets/ui/wb-attention.ts";
import { WBView } from "../assets/ui/wb-client-view.ts";
import { connectionStore } from "../assets/ui/wb-connection-store.ts";

const HERE = dirname(fileURLToPath(import.meta.url));

// The part that holds the rail button and its panel.
const PART = "wb-attention.part.html";
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
const GLOBALS = ["window", "document", "WBConsole", "fetch", "Notification", "localStorage"];

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
  globalThis.window = {
    addEventListener() {},
    dispatchEvent() {},
    setInterval: () => 0,
    clearInterval() {},
    location: { search: "", href: "http://127.0.0.1:7431/" },
  };
  globalThis.document = { title: "Ralphy", hidden: false, addEventListener() {} };
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
  const markup = partMarkup(PART);
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
  const markup = partMarkup(PART);
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

test("every name the markup of wbAttention reads is its own", (t) => {
  const { data } = page(t);
  const markup = partMarkup(PART);
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

// The browser's notifications: a class that keeps what it was asked, and the
// storage of this device.
function notifications(permission, answer = permission) {
  const shown = [];
  const asked = [];
  const stored = new Map();
  globalThis.Notification = class {
    static permission = permission;
    static async requestPermission() {
      asked.push(true);
      this.permission = answer;
      return answer;
    }
    constructor(title, options) {
      shown.push({ title, ...options });
    }
  };
  // The per-client view keeps one record under one key.
  globalThis.localStorage = { getItem: (k) => stored.get(k) ?? null, setItem: (k, v) => stored.set(k, v) };
  window.focus = () => {};
  const view = () => JSON.parse(stored.get(WBView.KEY) ?? "{}");
  return { shown, asked, stored, view };
}

const waitingRow = () => fixture("api-attention").requests[0];

test("the notification is off by default", async (t) => {
  const { data } = page(t, { replies: [{ requests: [waitingRow()] }] });
  const { shown, asked, stored, view } = notifications("default", "granted");
  data.init();
  await data.read();
  assert.equal(data.notifyOn, false, "off until the operator turns it on");
  assert.deepEqual(asked, [], "no permission is asked before the switch");
  assert.equal(stored.size, 0);
  assert.deepEqual(shown, []);
  assert.match(partMarkup(PART), /Notify me in this browser/);

  await data.setNotify(true);
  assert.equal(asked.length, 1, "turning it on asks once");
  assert.equal(data.notifyOn, true);
  assert.equal(view().notify, true, "the choice is kept on this device");
  await data.setNotify(false);
  assert.equal(data.notifyOn, false);
  assert.equal(view().notify, false);

  // A browser that already allowed notifications does not turn the switch on
  // by itself; the saved choice does.
  const granted = notifications("granted");
  data.init();
  assert.equal(data.notifyOn, false, "nothing saved: off");
  granted.stored.set(WBView.KEY, JSON.stringify({ v: 1, notify: true }));
  data.init();
  assert.equal(data.notifyOn, true, "saved on, and allowed: on");
});

test("a refused permission leaves the switch off and says so", async (t) => {
  const { data } = page(t);
  const { view } = notifications("default", "denied");
  const flashed = [];
  data.$store.modals = { flash: (m) => flashed.push(m) };
  await data.setNotify(true);
  assert.equal(data.notifyOn, false);
  assert.equal(view().notify, false);
  assert.equal(flashed.length, 1, "the operator is told");
});

test("the notification fires only when the tab is hidden", async (t) => {
  const first = waitingRow();
  const second = { ...first, id: "console:2", console: "2" };
  const { data } = page(t, { projects: [{ slug: first.repo, name: "plain" }] });
  const { shown } = notifications("granted");
  await data.setNotify(true);

  data.show([]);
  document.hidden = false;
  data.show([first]);
  assert.deepEqual(shown, [], "a visible tab shows none");

  document.hidden = true;
  data.show([first, second]);
  assert.equal(shown.length, 1, "a hidden tab shows one for the new request");
  assert.equal(shown[0].tag, "console:2");
  assert.equal(shown[0].title, "plain");
  data.show([first, second]);
  assert.equal(shown.length, 1, "a request the page already knew shows none");

  await data.setNotify(false);
  data.show([first, second, { ...first, id: "console:3" }]);
  assert.equal(shown.length, 1, "the switch off shows none");
});

test("what is on the page at load is not new", async (t) => {
  const { data } = page(t);
  const { shown } = notifications("granted");
  await data.setNotify(true);
  document.hidden = true;
  data.show([waitingRow()]);
  assert.deepEqual(shown, []);
});

test("opening the console of a done request dismisses it", async (t) => {
  const waiting = waitingRow();
  const done = { ...waiting, id: "done:1:1", kind: "console-done", detail: undefined, dismissable: true };
  const session = { id: 1, repo: waiting.repo, agent: "claude", kind: "agent", started_at: 1, daemon_id: "", environment: "", record: "w-local" };
  const { data } = page(t, { sessions: [session], replies: [{ requests: [] }] });
  assert.equal(data.kindOf(done), "The agent is done");
  assert.equal(data.actionOf(done), "console");
  assert.equal(data.canDismiss(done), true);
  const sent = [];
  globalThis.fetch = async (url, init) => {
    sent.push([init.method, url, init.body]);
    return { ok: true, status: 200, json: async () => ({ requests: [] }) };
  };
  const inside = (record) => ({ closest: () => ({ _deskId: record }) });
  data.requests = [done, waiting];

  await data.focused(inside("w-other"));
  await data.focused({ closest: () => null });
  assert.deepEqual(sent, [], "another window, or none, dismisses nothing");

  data.focused(inside("w-local"));
  await new Promise((resolve) => setImmediate(resolve));
  const posts = sent.filter(([method]) => method === "POST");
  assert.deepEqual(posts, [["POST", "/api/attention/dismiss", JSON.stringify({ id: "done:1:1" })]]);
});

test("a console url focuses the console after load and leaves the url", async (t) => {
  const session = (id, daemon_id, record) => ({ id, repo: "owner/app", agent: "claude", kind: "agent", started_at: 1, daemon_id, environment: "", record });
  const { data, reveals, activated } = page(t, {
    sessions: [session(7, "01OTHER", "w-other"), session(7, PEER, "w-linked")],
  });
  let beat = null;
  const urls = [];
  Object.assign(window, {
    location: { search: `?console=${PEER}/7`, href: `http://127.0.0.1:7431/?console=${PEER}/7&x=1` },
    history: { state: null, replaceState: (_state, _title, url) => urls.push(url) },
    setInterval: (fn, ms) => {
      if (ms === LINK_BEAT_MS) beat = fn;
      return 1;
    },
  });
  data.init();
  assert.ok(beat, "a link starts the beat");
  beat();
  assert.deepEqual(reveals, [], "nothing before the session list is read");
  data.$store.projects.sessionsRead = { current: true };
  beat();
  assert.deepEqual(reveals, ["w-linked"], "the console of that daemon and session");
  assert.deepEqual(activated, ["consoles"]);
  assert.deepEqual(urls, ["http://127.0.0.1:7431/?x=1"], "the parameter leaves the url");
  beat();
  assert.deepEqual(reveals, ["w-linked"], "once");
});

test("a console url names a console that is not there: the url is cleaned at the end", async (t) => {
  const { data, reveals } = page(t);
  let beat = null;
  const urls = [];
  Object.assign(window, {
    location: { search: "?console=01GONE/9", href: "http://127.0.0.1:7431/?console=01GONE/9" },
    history: { state: null, replaceState: (_state, _title, url) => urls.push(url) },
    setInterval: (fn, ms) => {
      if (ms === LINK_BEAT_MS) beat = fn;
      return 1;
    },
  });
  data.init();
  for (let n = 0; n <= LINK_TRIES; n++) beat();
  assert.deepEqual(reveals, []);
  assert.deepEqual(urls, ["http://127.0.0.1:7431/"]);
});
