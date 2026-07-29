// Unit tests for assets/ui/wb-runs.js — the run-snapshot mapper and the step
// vocabulary (#330), run against the real source with no DOM. Lives OUTSIDE
// assets/ui on purpose: lib.rs embeds all of assets/ui via include_dir!.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-runs.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBRun;
}

test("fromSnapshot maps the plan block to steps and the issue it belongs to", () => {
  const run = load().fromSnapshot({
    runid: "01X",
    plan: { issue: 72, steps: [{ text: "a", status: "noticed" }, { text: "b", status: "checked" }] },
  });
  assert.equal(run.steps[0].status, "noticed");
  assert.equal(run.steps[0].text, "a");
  assert.equal(run.steps[1].status, "checked");
  assert.equal(run.planIssue, 72);
  assert.equal(run.planReadFailed, false);
});

test("a document with no plan key yields no steps and no plan issue", () => {
  const run = load().fromSnapshot({ runid: "01X" });
  assert.deepEqual(run.steps, []);
  assert.equal(run.planIssue, null);
});

// --- the plan's own issue key (the trailer) ---------------------------------
// The panel keys the plan PROSE on the trailer the planner writes
// (crates/ralphy-adapter-support/src/resume.rs `plan_trailer`), because the file
// on disk belongs to the previous issue for the whole planning phase of the next.
const TRAILER = (n) => `<!-- ralphy-plan: issue=${n} -->`;

test("planTrailerIssue reads the issue the plan says it is for", () => {
  const wb = load();
  assert.equal(wb.planTrailerIssue(`# Plan for #72\n\n## Steps\n- [ ] a\n\n${TRAILER(72)}\n`), 72);
  // Tolerant of the spacing a hand edit or a planner may produce.
  assert.equal(wb.planTrailerIssue("<!--ralphy-plan:issue=8-->"), 8);
});

test("planTrailerIssue is null for prose that names no issue", () => {
  const wb = load();
  for (const md of ["", null, undefined, "# Plan for #72\n\n## Steps\n- [ ] a\n"]) {
    assert.equal(wb.planTrailerIssue(md), null, JSON.stringify(md));
  }
});

// The trailer is NOT required to be the last line — the executor appends
// `## Notes & decisions` / `## Handoff` after it while it works, and the Rust
// resume detector's last-line rule would make the prose vanish the moment
// execution wrote its first note.
test("planTrailerIssue survives sections appended after the trailer", () => {
  const md = `# Plan for #72\n\n## Steps\n- [x] a\n\n${TRAILER(72)}\n\n## Handoff\ndone\n`;
  assert.equal(load().planTrailerIssue(md), 72);
});

test("planTrailerIssue takes the LAST trailer when the prose quotes an earlier one", () => {
  const md = `## Notes\nthe old plan ended with ${TRAILER(71)}\n\n${TRAILER(72)}\n`;
  assert.equal(load().planTrailerIssue(md), 72);
});

test("planBelongsTo answers the question the panel actually asks", () => {
  const wb = load();
  const md = `# Plan for #72\n\n${TRAILER(72)}\n`;
  assert.equal(wb.planBelongsTo(md, 72), true);
  // …and refuses the previous issue's plan, which is the whole defect: the file
  // still holds #71's plan while the run is planning #72.
  assert.equal(wb.planBelongsTo(md, 71), false);
  // Unkeyed prose belongs to nobody — a half-written plan is not this issue's.
  assert.equal(wb.planBelongsTo("# Plan for #72\n", 72), false);
  // No issue to compare against is not a match either (never "belongs to null").
  assert.equal(wb.planBelongsTo(md, null), false);
  assert.equal(wb.planBelongsTo(md, undefined), false);
});

test("planBelongsTo compares by VALUE, so a string issue number still matches", () => {
  const md = `x\n${TRAILER(350)}\n`;
  assert.equal(load().planBelongsTo(md, "350"), true);
});

// --- the plan's verdict, mirrored from the Rust that decides -----------------
// The board's plan pill must agree with the runner: infeasible is ZERO OPEN STEPS
// (`plan::count_open_steps`, read by runner/phases.rs), the reason is the prose
// under `## Feasible…` (`handoff::infeasible_reason`), and a bundle verdict is
// that reason containing "bundle" (`handoff::is_bundle_reason`).
const FEASIBLE_PLAN = `# Plan for #350: a real one

## Feasible: yes
The ground is all present on this branch.

## Steps
- [x] done already
- [ ] still to do

<!-- ralphy-plan: issue=350 -->
`;

