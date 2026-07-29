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
