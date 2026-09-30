// Unit tests for assets/ui/wb-mode.js — the "seed or honest error?" predicate.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-mode.js"), "utf8");

function load(protocol) {
  const window = {};
  new Function("window", "location", SRC)(window, { protocol });
  return window.WBMode;
}

test("only a file: page is the demo, and only the demo may show seed data", () => {
  for (const [protocol, mode] of [
    ["file:", "demo"],
    ["http:", "daemon"],
    ["https:", "daemon"],
  ]) {
    const m = load(protocol);
    const demo = mode === "demo";
    assert.equal(m.modeFor(protocol), mode, `${protocol} mode`);
    // The defaults read the page's own location.
    assert.equal(m.isDemo(), demo, `${protocol} isDemo`);
    assert.equal(m.isDaemon(), !demo, `${protocol} isDaemon`);
    assert.equal(m.seedAllowed(), demo, `${protocol} seedAllowed`);
  }
});

test("a torn-off window opens at its route, or at its file in the demo", () => {
  // The daemon refuses the file names, and a file: page has no router.
  assert.equal(load("http:").pageUrl("popup"), "popup");
  assert.equal(load("https:").pageUrl("fence"), "fence");
  assert.equal(load("file:").pageUrl("popup"), "detached.html");
  assert.equal(load("file:").pageUrl("fence"), "detached-fence.html");
});