const BUNDLE_PLAN = `# Plan for #7: too much

## Feasible: no
The issue bundles six PRD tasks; split into W1-T01..T06.

## Steps

<!-- ralphy-plan: issue=7 -->
`;

test("planSummary reads the trailer, the steps and the verdict together", () => {
  const s = load().planSummary(FEASIBLE_PLAN);
  assert.equal(s.issue, 350);
  assert.equal(s.steps, 2);
  assert.equal(s.openSteps, 1);
  assert.equal(s.infeasible, false);
  assert.equal(s.needsSplit, false);
  assert.equal(s.heading, "Feasible: yes");
  assert.ok(s.reason.includes("ground is all present"), s.reason);
});

test("a plan with no open steps is infeasible, whatever its heading claims", () => {
  const wb = load();
  // The runner's own test: zero open steps IS the refusal. A plan that says
  // "Feasible: yes" and leaves nothing to do would still be skipped by the loop,
  // so the board must not call it ready.
  const lying = "## Feasible: yes\nlooks fine\n\n## Steps\n- [x] all done\n\n<!-- ralphy-plan: issue=9 -->";
  assert.equal(wb.planSummary(lying).infeasible, true);
  assert.equal(wb.planSummary(lying).needsSplit, false);
});

test("a bundle verdict is recognised as needing a split", () => {
  const s = load().planSummary(BUNDLE_PLAN);
  assert.equal(s.issue, 7);
  assert.equal(s.openSteps, 0);
  assert.equal(s.infeasible, true);
  assert.equal(s.needsSplit, true);
  assert.ok(s.reason.includes("split into W1-T01"), s.reason);
});

test("isBundleReason keys on the literal word, case-insensitively", () => {
  const wb = load();
  assert.equal(wb.isBundleReason("This BUNDLES two tasks"), true);
  assert.equal(wb.isBundleReason("under-specified: no acceptance criteria"), false);
  assert.equal(wb.isBundleReason(""), false);
  assert.equal(wb.isBundleReason(undefined), false);
});

test("a missing Feasible section yields no reason rather than a wrong one", () => {
  const wb = load();
  const s = wb.planSummary("# Plan\n\n## Steps\n- [ ] a\n\n<!-- ralphy-plan: issue=4 -->");
  assert.equal(s.heading, "");
  assert.equal(s.reason, "");
  assert.equal(s.infeasible, false);
});

// A plan.md written on Windows, or checked out with `core.autocrlf`. The trailing
// `\r` used to defeat the step regex entirely — every step vanished and the plan
// read as infeasible, on a file the operator could see was full of steps.
test("steps survive CRLF line endings", () => {
  const wb = load();
  const crlf = FEASIBLE_PLAN.replace(/\n/g, "\r\n");
  assert.deepEqual(
    wb.parseSteps(crlf).map((s) => s.status),
    ["checked", "open"],
  );
  assert.equal(wb.parseSteps(crlf)[1].text, "still to do", "no stray carriage return in the text");
  const s = wb.planSummary(crlf);
  assert.equal(s.openSteps, 1);
  assert.equal(s.infeasible, false);
  assert.equal(s.issue, 350);
  assert.equal(s.heading, "Feasible: yes");
  // …and the reason comes back normalised, with no carriage return left in it.
  assert.equal(s.reason, "The ground is all present on this branch.");
});

test("a CRLF plan's sections come back with LF bodies", () => {
  const wb = load();
  const crlf = "## Notes\nfirst\nsecond\n\n## Steps\n- [ ] a\n".replace(/\n/g, "\r\n");
  assert.equal(wb.section(crlf, "Notes"), "first\nsecond");
});

test("planSummary of nothing is a shape, not a crash", () => {
  const wb = load();
  for (const md of ["", null, undefined]) {
    const s = wb.planSummary(md);
    assert.equal(s.issue, null, JSON.stringify(md));
    assert.equal(s.steps, 0);
    // Zero steps reads as infeasible, which is why the CALLER must key on
    // `issue` first: no trailer means there is no plan to judge at all.
    assert.equal(s.infeasible, true);
  }
});

test("the pill says what the operator must not have to open a modal to learn", () => {
  const wb = load();
  const ready = wb.planSummary(FEASIBLE_PLAN);
  const bundle = wb.planSummary(BUNDLE_PLAN);
  assert.equal(wb.planPillLabel(ready), "plan ready");
  assert.equal(wb.planPillWarns(ready), false);
  assert.equal(wb.planPillLabel(bundle), "needs split");
  assert.equal(wb.planPillWarns(bundle), true);
  // A plan for a CLOSED issue is residue, not an invitation — and that outranks
  // its own verdict, because the issue is the thing that is over.
  assert.equal(wb.planPillLabel(ready, false), "leftover plan");
  assert.equal(wb.planPillWarns(ready, false), true);
});

