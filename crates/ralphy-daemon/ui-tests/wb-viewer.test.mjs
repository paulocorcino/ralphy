// Unit tests for assets/ui/wb-viewer.js — the link decision table behind a
// click in a rendered markdown document, and the two-pane paint behind the
// slot (ADR-0037 §3c). The link tests run the real source against a DOM that
// answers nothing: the module only touches `document` at load to find its
// mount point, and `linkTarget` is pure. The slot tests give it a DOM that
// remembers what was appended and a Monaco that boots.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-viewer.js"),
  "utf8",
);

function load() {
  const window = {};
  const document = { getElementById: () => null };
  new Function("window", "document", SRC)(window, document);
  return window.WBViewer;
}

const DIR = "docs/analise";

test("a relative link folds against the document's own directory", () => {
  const { linkTarget } = load();
  assert.deepEqual(linkTarget(DIR, "../backlog/TASKS.md"), {
    kind: "file",
    path: "docs/backlog/TASKS.md",
    fragment: "",
  });
  assert.deepEqual(linkTarget(DIR, "./server.py"), { kind: "file", path: "docs/analise/server.py", fragment: "" });
  assert.deepEqual(linkTarget("", "README.md"), { kind: "file", path: "README.md", fragment: "" });
});

test("a fragment on a file link survives the fold; the path loses it", () => {
  const { linkTarget } = load();
  assert.deepEqual(linkTarget(DIR, "../backlog/TASKS.md#fase-2"), {
    kind: "file",
    path: "docs/backlog/TASKS.md",
    fragment: "fase-2",
  });
});

test("a bare fragment is a jump inside the same document", () => {
  const { linkTarget } = load();
  assert.deepEqual(linkTarget(DIR, "#achados"), { kind: "fragment", fragment: "achados" });
});

test("a scheme or a root-absolute href is the browser's, not ours", () => {
  const { linkTarget } = load();
  assert.deepEqual(linkTarget(DIR, "https://github.com/x/y/issues/1"), { kind: "external" });
  assert.deepEqual(linkTarget(DIR, "mailto:someone@example.com"), { kind: "external" });
  assert.deepEqual(linkTarget(DIR, "/etc/passwd"), { kind: "external" });
});

test("a link that climbs out of the repo is refused before it reaches the daemon", () => {
  const { linkTarget } = load();
  assert.equal(linkTarget(DIR, "../../../secrets.md"), null);
  assert.equal(linkTarget("", "../x.md"), null);
  assert.equal(linkTarget(DIR, ""), null);
});

test("percent-escapes decode into the filename the markdown spelled", () => {
  const { linkTarget } = load();
  assert.deepEqual(linkTarget("", "notas/plano%20final.md"), {
    kind: "file",
    path: "notas/plano final.md",
    fragment: "",
  });
});

// The toolbar label (`pathLabel`): the path only, because the tab names the
// file and the sidebar names the repo; the full `label / path` only where the
// pane is the whole window (detached). `dir` / `file` are the two spans the
// stylesheet lets yield / never yield.
const REPO = "paulocorcino/vibeforge · WSL: Ubuntu-22.04";

test("an attached pane is labelled by its path alone; the full form rides the title", () => {
  const { pathLabel } = load();
  const got = pathLabel({ label: REPO, path: "docs/COMPARATIVO.md", kind: "markdown", detached: false });
  assert.deepEqual(got, {
    dir: "docs/",
    file: "COMPARATIVO.md",
    full: `${REPO} / docs/COMPARATIVO.md`,
  });
});

test("a detached pane keeps the repo and its environment in front of the path", () => {
  const { pathLabel } = load();
  const got = pathLabel({ label: REPO, path: "infra/bootstrap.env.example", kind: "code", detached: true });
  assert.equal(got.dir, `${REPO} / infra/`);
  assert.equal(got.file, "bootstrap.env.example");
  assert.equal(got.dir + got.file, got.full);
});

test("a diff pane keeps its HEAD marker; a root file has no directory to yield", () => {
  const { pathLabel } = load();
  const diff = pathLabel({ label: REPO, path: "README.md", kind: "diff", detached: false });
  assert.deepEqual(diff, { dir: "", file: "README.md ↔ HEAD", full: `${REPO} / README.md ↔ HEAD` });
});

