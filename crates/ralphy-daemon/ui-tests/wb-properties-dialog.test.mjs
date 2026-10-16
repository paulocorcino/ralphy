// Unit tests for assets/ui/wb-properties-dialog.ts, the Properties dialog's
// Alpine component. It is built with `loadComponent`, so every test here also
// fails when the component reads a shell() name it does not list in `uses`, or
// assigns a shell() field (ADR-0073 D4). The scrim is driven in
// wb-modals.test.mjs.
import { after, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { bindingNames, componentMarkup, loadComponent, UI, withoutComments } from "./harness.mjs";

const HTML = readFileSync(join(UI, "index.html"), "utf8");

// The component names `WBDaemon` bare, which Node resolves through
// `globalThis`: the fake is set there, and taken down after this file.
after(() => delete globalThis.WBDaemon);

// The replies the daemon's own tests wrote (ADR-0070, shared replies). Their
// `modified` is a marker there: the time of a file made a moment ago cannot be pinned.
function fixture(name) {
  return JSON.parse(readFileSync(join(UI, "..", "..", "ui-tests", "fixtures", name + ".json"), "utf8"));
}

const STAT_FILE = {
  status: "ok",
  name: "a.txt",
  kind: "file",
  size: 12698,
  modified: Date.UTC(2026, 9, 15, 19, 48),
  readonly: false,
  ignored: false,
  type: { class: "text", encoding: "UTF-8", bom: false },
};
const STAT_FOLDER = { status: "ok", name: "src", kind: "folder", modified: Date.UTC(2026, 9, 15, 19, 48), readonly: false, ignored: true };
const SIZE = { status: "ok", bytes: 3435973837, files: 214, dirs: 3, ignored_bytes: 1024, truncated: false };

// `answers` maps a verb to its reply, or to a function that returns a promise,
// so a test can hold a reply back. Every ask is recorded.
function dialog(answers = {}) {
  const { scope: state, shell, window } = loadComponent("wbPropertiesDialog", { window: { dispatchEvent: () => true } });
  const calls = [];
  window.WBDaemon = {
    observe: async (verb, payload) => {
      calls.push({ verb, payload });
      const answer = answers[verb];
      return typeof answer === "function" ? answer(payload) : (answer ?? { status: "error", reason: "not found" });
    },
    withCheckout: (payload, checkout) => (checkout ? { ...payload, checkout: String(checkout) } : { ...payload }),
  };
  globalThis.WBDaemon = window.WBDaemon;
  state.$store.projects.setOpen("o/r");
  return { state, shell, calls };
}

const tick = () => new Promise((r) => setImmediate(r));
const valueOf = (state, label) => state.propsRows().find((r) => r.label === label)?.value;
const labels = (state) => state.propsRows().map((r) => r.label);

test("wbPropertiesDialog lists only shell() names in uses, and its members left shell()", () => {
  const { data, shell } = loadComponent("wbPropertiesDialog");
  for (const n of data.uses) assert.ok(n in shell, n);
  for (const n of Object.keys(data)) {
    if (n !== "uses") assert.ok(!(n in shell), `shell() still has ${n}`);
  }
});

test("every name the markup of wbPropertiesDialog reads is its own or in its uses list", () => {
  const { scope: state } = loadComponent("wbPropertiesDialog");
  const markup = componentMarkup(HTML, "wbPropertiesDialog");
  const attrs = [...markup.matchAll(/\s(x-[\w:.-]+|[@:][\w:.-]+)="([^"]*)"/g)];
  let read = 0;
  for (const [, attr, value] of attrs) {
    if (attr === "x-data") continue;
    const raw = attr === "x-for" ? value.split(/\s+in\s+/)[1] : value;
    for (const n of bindingNames(raw)) {
      // `r` is the row of the `x-for`.
      if (n.startsWith("$") || n === "r") continue;
      assert.doesNotThrow(() => state[n], `${attr}="${value}" reads ${n}`);
      read++;
    }
  }
  assert.ok(read > 8, `the markup was read: ${read} names`);
  // The files open it with this event; nothing outside names its state.
  assert.match(
    HTML,
    /<div class="properties-dialog" x-data="wbPropertiesDialog" @workbench:properties-open\.window="openProperties\(\$event\.detail\)">/,
  );
  const start = HTML.indexOf('x-data="wbPropertiesDialog"');
  const outside = withoutComments(HTML.slice(0, start) + HTML.slice(HTML.indexOf("<!-- ===", start)));
  assert.doesNotMatch(outside, /\bprops\.open|\bpropsRows\b/);
});

test("a file is asked once, in the checkout of the tree, and shows its facts", async () => {
  const { state, shell, calls } = dialog({ "file.stat": STAT_FILE });
  shell.$store.checkouts.checkouts = { "o/r": "wt-a" };
  state.$store.projects.setProjects([{ slug: "o/r", root: "C:\\Dev\\r" }]);

  state.openProperties({ path: "src/a.txt", isFolder: false });
  await tick();

  assert.deepEqual(calls, [{ verb: "file.stat", payload: { repo: "o/r", path: "src/a.txt", checkout: "wt-a" } }]);
  assert.deepEqual(state.propsRows(), [
    { label: "Name", value: "a.txt" },
    { label: "Path", value: "src/a.txt" },
    { label: "Full path", value: "C:\\Dev\\r\\src\\a.txt" },
    { label: "Size", value: "12.4 KB (12,698 bytes)" },
    { label: "Type", value: "Text · UTF-8" },
    { label: "Modified", value: state.propsRows().find((r) => r.label === "Modified").value },
    { label: "Read-only", value: "No" },
    { label: "Ignored by git", value: "No" },
  ]);
  assert.match(valueOf(state, "Modified"), /^\d{2} Oct 2026, \d{2}:\d{2}$/);
});

test("the dialog reads the replies the daemon wrote: the facts of a file and the size of a folder", async () => {
  const stat = { ...fixture("file.stat"), modified: Date.UTC(2026, 9, 15, 19, 48) };
  const file = dialog({ "file.stat": stat });
  file.state.openProperties({ path: "visible.txt", isFolder: false });
  await tick();
  assert.deepEqual(
    ["Size", "Type", "Read-only", "Ignored by git"].map((l) => valueOf(file.state, l)),
    ["5 bytes", "Text · UTF-8", "No", "No"],
  );

  const folder = dialog({ "file.stat": { ...stat, kind: "folder" }, "tree.size": fixture("tree.size") });
  folder.state.openProperties({ path: "node_modules", isFolder: true });
  await tick();
  assert.deepEqual(
    ["Size", "Contains", "Ignored contents"].map((l) => valueOf(folder.state, l)),
    ["1 byte", "1 file, 0 folders", "0 bytes"],
  );
});

test("until file.stat answers, the facts read … and nothing is called a size", () => {
  const { state } = dialog({ "file.stat": () => new Promise(() => {}) });

  state.openProperties({ path: "a.txt", isFolder: false });

  assert.equal(state.propsBusy(), true);
  assert.deepEqual(
    state.propsRows().map((r) => [r.label, r.value]),
    [
      ["Name", "a.txt"],
      ["Path", "a.txt"],
      ["Size", "…"],
      ["Type", "…"],
      ["Modified", "…"],
      ["Read-only", "…"],
      ["Ignored by git", "…"],
    ],
  );
});

test("a folder asks for its facts and its size, and the slow rows spin until the size is back", async () => {
  let release;
  const { state, calls } = dialog({
    "file.stat": STAT_FOLDER,
    "tree.size": () => new Promise((r) => (release = r)),
  });

  state.openProperties({ path: "src", isFolder: true });
  await tick();

  assert.deepEqual(calls.map((c) => c.verb), ["file.stat", "tree.size"]);
  const slow = state.propsRows().filter((r) => r.pending);
  assert.deepEqual(slow, [
    { label: "Size", value: "Calculating…", pending: true },
    { label: "Contains", value: "Calculating…", pending: true },
  ]);
  assert.equal(valueOf(state, "Ignored by git"), "Yes");
  assert.equal(state.propsBusy(), true);
  assert.equal(state.propsStatus(), "Calculating the size of the folder…");

  release(SIZE);
  await tick();

  assert.equal(state.propsBusy(), false);
  assert.equal(valueOf(state, "Size"), "3.2 GB (3,435,973,837 bytes)");
  assert.equal(valueOf(state, "Contains"), "214 files, 3 folders");
  assert.equal(valueOf(state, "Ignored contents"), "1.0 KB (1,024 bytes)");
  assert.equal(state.propsRows().some((r) => r.pending), false);
  assert.equal(state.propsStatus(), "The size of the folder is ready.");
});

test("a count that stopped early is a minimum and says why", async () => {
  const { state } = dialog({ "file.stat": STAT_FOLDER, "tree.size": { ...SIZE, truncated: true } });

  state.openProperties({ path: "src", isFolder: true });
  await tick();

  assert.equal(valueOf(state, "Size"), "≥ 3.2 GB (3,435,973,837 bytes)");
  assert.equal(valueOf(state, "Note"), "The count stopped after 5 s, so these numbers are a minimum.");
});

test("a second count is refused as busy, with a new try that asks again", async () => {
  let answer = { status: "error", reason: "busy" };
  const { state, calls } = dialog({ "file.stat": STAT_FOLDER, "tree.size": () => answer });

  state.openProperties({ path: "src", isFolder: true });
  await tick();

  const busy = state.propsRows().find((r) => r.label === "Size");
  assert.deepEqual(busy, { label: "Size", value: "Another size is being calculated. Try again.", retry: true });

  answer = SIZE;
  state.propsRetrySize();
  await tick();

  assert.equal(calls.filter((c) => c.verb === "tree.size").length, 2);
  assert.equal(valueOf(state, "Contains"), "214 files, 3 folders");
});

test("an older daemon is named, and the dialog does not pretend to know the facts", async () => {
  const { state } = dialog({ "file.stat": { status: "error", message: "unknown verb" } });

  state.openProperties({ path: "a.txt", isFolder: false });
  await tick();

  assert.deepEqual(labels(state), ["Name", "Path", "Facts"]);
  assert.equal(valueOf(state, "Facts"), "This host runs an older daemon. Update it to see properties.");
});

test("a refusal is a reason, not an empty dialog", async () => {
  const { state } = dialog({});

  state.openProperties({ path: "gone.txt", isFolder: false });
  await tick();

  assert.equal(valueOf(state, "Facts"), "Could not read the properties: it does not exist.");
});

test("the project root is a folder, named for the project", async () => {
  const { state, calls } = dialog({ "file.stat": { ...STAT_FOLDER, ignored: false }, "tree.size": SIZE });
  state.$store.projects.setProjects([{ slug: "o/r", root: "C:\\Dev\\r" }]);

  state.openProperties({ path: "", isFolder: false });
  await tick();

  assert.equal(state.props.isFolder, true);
  assert.deepEqual(calls.map((c) => c.verb), ["file.stat", "tree.size"]);
  assert.equal(valueOf(state, "Name"), `${state.$store.projects.projectLabel("o/r")} (project root)`);
  assert.equal(valueOf(state, "Full path"), "C:\\Dev\\r");
  assert.equal(labels(state).includes("Path"), false, "the root has no rel path");
});

test("a reply for a row that is no longer the open one is dropped", async () => {
  const releases = {};
  const { state } = dialog({
    "file.stat": (payload) => new Promise((r) => (releases[payload.path] = r)),
  });

  state.openProperties({ path: "a.txt", isFolder: false });
  state.openProperties({ path: "b.txt", isFolder: false });
  releases["b.txt"]({ ...STAT_FILE, size: 2 });
  await tick();
  releases["a.txt"]({ ...STAT_FILE, size: 1 });
  await tick();

  assert.equal(valueOf(state, "Size"), "2 bytes", "the late reply of a.txt must not overwrite b.txt");
});

test("a reply that arrives after the dialog was closed is dropped", async () => {
  let release;
  const { state } = dialog({ "file.stat": () => new Promise((r) => (release = r)) });

  state.openProperties({ path: "a.txt", isFolder: false });
  state.propsClose();
  release(STAT_FILE);
  await tick();

  assert.equal(state.props.open, false);
  assert.equal(state.props.stat, null);
});

test("Copy writes the visible lines, one per line, and leaves out a row that is still counting", async () => {
  const { state } = dialog({ "file.stat": STAT_FOLDER, "tree.size": () => new Promise(() => {}) });
  const written = [];
  const real = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    value: { clipboard: { writeText: async (text) => written.push(text) } },
    configurable: true,
  });
  try {
    state.openProperties({ path: "src", isFolder: true });
    await tick();
    state.propsCopy();
  } finally {
    if (real) Object.defineProperty(globalThis, "navigator", real);
    else delete globalThis.navigator;
  }

  assert.equal(written.length, 1);
  const lines = written[0].split("\n");
  assert.deepEqual(lines.slice(0, 2), ["Name: src", "Path: src"]);
  assert.equal(lines.some((l) => l.startsWith("Size")), false, "a row with a spinner has no value yet");
  assert.equal(state.props.copied, true);
});
