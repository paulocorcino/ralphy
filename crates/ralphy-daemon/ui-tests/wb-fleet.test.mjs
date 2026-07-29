// Unit tests for assets/ui/wb-fleet.js — runs the real source with no DOM.
// Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into the
// daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const SRC = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "../assets/ui/wb-fleet.js"),
  "utf8",
);

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBFleet;
}

const LOCAL_ROW = { key: "01ABC/owner/repo", slug: "owner/repo", path: "C:/Dev/repo" };
const PEER_ROW = {
  key: "01XYZ/owner/repo",
  slug: "owner/repo",
  path: "/home/p/repo",
  daemon: "01XYZ",
  env: "WSL: Ubuntu-22.04",
  peerState: "reachable",
};
const PEER = {
  daemon_id: "01XYZ",
  name: "wsl-box",
  avatar: "🐺",
  environment: "WSL: Ubuntu-22.04",
  state: "reachable",
  diagnosis: "peer WSL: Ubuntu-22.04 answered the handshake",
  nudgeable: true,
};

test("the same slug on two daemons is two rows in two groups", () => {
  const groups = load().fleetGroups([LOCAL_ROW, PEER_ROW], [PEER]);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].local, true, "local sorts first");
  assert.equal(groups[0].rows.length, 1);
  assert.equal(groups[1].daemon, "01XYZ");
  assert.equal(groups[1].rows.length, 1);
  // Neither row was overwritten by the other: both paths survive.
  assert.equal(groups[0].rows[0].path, "C:/Dev/repo");
  assert.equal(groups[1].rows[0].path, "/home/p/repo");
  assert.equal(groups[1].environment, "WSL: Ubuntu-22.04");
  assert.equal(groups[1].name, "wsl-box");
});

test("an unreachable peer's group still carries its rows", () => {
  const down = Object.assign({}, PEER, {
    state: "unreachable",
    diagnosis: "peer WSL: Ubuntu-22.04 did not answer (connection refused)",
  });
  const downRow = Object.assign({}, PEER_ROW, { peerState: "unreachable" });
  const groups = load().fleetGroups([LOCAL_ROW, downRow], [down]);
  const peerGroup = groups.find((g) => g.daemon === "01XYZ");
  assert.ok(peerGroup, "the peer must still be a group");
  assert.equal(peerGroup.state, "unreachable");
  assert.equal(peerGroup.rows.length, 1, "its repos stay listed");
  assert.match(peerGroup.diagnosis, /WSL: Ubuntu-22\.04/);
});

test("a peer that contributed no rows is still a group", () => {
  const groups = load().fleetGroups([LOCAL_ROW], [Object.assign({}, PEER, { state: "unauthorized" })]);
  assert.equal(groups.length, 2);
  const peerGroup = groups.find((g) => g.daemon === "01XYZ");
  assert.equal(peerGroup.rows.length, 0);
  assert.equal(peerGroup.state, "unauthorized");
});

test("a fleet of one shows no environment headers", () => {
  const groups = load().fleetGroups([LOCAL_ROW], []);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].header, false, "one environment needs no header naming it");
  assert.equal(groups[0].state, "local");
});

test("two environments both get headers", () => {
  const groups = load().fleetGroups([LOCAL_ROW, PEER_ROW], [PEER]);
  assert.deepEqual(
    groups.map((g) => g.header),
    [true, true],
  );
});

test("the order peers arrive in does not change the grouping", () => {
  const other = { daemon_id: "01AAA", name: "debian-box", environment: "WSL: Debian", state: "reachable" };
  const otherRow = {
    key: "01AAA/owner/x",
    slug: "owner/x",
    daemon: "01AAA",
    env: "WSL: Debian",
    peerState: "reachable",
  };
  const forward = load().fleetGroups([LOCAL_ROW, PEER_ROW, otherRow], [PEER, other]);
  const reversed = load().fleetGroups([otherRow, PEER_ROW, LOCAL_ROW], [other, PEER]);
  assert.deepEqual(
    forward.map((g) => g.key),
    reversed.map((g) => g.key),
  );
});

test("a missing repos or peers list is not a crash", () => {
  const fleet = load();
  assert.deepEqual(fleet.fleetGroups(undefined, undefined), []);
  assert.equal(fleet.fleetGroups(null, [PEER]).length, 1);
});

test("repo refs keep local slugs and use peer keys", () => {
  const fleet = load();
  assert.equal(fleet.repoRef({ slug: "owner/repo" }), "owner/repo");
  assert.equal(
    fleet.repoRef({
      key: "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo",
      slug: "owner/repo",
    }),
    "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo",
  );
});

test("peer refs require a Crockford ULID head", () => {
  const fleet = load();
  assert.equal(fleet.isPeerRef("owner/repo"), false);
  assert.equal(fleet.isPeerRef("01ARZ3NDEKTSV4RRFFQ69G5FAW"), false);
  assert.equal(fleet.isPeerRef("01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/repo"), true);
  assert.equal(fleet.isPeerRef("01PEERA/repo"), false);
});