// --- the slot: two panes, a mirror, and the order things are torn down in ----
// The DOM fake records structure only (children, display, grid column, the
// `split` class); Monaco is a boot that resolves and editors/models that log
// their disposal, so the ONE ordering rule — mirror editor before model — is
// observable.
function fakeEl(tag) {
  const classes = new Set();
  const el = {
    tag,
    style: {
      setProperty(k, v) {
        el.style[k] = v;
      },
    },
    dataset: {},
    className: "",
    children: [],
    parent: null,
    classList: {
      add: (c) => classes.add(c),
      remove: (c) => classes.delete(c),
      toggle: (c, force) => {
        const on = force === undefined ? !classes.has(c) : !!force;
        on ? classes.add(c) : classes.delete(c);
        return on;
      },
      contains: (c) => classes.has(c),
    },
    append(...kids) {
      for (const k of kids) {
        k.parent = el;
        el.children.push(k);
      }
    },
    appendChild(k) {
      el.append(k);
    },
    remove() {
      if (el.parent) el.parent.children = el.parent.children.filter((k) => k !== el);
      el.parent = null;
    },
    querySelector: () => fakeEl("q"),
    querySelectorAll: () => [],
    addEventListener() {},
    setAttribute() {},
    focus() {},
  };
  return el;
}

function loadWithDom() {
  const log = [];
  let seq = 0;
  const fakeEditor = (model, tag) => ({
    model,
    layout() {},
    focus() {},
    getModel: () => model,
    getValue: () => model.value,
    onDidChangeModelContent() {},
    addCommand() {},
    updateOptions() {},
    dispose: () => log.push(`editor:${tag}`),
  });
  const monacoStub = {
    KeyMod: { CtrlCmd: 2048 },
    KeyCode: { KeyS: 49 },
    Uri: { file: (p) => p },
    editor: { getModels: () => [] },
  };
  const models = [];
  const WBMonaco = {
    ready: () => Promise.resolve(monacoStub),
    gutterOptions: () => ({}),
    create(container, { value, path }) {
      const model = { value, path, dispose: () => log.push(`model:${path}`) };
      models.push(model);
      return fakeEditor(model, `own:${path}`);
    },
    createOver(container, model) {
      log.push(`over:${model.path}`);
      return fakeEditor(model, `mirror:${model.path}:${++seq}`);
    },
  };
  const window = {
    WB: { emit() {} },
    WBMode: { isDaemon: () => true, isDemo: () => false },
    WBFleet: { refSlug: (p) => p },
    WBMonaco,
    monaco: monacoStub,
    getShell: () => ({ _flashAction() {} }),
    addEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {} }),
  };
  const mount = fakeEl("viewers");
  mount.clientWidth = 1280;
  const document = {
    getElementById: (id) => (id === "viewers" ? mount : null),
    createElement: (tag) => fakeEl(tag),
    addEventListener() {},
    dispatchEvent() {},
  };
  new Function("window", "document", SRC)(window, document);
  for (const k of ["WB", "WBMonaco", "WBMode"]) globalThis[k] = window[k];
  return { viewer: window.WBViewer, mount, log, models };
}
after(() => ["WB", "WBMonaco", "WBMode"].forEach((k) => delete globalThis[k]));
const settle = () => new Promise((r) => setTimeout(r, 5));
const paneOf = (mount, id) => mount.children.find((c) => c.dataset.tabId === id);
const mirrors = (mount) => mount.children.filter((c) => c.className.includes("mirror-viewer"));
const openCode = (viewer, id, path) =>
  viewer.open({ id, project: "p", path, ftype: "code", content: `// ${path}` });

test("one argument to setActive is the single pane every caller had", async () => {
  const { viewer, mount } = loadWithDom();
  openCode(viewer, "a", "a.js");
  openCode(viewer, "b", "b.js");
  await settle();
  viewer.setActive("a");
  assert.equal(paneOf(mount, "a").style.display, "flex");
  assert.equal(paneOf(mount, "b").style.display, "none");
  assert.equal(mount.classList.contains("split"), false);
  assert.equal(paneOf(mount, "a").style.gridColumn, "");
  assert.equal(mirrors(mount).length, 0);
  assert.equal(viewer.width(), 1280);
});

