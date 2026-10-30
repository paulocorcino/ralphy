// Unit tests for assets/ui/wb-spend-panel.ts, the Alpine component of the Spend
// tab: the spend document, the Ledger rows, the period and the pane. The
// component is built with `loadComponent`, so every test here also fails when
// it reads a shell() name it does not list in `uses`, or assigns a shell()
// field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { assembledPage, bindingNames, loadComponent, loadShell, partMarkup, withoutComments } from "./harness.mjs";

// The part that holds the Spend tab's markup.
const PART = "wb-spend-panel.part.html";

// One page with the shell and the Spend tab, and a window that delivers the
// events it is sent, as the browser does. `init()` adds the listeners, as
// Alpine does when it builds the component.
function page() {
  const listeners = {};
  const loaded = loadShell({
    window: {
      addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
      dispatchEvent: (e) => {
        for (const fn of listeners[e.type] || []) fn(e);
        return true;
      },
    },
  });
  const spend = loadComponent("wbSpend", { from: loaded, magics: { $nextTick: (fn) => fn() } });
  spend.scope.init();
  const fire = (type, detail = null) => loaded.window.dispatchEvent({ type, detail });
  return { loaded, shell: loaded.state, spend: spend.scope, fire };
}

// ---- the component and its markup ---------------------------------------------