test("an infeasible plan that is not a bundle says so in its own words", () => {
  const wb = load();
  const s = wb.planSummary(
    "## Feasible: no\nUnder-specified: no acceptance criteria.\n\n## Steps\n\n<!-- ralphy-plan: issue=5 -->",
  );
  assert.equal(wb.planPillLabel(s), "not feasible");
  assert.equal(wb.planPillWarns(s), true);
});

test("a noticed step is visually distinct from a checked one", () => {
  const wb = load();
  assert.notEqual(wb.stepGlyph("noticed"), wb.stepGlyph("checked"));
  assert.equal(wb.stepClass("noticed"), "st-noticed");
  assert.equal(wb.stepClass("checked"), "st-checked");
});

test("an unknown status falls back to open rather than vanishing", () => {
  const wb = load();
  assert.equal(wb.stepGlyph("tomorrows_status"), wb.stepGlyph("open"));
  assert.equal(wb.stepLabel("tomorrows_status"), wb.stepLabel("open"));
  assert.equal(wb.stepClass("tomorrows_status"), "st-open");
});

test("a status naming an Object.prototype member still falls back to open", () => {
  const wb = load();
  for (const evil of ["toString", "constructor", "hasOwnProperty"]) {
    assert.equal(wb.stepGlyph(evil), wb.stepGlyph("open"), evil);
    assert.equal(wb.stepLabel(evil), wb.stepLabel("open"), evil);
    assert.equal(wb.stepClass(evil), "st-open", evil);
  }
});

test("parseSteps reads the three markers in order (the file:// demo seed)", () => {
  const steps = load().parseSteps("## Steps\n- [ ] a\n- [x] b\n- [!] c\nprose\n- not a step\n");
  assert.deepEqual(
    steps.map((s) => s.status),
    ["open", "checked", "noticed"],
  );
  assert.deepEqual(
    steps.map((s) => s.text),
    ["a", "b", "c"],
  );
});

// --- verb chrome (#331) -----------------------------------------------------

test("verbLockTitle states each verb's OWN description when nothing holds the lock", () => {
  const wb = load();
  // Per-verb equality, not just "non-empty": a helper returning "x" for all
  // three would satisfy a length check while saying nothing about the verb.
  for (const verb of ["run", "triage", "push"]) {
    const title = wb.verbLockTitle(verb, "");
    assert.equal(title, wb.VERB_TITLE[verb], verb);
    assert.ok(!/lock/i.test(title), `an unlocked verb must not claim a lock: ${title}`);
  }
  assert.equal(new Set(["run", "triage", "push"].map((v) => wb.verbLockTitle(v, ""))).size, 3);
});

test("verbLockTitle states the reason VERBATIM when the lock is held", () => {
  const reason = "a run holds this repo's lock — write controls are disabled until it finishes";
  assert.ok(load().verbLockTitle("triage", reason).includes(reason));
});

test("verbLockTitle falls back for an unknown verb rather than reading undefined", () => {
  const wb = load();
  assert.ok(wb.verbLockTitle("nope", "").includes("nope"));
  // A verb naming an Object.prototype member must not hand back a function —
  // the same guard the status tables carry above.
  for (const evil of ["toString", "constructor", "hasOwnProperty"]) {
    const title = wb.verbLockTitle(evil, "");
    assert.equal(typeof title, "string", evil);
    assert.ok(title.includes(evil), `${evil}: ${title}`);
  }
});

test("exitNote truncates a runaway last line instead of growing without bound", () => {
  const wb = load();
  const note = wb.exitNote("run", 1, "z".repeat(5000));
  assert.ok(note.length < 300, `note length ${note.length}`);
  assert.ok(note.endsWith("…"), note.slice(-20));
});

// The negative control: a note generator that never stays silent is not a gate
// — it would raise a refusal banner on every successful run.
test("exitNote is EMPTY for a clean exit, whatever the last line said", () => {
  assert.equal(load().exitNote("run", 0, "anything"), "");
});

test("exitNote names the verb and carries the CLI's last line on a refusal", () => {
  const note = load().exitNote("run", 1, "working tree … is not clean");
  assert.ok(note.includes("run"), note);
  assert.ok(note.includes("working tree … is not clean"), note);
});

test("exitNote renders a missing code as unknown, never as `exit null`", () => {
  const wb = load();
  for (const code of [null, undefined]) {
    const note = wb.exitNote("push", code, "");
    assert.ok(note.includes("unknown"), note);
    assert.ok(!/null|undefined/.test(note), note);
  }
});
