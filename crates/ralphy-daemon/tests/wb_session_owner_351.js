const assert = require("node:assert/strict");
const test = require("node:test");
const route = require("../assets/ui/wb-session-route.js");

const PEER_REPO = "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/shared";

test("every id reconnect and close retains the composite repo", () => {
  const plain = route.url("ws://local", { id: 7, repo: PEER_REPO });
  const watch = route.url("ws://local", { id: 7, repo: PEER_REPO, watch: true });
  const takeover = route.url("ws://local", {
    id: 7,
    repo: PEER_REPO,
    takeover: true,
  });
  for (const value of [plain, watch, takeover]) {
    assert.match(value, /repo=01ARZ3NDEKTSV4RRFFQ69G5FAW%2Fowner%2Fshared/);
  }
  assert.equal(
    route.closeUrl(7, PEER_REPO),
    "/api/sessions/close?id=7&repo=01ARZ3NDEKTSV4RRFFQ69G5FAW%2Fowner%2Fshared",
  );
});

test("session-open supplies id and owner before terminal output", () => {
  assert.deepEqual(
    route.announcement(
      { sessionId: null, daemonId: null, environment: null },
      {
        session: 9,
        daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
        environment: "WSL: Ubuntu-22.04",
      },
    ),
    {
      sessionId: 9,
      daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
      environment: "WSL: Ubuntu-22.04",
      name: null,
    },
  );
});

test("the vendor session name arrives with the announcement and survives a re-announcement", () => {
  const opened = route.announcement(
    { sessionId: null, daemonId: null, environment: null },
    { session: 4, daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW", name: "wb-ralphy-7f3a" },
  );
  assert.equal(opened.name, "wb-ralphy-7f3a");

  // A later frame that omits the name must not blank it: the console still
  // answers to it, and a titlebar that dropped it would send the operator
  // hunting through `claude agents` for an address they already had.
  assert.equal(route.announcement(opened, { session: 4 }).name, "wb-ralphy-7f3a");

  // A vendor with no `--name` (and the free console) announces none.
  assert.equal(
    route.announcement({ sessionId: null, daemonId: null, environment: null }, { session: 5 })
      .name,
    null,
  );
});

test("a local slug never marks the peer composite repo live", () => {
  assert.equal(route.matchesRepo({ repo: "owner/shared" }, PEER_REPO), false);
  assert.equal(route.matchesRepo({ repo: PEER_REPO }, PEER_REPO), true);
});

test("a failed peer close keeps the window available for retry", () => {
  assert.equal(route.closeSucceeded(200), true);
  assert.equal(route.closeSucceeded(404), true);
  assert.equal(route.closeSucceeded(502), false);
});