test("wbSpend lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbSpend");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbSpend reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbSpend");
  const markup = partMarkup(PART);
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variables of the two panes.
  const loopVars = new Set(["p", "t", "d", "o", "m", "r", "c", "i", "peer", "cause", "col", "row"]);
  // A template literal's text is not code: only its `${…}` parts are read.
  const code = (value) => value.replace(/`([^`]*)`/g, (_, t) => Array.from(t.matchAll(/\$\{([^}]*)\}/g), (m) => m[1]).join(", "));
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component.
    if (attr === "x-data") continue;
    for (const n of bindingNames(code(value))) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 60, `the markup was read: ${read} names`);
});

test("nothing outside the Spend element names its state", () => {
  // The page without the Spend part: the shell and the other components.
  const outside = withoutComments(assembledPage("index.html", { skip: PART }));
  for (const name of ["spendView", "spendModel", "ledgerModel", "ledgerView", "spendPane", "spendPeriod", "setSpendPane", "setSpendPeriod", "showUnpricedLedger", "loadLedger", "loadSpend", "refreshSpend"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
  // The rail button asks for the tab with an event: the element hears it.
  assert.match(outside, /\$dispatch\('workbench:spend-open'\)/);
  assert.match(partMarkup(PART), /@workbench:spend-open\.window="openSpend\(\)"/);
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the Spend tab", () => {
  const { shell, spend, fire } = page();
  const calls = [];
  spend.refreshSpend = () => calls.push("refresh");
  spend.loadSpend = () => calls.push("load");
  shell.$store.projects.setOpen("o/r");
  fire("workbench:spend-refresh");
  assert.deepEqual(calls, ["refresh"]);
  // The tab came to the front: only a document of another project is read again.
  spend.spend = { loading: false, error: "", doc: {}, slug: "o/r" };
  fire("workbench:spend-shown");
  assert.deepEqual(calls, ["refresh"]);
  spend.spend = { loading: false, error: "", doc: {}, slug: "o/x" };
  fire("workbench:spend-shown");
  assert.deepEqual(calls, ["refresh", "refresh"]);
  // The open panels read again, and a closed Spend tab reads nothing.
  shell.$store.tabs.tabs = shell.$store.tabs.tabs.filter((t) => t.id !== "spend");
  fire("workbench:panels-reread");
  assert.deepEqual(calls, ["refresh", "refresh"]);
  shell.$store.tabs.tabs.push({ id: "spend", kind: "spend" });
  fire("workbench:panels-reread");
  assert.deepEqual(calls, ["refresh", "refresh", "load"]);
});

test("opening the Spend tab asks the shell for its tab, then reads it", () => {
  const { spend, loaded } = page();
  const order = [];
  // The shell's own `init()` hears the request (app.test.mjs).
  loaded.window.addEventListener("workbench:tab-open", (e) => order.push(`tab:${e.detail.tab.id}:${e.detail.tab.kind}`));
  spend.loadSpend = () => order.push("load");
  spend.openSpend();
  assert.deepEqual(order, ["tab:spend:spend", "load"], "the tab is asked for before the read");
});

// ---- the views ------------------------------------------------------------------

test("spendView shows the spend document only for the project that is open", () => {
  const { shell, spend } = page();
  spend.spend = { loading: false, error: "", doc: { total: "~$1.00" }, slug: "owner/a" };
  assert.equal(spend.spendView().kind, "empty", "no project open");
  shell.$store.projects.setOpen("owner/a");
  assert.equal(spend.spendView().kind, "ready");
  assert.equal(spend.spendView().total, "~$1.00");
  // A document read for another project is stale: the pane waits for its own.
  shell.$store.projects.setOpen("owner/b");
  assert.equal(spend.spendView().kind, "loading");
});

test("the Spend view takes the titles the board already holds, and loads nothing", () => {
  const { loaded, shell, spend } = page();
  shell.$store.projects.setOpen("o/r");
  shell.loadBoard = () => assert.fail("the Spend view must not load the board");
  shell.$store.board.boardIssues = { "o/r": [{ number: 251, title: "the retry" }], "o/x": [{ number: 9, title: "other" }] };
  loaded.window.WBSpend = { state: (view) => view };
  assert.deepEqual(spend.spendView().issues, [{ number: 251, title: "the retry" }]);
  // CONTROL: a store that holds nothing for the project gives no titles.
  shell.$store.board.boardIssues = {};
  assert.deepEqual(spend.spendView().issues, []);
});

test("a period change drops the Ledger rows and reads the pane that is shown", () => {
  const { spend } = page();
  const calls = [];
  spend.loadSpend = () => calls.push("spend");
  spend.loadLedger = () => calls.push("ledger");
  spend.ledger.slug = "o/r";
  spend.setSpendPeriod("7d");
  assert.deepEqual([spend.spendPeriod, spend.ledger.slug, calls], ["7d", null, ["spend"]]);
  spend.setSpendPane("ledger");
  spend.setSpendPeriod("30d");
  assert.deepEqual(calls, ["spend", "ledger", "spend", "ledger"]);
});

// ---- the Ledger read --------------------------------------------------------------

// `GET /api/usage` answered by `reply` (a response, or a throw for no answer).
async function withUsage(reply, run) {
  const real = globalThis.fetch;
  const urls = [];
  globalThis.fetch = async (url) => {
    urls.push(String(url));
    if (reply instanceof Error) throw reply;
    return reply;
  };
  try {
    return await run(urls);
  } finally {
    globalThis.fetch = real;
  }
}

const usageReply = (ok, body) => ({ ok, status: ok ? 200 : 500, json: async () => body });

test("a Ledger read keeps the rows of the open project and the period", async () => {
  const { shell, spend } = page();
  shell.$store.projects.setOpen("o/r");
  spend.spendPeriod = "7d";
  const body = { records: [{ id: 1 }], interactive: "junk", missing: [{ peer: "p" }], daemon_id: "d1" };
  await withUsage(usageReply(true, body), async (urls) => {
    await spend.loadLedger();
    assert.match(urls[0], /project=o%2Fr/);
    assert.match(urls[0], /period=7d/);
  });
  assert.deepEqual(
    [spend.ledger.records, spend.ledger.interactive, spend.ledger.missing, spend.ledger.daemonId, spend.ledger.slug, spend.ledger.error],
    [[{ id: 1 }], [], [{ peer: "p" }], "d1", "o/r", ""],
  );
});

test("a refused or unanswered Ledger read says so and shows no rows", async () => {
  for (const reply of [usageReply(false, {}), new Error("socket closed")]) {
    const { shell, spend } = page();
    shell.$store.projects.setOpen("o/r");
    await withUsage(reply, () => spend.loadLedger());
    assert.equal(spend.ledger.error, "Could not load the ledger: the daemon did not answer.");
    assert.deepEqual(spend.ledger.records, []);
    assert.equal(spend.ledger.loading, false);
  }
});

test("a Ledger read that lands after a project switch writes no rows", async () => {
  const { shell, spend } = page();
  shell.$store.projects.setOpen("o/r");
  await withUsage(usageReply(true, { records: [{ id: 1 }] }), async () => {
    const read = spend.loadLedger();
    shell.$store.projects.setOpen("o/x");
    await read;
  });
  assert.deepEqual([spend.ledger.records, spend.ledger.slug, spend.ledger.loading], [[], null, false]);
});
