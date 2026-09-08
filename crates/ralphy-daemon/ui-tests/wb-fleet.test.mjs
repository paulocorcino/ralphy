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

test("only the states a nudge can answer offer to wake", () => {
  const { wakeable } = load();
  const group = (state, extra) =>
    Object.assign({ daemon: "01XYZ", local: false, nudgeable: true, state: state }, extra);
  for (const state of ["asleep", "unreachable"]) {
    assert.equal(wakeable(group(state)), true, `${state} must be wakeable`);
  }
  // A nudge cannot rotate a token, upgrade a daemon, fix a descriptor, or make a
  // non-descriptor into a peer. Offering to wake these promises a fix that can
  // never arrive, and the operator would keep clicking it.
  for (const state of ["reachable", "unauthorized", "version-mismatch", "refused", "malformed"]) {
    assert.equal(wakeable(group(state)), false, `${state} must not offer a wake`);
  }
  assert.equal(
    wakeable(group("asleep", { nudgeable: false })),
    false,
    "a peer that announced no distro has nothing to wake it with",
  );
  assert.equal(wakeable(group("asleep", { local: true })), false, "this daemon is not its own peer");
  assert.equal(wakeable(null), false);
});

test("a peer ref names the daemon to wake, a local one names none", () => {
  const { refDaemon } = load();
  assert.equal(refDaemon("01ARZ3NDEKTSV4RRFFQ69G5FAV/owner/repo"), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
  assert.equal(refDaemon("owner/repo"), "", "a local slug has no daemon at its head");
  assert.equal(refDaemon(null), "");
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

// ---- the ref → slug fold (ADR-0052 §5) --------------------------------------
// A peer ref carries a `<daemon_id>/` routing head. `refSlug` strips it and
// `refLabel` re-attaches the environment in its place. Both were UNTESTED until
// a review found the only occurrence of `refSlug` in this suite was a
// hand-written stub in wb-console.test.mjs — so a broken fold passed everywhere.

test("refSlug strips a ULID routing head and nothing else", () => {
  const wb = load();
  assert.equal(wb.refSlug("01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/repo"), "owner/repo");
  // A plain local ref has no head to strip.
  assert.equal(wb.refSlug("owner/repo"), "owner/repo");
  // NEGATIVE CONTROL, and the reason this is a ULID test and not a "first
  // segment" test: an owner genuinely named like a path segment must survive.
  // A stub that took `slice(-2)` would pass the peer case above and silently
  // eat the owner here.
  assert.equal(wb.refSlug("acme/team/repo"), "acme/team/repo");
  assert.equal(wb.refSlug("owner/path-9f2a1c"), "owner/path-9f2a1c");
  // Crockford base32: no I, L, O or U. A 26-char head that is not a ULID is not
  // a head.
  assert.equal(wb.refSlug("ILOUILOUILOUILOUILOUILOUIL/owner/repo"), "ILOUILOUILOUILOUILOUILOUIL/owner/repo");
  // Wrong length is not a ULID either.
  assert.equal(wb.refSlug("01ARZ3NDEKTSV4RRFFQ69G5FA/owner/repo"), "01ARZ3NDEKTSV4RRFFQ69G5FA/owner/repo");
  // Absent input yields the empty string, never "null" or "undefined".
  assert.equal(wb.refSlug(null), "");
  assert.equal(wb.refSlug(undefined), "");
  assert.equal(wb.refSlug(""), "");
});

test("refLabel appends the environment only for a peer", () => {
  const wb = load();
  const peer = "01ARZ3NDEKTSV4RRFFQ69G5FAZ/owner/repo";
  assert.equal(wb.refLabel(peer, "WSL: Ubuntu-22.04"), "owner/repo · WSL: Ubuntu-22.04");
  // A local ref takes no environment segment even when one is offered — the
  // environment is what the stripped ULID was there to say, and a local repo
  // never had one.
  assert.equal(wb.refLabel("owner/repo", "WSL: Ubuntu-22.04"), "owner/repo");
  // A peer whose environment has not been announced yet shows the slug alone,
  // not a trailing separator.
  assert.equal(wb.refLabel(peer, null), "owner/repo");
  assert.equal(wb.refLabel(peer, ""), "owner/repo");
});
