// Unit tests for assets/ui/wb-viewer.js — the link decision table behind a
// click in a rendered markdown document. Runs the real source against a DOM
// that answers nothing: the module only touches `document` at load to find its
// mount point, and `linkTarget` is pure.
import { test } from "node:test";
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
