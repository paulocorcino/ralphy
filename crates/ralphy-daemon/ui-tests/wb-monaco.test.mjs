// Unit tests for assets/ui/wb-monaco.ts — the boot seam's two ways of making a
// code editor. Runs the real module against a fake `window.monaco` that only
// records what it was asked to build. The module reads `window` only inside its
// functions, so each test sets the global before it calls.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBMonaco } from "../assets/ui/wb-monaco.ts";

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
  globalThis.window = { monaco };
  return { WBMonaco, calls };
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
