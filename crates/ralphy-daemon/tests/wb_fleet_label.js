// The federated ref, said to a human (ADR-0052 §5).
//
// A peer ref is `<daemon_id>/<owner>/<repo>`. The head is ROUTING — it is what
// makes the same `owner/repo` on two daemons two rows — and it is not a name.
// The rule these tests pin: the head never reaches an operator-facing string,
// and stripping it never touches a local slug that merely looks composite.
const assert = require("node:assert/strict");
const test = require("node:test");
const fleet = require("../assets/ui/wb-fleet.js");

const PEER_ID = "01ARZ3NDEKTSV4RRFFQ69G5FAW";
const PEER_REF = `${PEER_ID}/paulocorcino/vibeforge`;
const PEER_ENV = "WSL: Ubuntu-22.04";

test("a peer ref loses its routing head and nothing else", () => {
  assert.equal(fleet.refSlug(PEER_REF), "paulocorcino/vibeforge");
  assert.equal(fleet.refDaemon(PEER_REF), PEER_ID);
});

test("a local ref is returned untouched, whatever its shape", () => {
  // `owner/repo`, the remoteless `path-<hash>` fallback (ADR-0008 D7), and the
  // empty/absent ref every "no project open" surface passes in.
  assert.equal(fleet.refSlug("paulocorcino/vibeforge"), "paulocorcino/vibeforge");
  assert.equal(fleet.refSlug("path-9f2c1ab4"), "path-9f2c1ab4");
  assert.equal(fleet.refSlug(""), "");
  assert.equal(fleet.refSlug(null), "");
});

test("an owner that merely looks composite is not mistaken for a daemon", () => {
  // Only a full 26-char Crockford ULID at the head is a routing head. A repo
  // owner is a repo owner — re-labelling one would delete a path segment the
  // wire needs.
  assert.equal(fleet.refSlug("01ARZ3NDEK/owner/repo"), "01ARZ3NDEK/owner/repo");
  assert.equal(fleet.refSlug("owner/01ARZ3NDEKTSV4RRFFQ69G5FAW"), "owner/01ARZ3NDEKTSV4RRFFQ69G5FAW");
});

test("the environment replaces the head only where there was one", () => {
  assert.equal(fleet.refLabel(PEER_REF, PEER_ENV), `paulocorcino/vibeforge · ${PEER_ENV}`);
  // A local ref never gets a suffix: its environment is the machine the
  // operator is sitting at, and naming it would label every row on a fleet of
  // one.
  assert.equal(fleet.refLabel("paulocorcino/vibeforge", PEER_ENV), "paulocorcino/vibeforge");
  // A peer whose environment has not arrived yet (a console between spawn and
  // `session-open`) still loses the head — it degrades to the slug, never back
  // to the ULID.
  assert.equal(fleet.refLabel(PEER_REF, ""), "paulocorcino/vibeforge");
  assert.equal(fleet.refLabel(PEER_REF, undefined), "paulocorcino/vibeforge");
});

test("the grouping fold still works through the module wrapper", () => {
  // `wb-fleet.js` gained a UMD wrapper so this file can require it; the #349
  // fold it already carried must survive that.
  const groups = fleet.fleetGroups(
    [
      { slug: "paulocorcino/vibeforge", env: "Windows", daemonName: "anvil" },
      { key: PEER_REF, slug: "paulocorcino/vibeforge", daemon: PEER_ID, env: PEER_ENV },
    ],
    [{ daemon_id: PEER_ID, environment: PEER_ENV, name: "wsl-box", state: "reachable" }],
  );
  assert.equal(groups.length, 2);
  assert.equal(groups[0].local, true);
  assert.equal(groups[1].daemon, PEER_ID);
});
