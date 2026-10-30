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
  const { calls } = load();
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
  const { calls } = load();
  const model = { value: "shared" };
  const ed = WBMonaco.createOver({}, model, { narrow: false });
  assert.equal(calls.createModel.length, 0);
  assert.equal(calls.create.length, 1);
  assert.equal(calls.create[0].opts.model, model);
  assert.equal(ed.opts.model, model);
});

test("createOver renders with the same options as create, gutter included", () => {
  const { calls } = load();
  WBMonaco.create({}, { value: "x", path: "a.js", uid: 1, project: "p", narrow: true, wordWrap: "on" });
  WBMonaco.createOver({}, calls.createModel[0], { narrow: true, wordWrap: "on" });
  const [own, over] = calls.create.map((c) => c.opts);
  assert.deepEqual(over, own);
  assert.deepEqual(
    Object.fromEntries(Object.keys(WBMonaco.gutterOptions(true)).map((k) => [k, over[k]])),
    WBMonaco.gutterOptions(true),
  );
});

// The boot turns the language services off: the workers are not vendored, so a
// provider left on would ask for a worker that does not exist. `ready()` is
// memoized per module, so each of the two tests below boots its own copy of the
// module (the query on the path makes a new one).
function bootWith(extra) {
  const log = [];
  const service = (name) => ({
    setModeConfiguration: (config) => log.push([name, "mode", config]),
    setOptions: (options) => log.push([name, "options", options]),
    setDiagnosticsOptions: (options) => log.push([name, "diagnostics", options]),
  });
  globalThis.window = {
    require: Object.assign((_modules, onLoad) => onLoad(), { config() {} }),
    monaco: {
      editor: { defineTheme() {} },
      languages: {
        register() {},
        setMonarchTokensProvider() {},
        setLanguageConfiguration() {},
        json: { jsonDefaults: service("json") },
        css: { cssDefaults: service("css") },
        typescript: { typescriptDefaults: service("typescript"), javascriptDefaults: service("javascript") },
        ...extra(service),
      },
    },
  };
  return log;
}

const ALL_OFF = (config) => Object.keys(config).length === 9 && Object.values(config).every((v) => v === false);
const NO_TS_CHECKS = { noSemanticValidation: true, noSyntaxValidation: true, noSuggestionDiagnostics: true };

test("ready turns every language service off, in the order JSON, CSS, HTML, TypeScript, JavaScript", async () => {
  const withHtml = (await import("../assets/ui/wb-monaco.ts?with-html")).WBMonaco;
  const log = bootWith((service) => ({ html: { htmlDefaults: service("html") } }));
  await withHtml.ready();
  assert.deepEqual(
    log.map(([name, what]) => `${name} ${what}`),
    [
      "json mode",
      "json diagnostics",
      "css mode",
      "css options",
      "html mode",
      "typescript mode",
      "typescript diagnostics",
      "javascript mode",
      "javascript diagnostics",
    ],
  );
  for (const [name, what, arg] of log) {
    if (what === "mode") assert.ok(ALL_OFF(arg), `${name} has every provider off`);
  }
  assert.equal("tokens" in log[0][2], false, "JSON keeps its token colors");
  assert.deepEqual(log[1][2], { validate: false, schemaValidation: "ignore" });
  assert.deepEqual(log[3][2], { validate: false });
  assert.deepEqual(log[6][2], NO_TS_CHECKS);
  assert.deepEqual(log[8][2], NO_TS_CHECKS);
});

test("ready skips a language whose service the build does not ship", async () => {
  const without = (await import("../assets/ui/wb-monaco.ts?without-html")).WBMonaco;
  const log = bootWith(() => ({}));
  await without.ready();
  assert.deepEqual(
    log.map(([name, what]) => `${name} ${what}`),
    ["json mode", "json diagnostics", "css mode", "css options", "typescript mode", "typescript diagnostics", "javascript mode", "javascript diagnostics"],
  );
});
