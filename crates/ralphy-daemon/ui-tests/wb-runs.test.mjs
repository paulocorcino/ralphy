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
