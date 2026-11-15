// Unit tests for assets/ui/wb-viewer-scripts.ts — the "Run scripts" control
// of an HTML file pane. The daemon and the DOM are fakes; the three browser
// engines are checked by tests/browser/files/wb_html_file.py.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createViewerScripts, previewUrl } from "../assets/ui/wb-viewer-scripts.ts";

// The reply the daemon writes, shared with the daemon's own tests.
function fixture(name) {
  return JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), "utf8"));
}

const settle = () => new Promise((resolve) => { setTimeout(resolve, 5); });

function element(tag) {
  const held = {};
  return { tag, className: "", title: "", src: "", setAttribute: (k, v) => { held[k] = v; }, held };
}

// One pane, a fake daemon that answers `file.preview` with `reply`, and what
// the control did.
function pane(reply, over = {}) {
  const log = { asked: [], flashed: [], drawn: 0, captions: [], frames: [] };
  const button = { classList: { toggle: (name, on) => log.captions.push([name, on]) } };
  const article = {
    innerHTML: "<p>static</p>",
    append: (frame) => log.frames.push(frame),
  };
  const rec = {
    id: "file:o/r:docs/page.html",
    project: "o/r",
    path: "docs/my page.html",
    checkout: null,
    editing: false,
    dirty: false,
    el: { querySelector: (sel) => (sel === ".md-body" ? article : button) },
    ...over,
  };
  globalThis.WBDaemon = {
    withCheckout: (payload, checkout) => (checkout ? { ...payload, checkout } : payload),
    observe: (verb, payload) => {
      log.asked.push([verb, payload]);
      return Promise.resolve("revoke" in payload ? { status: "ok" } : reply);
    },
  };
  const scripts = createViewerScripts({
    document: { createElement: element },
    messages: { flash: (text) => log.flashed.push(text) },
    alive: () => over.closed !== true,
    setCaption: (_btn, icon, caption) => log.captions.push([icon, caption]),
    renderStatic: (r) => {
      log.drawn += 1;
      scripts.end(r);
    },
  });
  return { scripts, rec, log };
}

test("the address of a preview file keeps the folders and encodes each name", () => {
  // [path of the file, address under the token "t0"]
  const rows = [
    ["page.html", "/preview/t0/page.html"],
    ["docs/my page.html", "/preview/t0/docs/my%20page.html"],
    ["a#b/c?d.html", "/preview/t0/a%23b/c%3Fd.html"],
  ];
  for (const [path, want] of rows) assert.equal(previewUrl("t0", path), want, path);
});

test("Run scripts asks the daemon for a token and shows the file from its route", async () => {
  const reply = { ...fixture("file.preview"), token: "t0" };
  assert.deepEqual(Object.keys(reply).toSorted(), ["status", "token"]);
  const { scripts, rec, log } = pane(reply, { checkout: "wt-a" });
  scripts.toggle(rec);
  await settle();
  assert.deepEqual(log.asked, [["file.preview", { repo: "o/r", checkout: "wt-a" }]]);
  assert.equal(rec.scripts, "t0");
  assert.equal(log.frames.length, 1);
  assert.deepEqual(log.frames[0].held, { sandbox: "allow-scripts" });
  assert.equal(log.frames[0].src, "/preview/t0/docs/my%20page.html");
  assert.deepEqual(log.captions.at(-1), ["bi-stop-circle", "Stop scripts"]);
  assert.deepEqual(log.flashed, []);
});

test("a second click ends the token and draws the file with no script", async () => {
  const { scripts, rec, log } = pane({ status: "ok", token: "t0" });
  scripts.toggle(rec);
  await settle();
  scripts.toggle(rec);
  await settle();
  assert.equal(rec.scripts, undefined);
  assert.equal(log.drawn, 1);
  assert.deepEqual(log.asked.at(-1), ["file.preview", { repo: "o/r", revoke: "t0" }]);
  assert.deepEqual(log.captions.at(-1), ["bi-play-circle", "Run scripts"]);
  // Ended once: a second end asks nothing more.
  const asked = log.asked.length;
  scripts.end(rec);
  assert.equal(log.asked.length, asked);
});

test("no script runs for unsaved text, a refusal, or a pane that went away", async () => {
  // [the reply of the daemon, the pane, what the operator is told, what the daemon is asked]
  const text = { cause: "the project is on another computer", remedy: "Open that workbench." };
  const peer = { status: "error", failure: { domain: "daemon", code: "scripts-on-a-peer", text } };
  const rows = [
    [{ status: "ok", token: "t0" }, { dirty: true }, ["Save the file before you run its scripts."], []],
    [
      peer,
      {},
      ["Could not run the scripts: the project is on another computer. Open that workbench."],
      [{ repo: "o/r" }],
    ],
    [{ status: "ok" }, {}, ["Could not run the scripts: the daemon gave no reason."], [{ repo: "o/r" }]],
    [{ status: "ok", token: "t0" }, { closed: true }, [], [{ repo: "o/r" }, { repo: "o/r", revoke: "t0" }]],
    [{ status: "ok", token: "t0" }, { editing: true }, [], [{ repo: "o/r" }, { repo: "o/r", revoke: "t0" }]],
  ];
  for (const [reply, over, flashed, asked] of rows) {
    const { scripts, rec, log } = pane(reply, over);
    scripts.toggle(rec);
    await settle();
    const label = JSON.stringify([reply, over]);
    assert.equal(rec.scripts, undefined, label);
    assert.deepEqual(log.frames, [], label);
    assert.deepEqual(log.flashed, flashed, label);
    assert.deepEqual(log.asked.map(([, payload]) => payload), asked, label);
  }
});
