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

// The startup command rides ONLY the free-console launch, encoded so a space
// or an `&` in `btop --utf-force` survives the query string. A reattach never
// carries it: the daemon's record owns what the session runs.
test("the console launch carries its startup command, encoded; a reattach never does", () => {
  const { url } = load();
  assert.equal(
    url("ws://h", { console: true, repo: "o/r", command: "htop -d 5" }),
    "ws://h/ws/session?console=1&repo=o%2Fr&command=htop%20-d%205",
  );
  assert.equal(
    url("ws://h", { console: true, command: "btop" }),
    "ws://h/ws/session?console=1&command=btop",
    "a repo-less console still takes the command",
  );
  assert.ok(!url("ws://h", { console: true, repo: "o/r" }).includes("command"));
  assert.ok(!url("ws://h", { id: 4, repo: "o/r", command: "htop" }).includes("command"));
});

// ADR-0051 §9 amendment 2026-09-22: every claim of the writer slot names the
// tab's holder, so this tab's reattach can reclaim a slot its own half-open
// socket still holds. A watcher claims nothing and names nothing, and a
// malformed holder is dropped rather than spliced into the query.
test("url names the holder on every claim of the writer slot, never on a watch", () => {
  const { url } = load();
  const h = "0f3a-tab_B";
  assert.equal(
    url("ws://h", { id: 3, repo: "o/r", holder: h }),
    "ws://h/ws/session?id=3&repo=o%2Fr&holder=0f3a-tab_B",
  );
  assert.equal(
    url("ws://h", { id: 3, takeover: true, holder: h }),
    "ws://h/ws/session?id=3&takeover=1&holder=0f3a-tab_B",
  );
  assert.equal(
    url("ws://h", { repo: "o/r", agent: "claude", checkout: "wt-a", holder: h }),
    "ws://h/ws/session?repo=o%2Fr&agent=claude&checkout=wt-a&holder=0f3a-tab_B",
  );
  assert.equal(
    url("ws://h", { console: true, command: "btop", holder: h }),
    "ws://h/ws/session?console=1&command=btop&holder=0f3a-tab_B",
  );
  assert.equal(url("ws://h", { id: 3, watch: true, holder: h }), "ws://h/ws/session?id=3&watch=1");
  for (const bad of ["", "a&takeover=1", "x".repeat(65), 7, null, undefined]) {
    assert.equal(url("ws://h", { id: 3, holder: bad }), "ws://h/ws/session?id=3", String(bad));
  }
});

function memoryStorage(seed) {
  const map = new Map(Object.entries(seed || {}));
  return {
    getItem: (k) => (map.has(k) ? map.get(k) : null),
    setItem: (k, v) => map.set(k, String(v)),
    map,
  };
}

test("holder keeps the tab's name across a reload and mints only when there is none", () => {
  const { holder } = load();
  let minted = 0;
  const mint = () => `m${++minted}`;
  const storage = memoryStorage();
  assert.equal(holder(storage, mint), "m1");
  // A reload reads the same sessionStorage: same holder, nothing minted.
  assert.equal(holder(storage, mint), "m1");
  assert.equal(minted, 1);
  // A stored value that is not a holder is replaced, not trusted.
  const tampered = memoryStorage({ "ralphy.holder": "a&takeover=1" });
  assert.equal(holder(tampered, mint), "m2");
  assert.equal(tampered.map.get("ralphy.holder"), "m2");
});

test("holder still names the tab when storage is absent or throws", () => {
  const { holder } = load();
  assert.equal(holder(null, () => "fresh"), "fresh");
  const throwing = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.equal(holder(throwing, () => "fresh"), "fresh");
});

test("tabHolder mints one well-formed holder per document and keeps it", () => {
  const { tabHolder } = load();
  const first = tabHolder();
  assert.match(first, /^[0-9a-f]{32}$/);
  assert.equal(tabHolder(), first, "every console of the tab claims as the same holder");
});
