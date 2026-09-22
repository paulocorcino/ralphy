// Unit tests for assets/ui/wb-monaco.js — the boot seam's two ways of making a
// code editor. Runs the real source against a fake `window.monaco` that only
// records what it was asked to build: the module touches nothing else at load.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-monaco.js"),
  "utf8",
);

function load() {
  const calls = { create: [], createModel: [] };
  const monaco = {
    Uri: { file: (p) => ({ path: p }) },
    editor: {
      create(container, opts) {
        calls.create.push({ container, opts });
        return { opts };
      },
      createModel(value, lang, uri) {
        const model = { value, uri };
        calls.createModel.push(model);
        return model;
      },
    },
  };
  const window = { monaco };
  new Function("window", "document", SRC)(window, {});
  return { WBMonaco: window.WBMonaco, calls };
}

test("create builds its own model and hands it to one editor", () => {
  const { WBMonaco, calls } = load();
  const container = {};
  WBMonaco.create(container, { value: "x", path: "a.js", uid: 1, project: "p", narrow: false });
  assert.equal(calls.createModel.length, 1);
  assert.equal(calls.create.length, 1);
  assert.equal(calls.create[0].container, container);
  assert.equal(calls.create[0].opts.model, calls.createModel[0]);
});

// The mirror pane (ADR-0037 §3c): a second editor over a model another pane
// owns. No model is created — the oracle `wb_monaco_308.py` counts models per
// open pane, and a mirror must not move it.
test("createOver puts a second editor over the SAME model and creates none", () => {
  const { WBMonaco, calls } = load();
  const model = { value: "shared" };
  const ed = WBMonaco.createOver({}, model, { narrow: false });
  assert.equal(calls.createModel.length, 0);
  assert.equal(calls.create.length, 1);
  assert.equal(calls.create[0].opts.model, model);
  assert.equal(ed.opts.model, model);
});

test("createOver renders with the same options as create, gutter included", () => {
  const { WBMonaco, calls } = load();
  WBMonaco.create({}, { value: "x", path: "a.js", uid: 1, project: "p", narrow: true, wordWrap: "on" });
  WBMonaco.createOver({}, calls.createModel[0], { narrow: true, wordWrap: "on" });
  const [own, over] = calls.create.map((c) => c.opts);
  assert.deepEqual(over, own);
  assert.deepEqual(
    Object.fromEntries(Object.keys(WBMonaco.gutterOptions(true)).map((k) => [k, over[k]])),
    WBMonaco.gutterOptions(true),
  );
});