test("a pinned slot shows two panes: the active in column 1, the pin in column 3", async () => {
  const { viewer, mount } = loadWithDom();
  openCode(viewer, "a", "a.js");
  openCode(viewer, "b", "b.js");
  openCode(viewer, "c", "c.js");
  await settle();
  viewer.setActive("a", { id: "b", mirror: false, focus: false, ratio: 0.6 });
  assert.equal(paneOf(mount, "a").style.display, "flex");
  assert.equal(paneOf(mount, "b").style.display, "flex");
  assert.equal(paneOf(mount, "c").style.display, "none");
  assert.equal(paneOf(mount, "a").style.gridColumn, "1");
  assert.equal(paneOf(mount, "b").style.gridColumn, "3");
  assert.equal(mount.classList.contains("split"), true);
  assert.equal(mount.style["--wb-split"], "60.00%");
  assert.ok(mount.children.some((c) => c.className === "viewers-divider"));
  // Back to one pane: the grid goes, the divider stays hidden by the stylesheet.
  viewer.setActive("a", null);
  assert.equal(paneOf(mount, "b").style.display, "none");
  assert.equal(paneOf(mount, "a").style.gridColumn, "");
  assert.equal(mount.classList.contains("split"), false);
});

test("a mirror is a second editor over the SAME model, in a pane of its own — no new model", async () => {
  const { viewer, mount, log, models } = loadWithDom();
  openCode(viewer, "a", "a.js");
  await settle();
  viewer.setActive("a", { id: "a", mirror: true, focus: false, ratio: null });
  assert.equal(models.length, 1);
  assert.deepEqual(log, ["over:a.js"]);
  assert.equal(mirrors(mount).length, 1);
  assert.equal(mirrors(mount)[0].dataset.mirrorOf, "a");
  assert.equal(mirrors(mount)[0].style.gridColumn, "3");
  assert.equal(paneOf(mount, "a").style.gridColumn, "1");
  assert.equal(mount.classList.contains("split"), true);
  // Repainting with the same slot keeps the one mirror.
  viewer.setActive("a", { id: "a", mirror: true, focus: true, ratio: 0.5 });
  assert.equal(mirrors(mount).length, 1);
  assert.deepEqual(log, ["over:a.js"]);
});

test("clearing the slot disposes the mirror editor and leaves the model alone", async () => {
  const { viewer, mount, log } = loadWithDom();
  openCode(viewer, "a", "a.js");
  await settle();
  viewer.setActive("a", { id: "a", mirror: true });
  viewer.setActive("a", null);
  assert.deepEqual(log, ["over:a.js", "editor:mirror:a.js:1"]);
  assert.equal(mirrors(mount).length, 0);
  assert.equal(mount.classList.contains("split"), false);
});

test("closing a mirrored pane tears the mirror down BEFORE its model", async () => {
  const { viewer, mount, log } = loadWithDom();
  openCode(viewer, "a", "a.js");
  await settle();
  viewer.setActive("a", { id: "a", mirror: true });
  viewer.close("a");
  assert.deepEqual(log, ["over:a.js", "editor:mirror:a.js:1", "model:a.js", "editor:own:a.js"]);
  assert.equal(mirrors(mount).length, 0);
  assert.equal(paneOf(mount, "a"), undefined);
});

test("a mirror asked for before the editor mounted paints single, then converges", async () => {
  const { viewer, mount, log } = loadWithDom();
  openCode(viewer, "a", "a.js");
  viewer.setActive("a", { id: "a", mirror: true });
  assert.equal(mount.classList.contains("split"), false);
  assert.deepEqual(log, []);
  await settle();
  assert.equal(mount.classList.contains("split"), true);
  assert.deepEqual(log, ["over:a.js"]);
});

test("a pin whose pane is not open yet paints single; a pane that is not code never mirrors", async () => {
  const { viewer, mount } = loadWithDom();
  openCode(viewer, "a", "a.js");
  viewer.open({ id: "m", project: "p", path: "x.png", ftype: "image", content: "data:image/png;base64," });
  await settle();
  viewer.setActive("a", { id: "gone", mirror: false });
  assert.equal(mount.classList.contains("split"), false);
  viewer.setActive("m", { id: "m", mirror: true });
  assert.equal(mount.classList.contains("split"), false);
  assert.equal(mirrors(mount).length, 0);
  assert.equal(paneOf(mount, "m").style.display, "flex");
});
