// Unit tests for assets/ui/wb-runs-panel.ts, the Alpine component of the Runs
// panel: the run picker, the issue trail, the plan section, the daemon verbs,
// the run modal and the raw output feed. The component is built with
// `loadComponent`, so every test here also fails when it reads a shell() name
// it does not list in `uses`, or assigns a shell() field (ADR-0073 D4).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, loadShell, UI, withoutComments } from "./harness.mjs";
import { wbAttention } from "../assets/ui/wb-attention.ts";
import { WBRun } from "../assets/ui/wb-runs.ts";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// One page with the shell and the panel, and a window that delivers the
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
  const runs = loadComponent("wbRuns", { from: loaded, magics: { $nextTick: (fn) => fn() } });
  runs.scope.init();
  const fire = (type, detail) => loaded.window.dispatchEvent({ type, detail });
  return { shell: loaded.state, runs: runs.scope, window: loaded.window, fire };
}

// A plan file that carries its own issue key in a trailer; `planBelongsTo`
// reads it.
const plan = (issue) =>
  [
    "## Feasible: yes",
    "## Steps",
    "1. do the thing",
    "## Notes & decisions",
    `<!-- ralphy-plan: issue=${issue} -->`,
  ].join("\n");

// ---- the component and its markup ---------------------------------------------

test("wbRuns lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbRuns");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses" && n !== "init") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbRuns reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbRuns");
  const markup = componentMarkup(HTML, "wbRuns");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  // The `x-for` loop variables of the panel and the modal.
  const loopVars = new Set(["r", "iss", "s", "i", "h", "a"]);
  let read = 0;
  for (const [, attr, value] of attrs) {
    // `x-data` names the component.
    if (attr === "x-data") continue;
    for (const n of bindingNames(value)) {
      if (n.startsWith("$") || loopVars.has(n)) continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 40, `the markup was read: ${read} names`);
});

test("nothing outside the Runs panel element names its state", () => {
  const start = HTML.indexOf('x-data="wbRuns"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  for (const name of ["openRunFor", "focusIssue", "selectRun", "runCfg", "verbError", "rawFeed", "trailFocus", "planSection", "openRunModal", "runIsLive"]) {
    assert.doesNotMatch(outside, new RegExp(`\\b${name}\\b`), name);
  }
});

// ---- the workbench events ------------------------------------------------------

test("each workbench event reaches the Runs panel", () => {
  const { runs, window, fire } = page();
  const opened = [];
  runs.openRunFor = (issue) => opened.push(issue);
  // The board's run pill.
  fire("workbench:run-open", { issue: 72 });
  assert.deepEqual(opened, [72]);
  // The raw output of a run the daemon started, through the window bridge,
  // capped so the DOM never grows unbounded.
  window.WBRuns.output("hello ");
  window.WBRuns.output("world");
  assert.equal(runs.rawFeed, "hello world");
  window.WBRuns.output("x".repeat(9000));
  assert.equal(runs.rawFeed.length, 8000);
  // A terminal verb frame; an empty note is a no-op.
  fire("workbench:run-verb-failed", { message: "cannot run: a run holds the lock" });
  assert.equal(runs.verbError, "cannot run: a run holds the lock");
  fire("workbench:run-verb-failed", { message: "" });
  assert.equal(runs.verbError, "cannot run: a run holds the lock");
});

test("closing the Runs panel drops the board-arrival marker, and opening it keeps the marker", () => {
  const { shell, runs } = page();
  shell.$store.runs.hydrateRuns = () => {};
  shell.toggleRuns();
  runs.trailFocus = 7;
  assert.equal(shell.runsOpen, true);
  shell.toggleRuns();
  assert.equal(shell.runsOpen, false);
  assert.equal(runs.trailFocus, null, "a closed panel keeps no marker");
  // CONTROL: opening sends nothing that drops it.
  runs.trailFocus = 7;
  shell.toggleRuns();
  assert.equal(runs.trailFocus, 7);
});

test("a kept runs read loads the selected run's plan only while the Runs panel shows", () => {
  const { shell, runs, fire } = page();
  let loads = 0;
  runs.loadRunPlan = () => loads++;
  shell.runsOpen = false;
  fire("workbench:runs-read", null);
  assert.equal(loads, 0, "a closed panel reads no plan");
  shell.runsOpen = true;
  fire("workbench:runs-read", null);
  assert.equal(loads, 1);
});

test("the Runs panel drops the trail and the verb refusal of the project that was open", () => {
  const { shell, runs, fire } = page();
  shell.$store.runs.currentRun = () => ({ runid: "run-2" });
  runs.planHeadings = (run) => (run?.runid === "run-2" ? ["Stage 1"] : []);
  Object.assign(runs, { trailFocus: 3, verbError: "z", planSection: "" });
  fire("workbench:project-changed", { slug: "owner/b", previous: "owner/a" });
  // The section is the first of the run the store now selects.
  assert.deepEqual([runs.trailFocus, runs.verbError, runs.planSection], [null, "", "Stage 1"]);
});

// ---- the run agent's state and the attention list ------------------------------

