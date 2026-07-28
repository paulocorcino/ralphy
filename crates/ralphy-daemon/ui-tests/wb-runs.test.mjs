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
