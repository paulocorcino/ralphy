// Unit tests for assets/ui/wb-viewer.ts — the link decision table behind a
// click in a rendered markdown document, and the two-pane paint behind the
// slot (ADR-0037 §3c). The link tests run the real module against a DOM that
// answers nothing: `createViewer` only touches `document` to find its mount
// point, and `linkTarget` is pure. The slot tests give it a DOM that
// remembers what was appended and a Monaco that boots.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import { createViewer } from "../assets/ui/wb-viewer.ts";
import { WBMonaco } from "../assets/ui/wb-monaco.ts";

const REAL_MONACO = { ...WBMonaco };

function load() {
  const window = {};
  const document = { getElementById: () => null };
  return createViewer(window, document);
}

const DIR = "docs/analise";

test("linkTarget folds a markdown link against the document's own directory", () => {
  const { linkTarget } = load();
  const file = (path, fragment = "") => ({ kind: "file", path, fragment });
  // [case, document directory, href, expected target; null is refused]
  const rows = [
    ["a relative link", DIR, "../backlog/TASKS.md", file("docs/backlog/TASKS.md")],
    ["a dot-relative link", DIR, "./server.py", file("docs/analise/server.py")],
    ["a link from the repo root", "", "README.md", file("README.md")],
    // A fragment on a file link survives the fold; the path loses it.
    [
      "a fragment on a file link",
      DIR,
      "../backlog/TASKS.md#fase-2",
      file("docs/backlog/TASKS.md", "fase-2"),
    ],
    // A bare fragment is a jump inside the same document.
    ["a bare fragment", DIR, "#achados", { kind: "fragment", fragment: "achados" }],
    // A scheme or a root-absolute href is the browser's, not ours.
    ["an https link", DIR, "https://github.com/x/y/issues/1", { kind: "external" }],
    ["a mailto link", DIR, "mailto:someone@example.com", { kind: "external" }],
    ["a root-absolute href", DIR, "/etc/passwd", { kind: "external" }],
    // A link that climbs out of the repo is refused before it reaches the daemon.
    ["a climb out of the repo", DIR, "../../../secrets.md", null],
    ["a climb out of the root", "", "../x.md", null],
    ["an empty href", DIR, "", null],
    // Percent-escapes decode into the filename the markdown spelled.
    ["percent-escapes", "", "notas/plano%20final.md", file("notas/plano final.md")],
  ];
  for (const [name, dir, href, want] of rows) {
    assert.deepEqual(linkTarget(dir, href), want, name);
  }
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
        if (on) classes.add(c);
        else classes.delete(c);
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
    // Ctrl+S is an editor-scoped action whose disposable the pane keeps.
    addAction: () => ({ dispose: () => log.push(`savekey:${tag}`) }),
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
  const stub = {
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
  // The pane imports the one `WBMonaco`: its boot is replaced for the test and
  // put back by `after`.
  Object.assign(WBMonaco, stub);
  return { viewer: createViewer(window, document), mount, log, models, document };
}
after(() => Object.assign(WBMonaco, REAL_MONACO));
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
  assert.deepEqual(log, ["over:a.js", "savekey:mirror:a.js:1", "editor:mirror:a.js:1"]);
  assert.equal(mirrors(mount).length, 0);
  assert.equal(mount.classList.contains("split"), false);
});

