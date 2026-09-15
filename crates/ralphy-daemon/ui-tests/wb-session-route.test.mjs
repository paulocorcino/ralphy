// Unit tests for assets/ui/wb-session-route.js — runs the real source with no
// DOM. Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into
// the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-session-route.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBSessionRoute;
}

const OPEN = { sessionId: 1, daemonId: "d", environment: "Windows" };

test("url appends the checkout only on a new agent launch that names one", () => {
  const { url } = load();
  assert.equal(
    url("ws://h", { repo: "o/r", agent: "claude", checkout: "wt-a" }),
    "ws://h/ws/session?repo=o%2Fr&agent=claude&checkout=wt-a",
  );
  // No checkout — byte-identical to the launch URL of today.
  const bare = "ws://h/ws/session?repo=o%2Fr&agent=claude";
  assert.equal(url("ws://h", { repo: "o/r", agent: "claude" }), bare);
  assert.equal(url("ws://h", { repo: "o/r", agent: "claude", checkout: null }), bare);
  assert.equal(url("ws://h", { repo: "o/r", agent: "claude", checkout: "" }), bare);
  // A reattach names a record the daemon already owns — never a checkout.
  const reattach = url("ws://h", { id: 3, repo: "o/r", checkout: "wt-a" });
  assert.equal(reattach, "ws://h/ws/session?id=3&repo=o%2Fr");
  assert.ok(!reattach.includes("checkout"));
  // The free console never takes one either.
  assert.ok(!url("ws://h", { console: true, repo: "o/r", checkout: "wt-a" }).includes("checkout"));
});

test("announcement folds the checkout and keeps the prior when a payload omits it", () => {
  const { announcement } = load();
  assert.equal(
    announcement({ ...OPEN, name: null, checkout: "wt-a" }, { name: "x" }).checkout,
    "wt-a",
  );
  assert.equal(announcement(OPEN, { checkout: "wt-a" }).checkout, "wt-a");
  assert.equal(announcement(OPEN, {}).checkout, null);
  assert.equal(announcement(OPEN, null).checkout, null);
  // An explicit null on a fresh announcement keeps nothing to fall back to.
  assert.equal(announcement(OPEN, { checkout: null }).checkout, null);
});