test("a run card shows its agent state", () => {
  const { scope: own } = loadComponent("wbRuns");
  const run = WBRun.fromSnapshot({
    v: 1,
    runid: "01WAIT",
    exec_agent: "claude",
    phase: {
      state: "executing",
      active: 71,
      agent: { state: "waiting", since: "2026-10-28T11:00:00-03:00", detail: "permission: Bash" },
    },
  });
  assert.deepEqual(own.runAgentDot(run), { state: "waiting", detail: "permission: Bash" });
  assert.equal(own.runAgentTitle(run), "Agent is waiting: permission: Bash");
  // A run with no agent state draws no dot.
  assert.equal(own.runAgentDot(WBRun.fromSnapshot({ v: 1, runid: "01OLD" })).state, null);
  // The card draws the dot with the state as its class, and the detail as text.
  const markup = componentMarkup(HTML, "wbRuns");
  assert.match(markup, /class="session-state run-agent-state"[^>]*:class="runAgentDot\(\$store\.runs\.currentRun\(\)\)\.state"/);
  assert.match(markup, /x-text="runAgentDot\(\$store\.runs\.currentRun\(\)\)\.detail"/);
});

test("open the run selects the run in the Runs panel of its project", () => {
  const { shell } = page();
  const { projects, runs, checkouts } = shell.$store;
  Object.assign(projects, { loadAgents() {}, wakePeerFor() {}, refAvailable: () => true });
  checkouts.ensureWorktreeListing = () => {};
  Object.assign(runs, { hydrateRuns() {}, mountRunsSub() {} });
  // The store hears the project change, as Alpine starts it: it then picks the
  // first run of the project it opens.
  runs.init();
  const run = (runid) => WBRun.fromSnapshot({ v: 1, runid });
  runs.runsByProject = { "owner/a": [run("01A"), run("01B")], "owner/b": [run("01C")] };
  projects.setOpen("owner/b");
  const attention = wbAttention();
  attention.open = true;
  attention.openRun({ id: "run:01B", kind: "run-waiting", vendor: "claude", repo: "owner/a", run: "01B", since: "", dismissable: false });
  assert.equal(attention.open, false);
  assert.equal(projects.openSlug, "owner/a", "the run's project opens");
  assert.equal(runs.currentRun()?.runid, "01B", "not the first run of the project");
  assert.equal(shell.runsOpen, true);
});

// ---- the folds -----------------------------------------------------------------

test("clockTitle names both anchors, and says nothing about the ones it lacks", () => {
  const { scope: own, shell } = loadComponent("wbRuns");
  shell.nowMs = Date.parse("2026-09-08T12:30:00Z");
  const t = own.clockTitle({
    since: "2026-09-08T12:00:00Z",
    startedAt: "2026-09-08T10:00:00Z",
  });
  assert.match(t, /^phase since /);
  assert.match(t, / · run started /);
  // The elapsed time is free from `startedAt` — no second anchor is stored.
  assert.match(t, /\(2h 30m\)/);
  // Under an hour drops the hour segment entirely.
  shell.nowMs = Date.parse("2026-09-08T10:07:00Z");
  assert.match(own.clockTitle({ startedAt: "2026-09-08T10:00:00Z" }), /\(7m\)/);
  // NEGATIVE CONTROL: no run, and a run with neither anchor, render empty —
  // never "undefined" or a bare separator.
  assert.equal(own.clockTitle(null), "");
  assert.equal(own.clockTitle({}), "");
});

test("planHeadings drops Steps and stays empty when the prose is for another issue", () => {
  const { scope: own } = loadComponent("wbRuns");
  // The prose belongs to the active issue: its headings render, minus Steps —
  // which the steps block owns and would otherwise appear twice.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: plan(42) }), [
    "Feasible: yes",
    "Notes & decisions",
  ]);
  // `planIssue` wins over `active` when both are present.
  assert.deepEqual(own.planHeadings({ planIssue: 7, active: 42, planMd: plan(7) }), [
    "Feasible: yes",
    "Notes & decisions",
  ]);

  // NEGATIVE CONTROL, and the defect the gate exists for: `.ralphy/plan.md`
  // still holds the PREVIOUS issue's plan between runs. Rendering its outline
  // under the current issue is the lie — so a mismatch renders nothing, not a
  // stale outline.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: plan(41) }), []);
  // A plan with no trailer is mid-write or not a ralphy plan; either way it
  // belongs to no issue.
  assert.deepEqual(own.planHeadings({ active: 42, planMd: "## Feasible: yes" }), []);
  assert.deepEqual(own.planHeadings(null), []);
  assert.deepEqual(own.planHeadings({}), []);
});

test("the plan prose renders only for the issue its trailer names", () => {
  const { scope: own } = loadComponent("wbRuns");
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: plan(42) }), true);
  assert.equal(own.planProseIsCurrent({ planIssue: 7, active: 42, planMd: plan(7) }), true);
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: plan(41) }), false);
  assert.equal(own.planProseIsCurrent({ active: 42, planMd: "## Feasible: yes" }), false);

  // `marked` and `DOMPurify` are vendor globals the harness does not load; an
  // identity pair shows which section text reached them.
  const saved = { marked: globalThis.marked, DOMPurify: globalThis.DOMPurify };
  globalThis.marked = { parse: (text) => text };
  globalThis.DOMPurify = { sanitize: (html) => html };
  try {
    assert.notEqual(own.renderPlanSection({ active: 42, planMd: plan(42) }, "Feasible: yes"), "");
    assert.equal(
      own.renderPlanSection({ active: 42, planMd: plan(41) }, "Feasible: yes"),
      "",
      "a plan written for another issue renders nothing",
    );
  } finally {
    globalThis.marked = saved.marked;
    globalThis.DOMPurify = saved.DOMPurify;
  }
});