test("closing a mirrored pane tears the mirror down BEFORE its model", async () => {
  const { viewer, mount, log } = loadWithDom();
  openCode(viewer, "a", "a.js");
  await settle();
  viewer.setActive("a", { id: "a", mirror: true });
  viewer.close("a");
  // Everything is disposed, and the mirror's editor goes before the model it
  // sits over — an editor over a disposed model throws on render. The order
  // of the rest is not the behavior.
  for (const event of [
    "over:a.js",
    "savekey:mirror:a.js:1",
    "editor:mirror:a.js:1",
    "savekey:own:a.js",
    "model:a.js",
    "editor:own:a.js",
  ]) {
    assert.ok(log.includes(event), `${event} in ${log.join(", ")}`);
  }
  assert.ok(
    log.indexOf("editor:mirror:a.js:1") < log.indexOf("model:a.js"),
    log.join(", "),
  );
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

test("nothing on screen hides the mirror; it comes back with its editor, not a new one", async () => {
  const { viewer, mount, log } = loadWithDom();
  openCode(viewer, "a", "a.js");
  await settle();
  viewer.setActive("a", { id: "a", mirror: true });
  // Consoles: every pane hidden, the mirror included — not disposed.
  viewer.setActive(null, null);
  assert.equal(mirrors(mount).length, 1);
  assert.equal(mirrors(mount)[0].style.display, "none");
  assert.deepEqual(log, ["over:a.js"]);
  viewer.setActive("a", { id: "a", mirror: true });
  assert.equal(mirrors(mount)[0].style.display, "flex");
  assert.deepEqual(log, ["over:a.js"], "the same editor is shown again");
  // A single paint of a real pane is a closed slot: now it goes.
  viewer.setActive("a", null);
  assert.equal(mirrors(mount).length, 0);
  assert.deepEqual(log, ["over:a.js", "savekey:mirror:a.js:1", "editor:mirror:a.js:1"]);
});

// The notice that replaces a remote image the page's CSP refused
// (`remoteImageNotice`): a short text that names what and where, and a reason
// that matches the policy — `https:` is opt-in, plain `http:` is never admitted.
test("remoteImageNotice names what was refused, where from, and why", () => {
  const { remoteImageNotice } = load();
  const https = "Turn on Remote images in Security settings, then reload the page.";
  // [case, source, alt, expected notice fields]
  const rows = [
    [
      "an https image names its alt, its host and the setting",
      "https://img.shields.io/badge/x-y-blue",
      "License: GPL v3",
      { text: " Image not shown: License: GPL v3 (img.shields.io)", reason: https },
    ],
    [
      "an image with no alt text is called an image",
      "https://example.com/a.png",
      "  ",
      { text: " Image not shown: image (example.com)" },
    ],
    [
      "a plain http image says the setting does not help",
      "http://example.com/a.png",
      "chart",
      { reason: "Images over plain http are not shown." },
    ],
    ["a source that is not a URL gives no host", "not a url", "x", { text: " Image not shown: x" }],
  ];
  for (const [name, src, alt, want] of rows) {
    const notice = remoteImageNotice(src, alt);
    const got = Object.fromEntries(Object.keys(want).map((k) => [k, notice[k]]));
    assert.deepEqual(got, want, name);
  }
});

// --- mermaid in a rendered document ----------------------------------------
// `marked` and `DOMPurify` are page globals; here they pass the HTML through,
// and the article answers the two mermaid `code` elements the HTML would hold:
// a fence inside a <pre>, and inline raw HTML with no <pre> around it.
test("an inline mermaid code element stays as code; a fenced one becomes a diagram holder", () => {
  const { viewer, mount, document } = loadWithDom();
  const html = '<pre><code class="language-mermaid">graph TD</code></pre><p><code class="language-mermaid">x</code></p>';
  const replaced = [];
  const pre = { replaceWith: (holder) => replaced.push(holder) };
  const fenced = { textContent: "graph TD", closest: (s) => (s === "pre" ? pre : null) };
  const inline = { textContent: "x", closest: () => null };
  const article = fakeEl("article");
  article.querySelectorAll = (s) => (s === "code.language-mermaid" ? [fenced, inline] : []);
  const createElement = document.createElement;
  document.createElement = (tag) => {
    const el = createElement(tag);
    const query = el.querySelector;
    el.querySelector = (s) => (s === ".md-body" ? article : query(s));
    return el;
  };
  globalThis.marked = { parse: (src) => (src === "# doc" ? html : "") };
  globalThis.DOMPurify = { sanitize: (h) => h };
  try {
    viewer.open({ id: "m", project: "p", path: "doc.md", ftype: "markdown", content: "# doc" });
  } finally {
    delete globalThis.marked;
    delete globalThis.DOMPurify;
  }
  assert.ok(paneOf(mount, "m"), "the markdown pane is mounted");
  assert.equal(article.innerHTML, html);
  assert.equal(replaced.length, 1);
  assert.equal(replaced[0].className, "mermaid");
  assert.equal(replaced[0].dataset.src, "graph TD");
});
