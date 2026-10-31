// Unit tests for assets/ui/wb-session-route.ts — the module has no
// DOM. Lives OUTSIDE assets/ui on purpose: lib.rs embeds all of assets/ui into
// the daemon binary via include_dir!, so a test there would ship.
import { test } from "node:test";
import assert from "node:assert/strict";
import { WBSessionRoute } from "../assets/ui/wb-session-route.ts";

function load() {
  return WBSessionRoute;
}

const OPEN = { sessionId: 1, daemonId: "d", environment: "Windows" };

test("the open payload carries the checkout only on a new agent launch that names one", () => {
  const { openPayload } = load();
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", checkout: "wt-a" }), {
    repo: "o/r",
    agent: "claude",
    checkout: "wt-a",
  });
  // No checkout: no key, as a launch without one sends today.
  const bare = { repo: "o/r", agent: "claude" };
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude" }), bare);
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", checkout: null }), bare);
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", checkout: "" }), bare);
  // A reattach names a record the daemon already owns — never a checkout.
  assert.deepEqual(openPayload({ id: 3, repo: "o/r", checkout: "wt-a" }), { id: 3, repo: "o/r" });
  // The free console never takes one either.
  assert.ok(!("checkout" in openPayload({ console: true, repo: "o/r", checkout: "wt-a" })));
});

test("requestOpts builds each kind of connect, and never sends null as a value", () => {
  const { openPayload, requestOpts } = load();
  const at = (req) => openPayload(requestOpts(req, "h1"));
  const rows = [
    [
      { repo: "o/r", agent: "claude", checkout: "wt-a", record: "r1" },
      { repo: "o/r", agent: "claude", checkout: "wt-a", record: "r1", holder: "h1" },
    ],
    [{ repo: null, agent: "claude" }, { repo: "", agent: "claude", holder: "h1" }],
    [{ id: 3, repo: null, watch: true }, { id: 3, watch: 1 }],
    [{ id: 3, repo: "o/r", takeover: true }, { id: 3, repo: "o/r", takeover: 1, holder: "h1" }],
    [{ console: true, repo: null, command: "ls" }, { console: 1, command: "ls", holder: "h1" }],
  ];
  for (const [req, payload] of rows) assert.deepEqual(at(req), payload, JSON.stringify(req));
});

// The daemon's `peer_session_query_forwards_the_link_the_browser_reported`
// test reads these same keys. The daemon reads them as strings.
test("the open payload carries the network the browser reports, and nothing when it reports none", () => {
  const { openPayload } = load();
  const link = { effectiveType: "4g", rtt: 100, downlink: 1.75 };
  const net = { net: "4g", net_rtt: "100", net_down: "1.75" };
  assert.deepEqual(openPayload({ id: 3, link }), { id: 3, ...net });
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", holder: "h1", link }), {
    repo: "o/r",
    agent: "claude",
    holder: "h1",
    ...net,
  });
  assert.deepEqual(openPayload({ console: true, link: { rtt: 0 } }), { console: 1, net_rtt: "0" });
  // Safari, and values the daemon would not read.
  assert.deepEqual(openPayload({ id: 3 }), { id: 3 });
  assert.deepEqual(openPayload({ id: 3, link: {} }), { id: 3 });
  assert.deepEqual(
    openPayload({ id: 3, link: { effectiveType: "wifi", rtt: 1.5, downlink: Number.NaN } }),
    { id: 3 },
  );
});

test("the open payload carries the console name only on a new agent launch", () => {
  const { openPayload } = load();
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", name: "fincal #1" }), {
    repo: "o/r",
    agent: "claude",
    name: "fincal #1",
  });
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", checkout: "wt-a", name: "fincal #1" }), {
    repo: "o/r",
    agent: "claude",
    checkout: "wt-a",
    name: "fincal #1",
  });
  const bare = { repo: "o/r", agent: "claude" };
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude" }), bare);
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", name: "" }), bare);
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", name: null }), bare);
  // A reattach names a session that already has its name; the free console
  // has no vendor to hand one to.
  assert.ok(!("name" in openPayload({ id: 3, repo: "o/r", name: "x" })));
  assert.ok(!("name" in openPayload({ console: true, repo: "o/r", name: "x" })));
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

// The startup command rides ONLY the free-console launch, whole: `btop
// --utf-force` keeps its space. A reattach never carries it: the daemon's
// record owns what the session runs.
test("the console launch carries its startup command; a reattach never does", () => {
  const { openPayload } = load();
  assert.deepEqual(openPayload({ console: true, repo: "o/r", command: "htop -d 5" }), {
    console: 1,
    repo: "o/r",
    command: "htop -d 5",
  });
  assert.deepEqual(
    openPayload({ console: true, command: "btop" }),
    { console: 1, command: "btop" },
    "a repo-less console still takes the command",
  );
  assert.ok(!("command" in openPayload({ console: true, repo: "o/r" })));
  assert.ok(!("command" in openPayload({ id: 4, repo: "o/r", command: "htop" })));
});

// ADR-0051 §9 amendment 2026-09-22: every claim of the writer slot names the
// tab's holder, so this tab's reattach can reclaim a slot its own half-open
// socket still holds. A watcher claims nothing and names nothing, and a
// malformed holder is left out.
test("the open payload names the holder on every claim of the writer slot, never on a watch", () => {
  const { openPayload } = load();
  const h = "0f3a-tab_B";
  assert.deepEqual(openPayload({ id: 3, repo: "o/r", holder: h }), { id: 3, repo: "o/r", holder: h });
  assert.deepEqual(openPayload({ id: 3, takeover: true, holder: h }), { id: 3, takeover: 1, holder: h });
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", checkout: "wt-a", holder: h }), {
    repo: "o/r",
    agent: "claude",
    checkout: "wt-a",
    holder: h,
  });
  assert.deepEqual(openPayload({ console: true, command: "btop", holder: h }), {
    console: 1,
    command: "btop",
    holder: h,
  });
  assert.deepEqual(openPayload({ id: 3, watch: true, holder: h }), { id: 3, watch: 1 });
  for (const bad of ["", "a&takeover=1", "x".repeat(65), 7, null, undefined]) {
    assert.deepEqual(openPayload({ id: 3, holder: bad }), { id: 3 }, String(bad));
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

// ADR-0079 D7: the attempt orders the claims of one tab, so it only grows,
// across a reload too, and rides in the open payload beside the holder.
test("nextAttempt only grows and survives a reload in sessionStorage", () => {
  const { nextAttempt } = load();
  const storage = memoryStorage({ "ralphy.attempt": "41" });
  const first = nextAttempt(storage);
  assert.ok(first >= 42, `kept 41, got ${first}`);
  assert.equal(storage.map.get("ralphy.attempt"), String(first));
  // A reload reads the same storage; the module memo of the old document is gone.
  const second = nextAttempt(memoryStorage({ "ralphy.attempt": storage.map.get("ralphy.attempt") }));
  assert.ok(second > first, `${second} after ${first}`);
  // A storage that was cleared, holds junk, or throws never makes it go down.
  const junk = memoryStorage({ "ralphy.attempt": "x&takeover=1" });
  assert.ok(nextAttempt(junk) > second);
  assert.ok(nextAttempt(memoryStorage()) > second + 1);
  const throwing = {
    getItem() {
      throw new Error("denied");
    },
    setItem() {
      throw new Error("denied");
    },
  };
  assert.ok(nextAttempt(throwing) > second + 2);
});

// A duplicated tab or a fence popup copies sessionStorage, so two windows
// share a holder and each has its own module memo. The clock keeps their
// attempts in the order of their connects.
test("two windows that copied one storage order their attempts by the clock", async () => {
  const { WBSessionRoute: popup } = await import("../assets/ui/wb-session-route.ts?window=popup");
  const { nextAttempt } = load();
  const t = Date.now();
  const copied = { "ralphy.attempt": String(nextAttempt(memoryStorage(), t)) };
  const opener = memoryStorage(copied);
  const copy = memoryStorage(copied);
  const first = popup.nextAttempt(copy, t + 10);
  const second = nextAttempt(opener, t + 20);
  assert.ok(second > first, `the opener's later connect ${second} after the popup's ${first}`);
  assert.ok(popup.nextAttempt(copy, t + 30) > second, "and the popup's after that");
  assert.ok(Number.isSafeInteger(nextAttempt(opener)));
});

test("the attempt is in the open payload only beside a holder", () => {
  const { openPayload, requestOpts } = load();
  const at = (req, holder) => openPayload(requestOpts(req, holder, false, 9));
  assert.deepEqual(at({ id: 3, repo: "o/r" }, "h1"), { id: 3, repo: "o/r", holder: "h1", attempt: 9 });
  assert.deepEqual(at({ console: true }, "h1"), { console: 1, holder: "h1", attempt: 9 });
  assert.deepEqual(at({ repo: "o/r", agent: "claude" }, "h1"), {
    repo: "o/r",
    agent: "claude",
    holder: "h1",
    attempt: 9,
  });
  // A watcher claims nothing.
  assert.deepEqual(at({ id: 3, watch: true }, "h1"), { id: 3, watch: 1 });
  assert.deepEqual(at({ id: 3 }, "a&b"), { id: 3 }, "no holder, no attempt");
  assert.deepEqual(openPayload({ id: 3, holder: "h1", attempt: -1 }), { id: 3, holder: "h1" });
});

// ADR-0079 D8: the byte count the terminal shows rides only on a reattach.
test("since is in the open payload of a reattach only, a watch's included", () => {
  const { openPayload, requestOpts } = load();
  const at = (req) => openPayload(requestOpts(req, "h1", false, undefined, 4096));
  assert.deepEqual(at({ id: 3, repo: "o/r" }), { id: 3, repo: "o/r", since: 4096, holder: "h1" });
  assert.deepEqual(at({ id: 3, watch: true }), { id: 3, since: 4096, watch: 1 });
  assert.deepEqual(at({ console: true }), { console: 1, holder: "h1" }, "a launch opens a new terminal");
  assert.deepEqual(at({ repo: "o/r", agent: "claude" }), { repo: "o/r", agent: "claude", holder: "h1" });
  assert.deepEqual(openPayload({ id: 3, since: 0 }), { id: 3, since: 0 });
  assert.deepEqual(openPayload({ id: 3, since: -1 }), { id: 3 });
  assert.deepEqual(openPayload({ id: 3, since: 1.5 }), { id: 3 });
});

test("tabAttempt raises the tab's counter on every call", () => {
  const { tabAttempt } = load();
  const first = tabAttempt();
  assert.ok(Number.isSafeInteger(first) && first > 0);
  assert.ok(tabAttempt() > first);
});

test("tabHolder mints one well-formed holder per document and keeps it", () => {
  const { tabHolder } = load();
  const first = tabHolder();
  assert.match(first, /^[0-9a-f]{32}$/);
  assert.equal(tabHolder(), first, "every console of the tab claims as the same holder");
});

// A peer session is owned by the composite ref `<daemon_id>/<owner>/<repo>`:
// every reconnect and close path must carry it whole, and only an exact match
// may mark the peer's row live.
const PEER_REPO = "01ARZ3NDEKTSV4RRFFQ69G5FAW/owner/shared";

test("every id reconnect and close retains the composite repo", () => {
  const { openPayload, closeUrl } = load();
  const plain = openPayload({ id: 7, repo: PEER_REPO });
  const watch = openPayload({ id: 7, repo: PEER_REPO, watch: true });
  const takeover = openPayload({ id: 7, repo: PEER_REPO, takeover: true });
  for (const payload of [plain, watch, takeover]) assert.equal(payload.repo, PEER_REPO);
  assert.equal(
    closeUrl(7, PEER_REPO),
    "/api/sessions/close?id=7&repo=01ARZ3NDEKTSV4RRFFQ69G5FAW%2Fowner%2Fshared",
  );
});

test("session-open supplies id and owner before terminal output", () => {
  const { announcement } = load();
  assert.deepEqual(
    announcement(
      { sessionId: null, daemonId: null, environment: null },
      { session: 9, daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW", environment: "WSL: Ubuntu-22.04" },
    ),
    {
      sessionId: 9,
      daemonId: "01ARZ3NDEKTSV4RRFFQ69G5FAW",
      environment: "WSL: Ubuntu-22.04",
      name: null,
      checkout: null,
    },
  );
});

test("the vendor session name arrives with the announcement and survives a re-announcement", () => {
  const { announcement } = load();
  const opened = announcement(
    { sessionId: null, daemonId: null, environment: null },
    { session: 4, daemon_id: "01ARZ3NDEKTSV4RRFFQ69G5FAW", name: "wb-ralphy-7f3a" },
  );
  assert.equal(opened.name, "wb-ralphy-7f3a");
  // A later frame that omits the name must not blank it: the console still
  // answers to it.
  assert.equal(announcement(opened, { session: 4 }).name, "wb-ralphy-7f3a");
  // A vendor with no `--name` (and the free console) announces none.
  assert.equal(
    announcement({ sessionId: null, daemonId: null, environment: null }, { session: 5 }).name,
    null,
  );
});

test("a local slug never marks the peer composite repo live", () => {
  const { matchesRepo } = load();
  assert.equal(matchesRepo({ repo: "owner/shared" }, PEER_REPO), false);
  assert.equal(matchesRepo({ repo: PEER_REPO }, PEER_REPO), true);
});

test("a failed peer close keeps the window available for retry", () => {
  const { closeSucceeded } = load();
  assert.equal(closeSucceeded(200), true);
  assert.equal(closeSucceeded(404), true);
  assert.equal(closeSucceeded(502), false);
});

test("the open payload names the window record on a new launch only", () => {
  const { openPayload } = load();
  // The daemon keeps one session per record, so both launch shapes carry it.
  assert.deepEqual(openPayload({ console: true, repo: "o/r", record: "w-a_1" }), {
    console: 1,
    repo: "o/r",
    record: "w-a_1",
  });
  assert.deepEqual(openPayload({ repo: "o/r", agent: "claude", record: "w-a_1" }), {
    repo: "o/r",
    agent: "claude",
    record: "w-a_1",
  });
  // A reattach names its session already.
  assert.deepEqual(openPayload({ id: 3, repo: "o/r", record: "w-a_1" }), { id: 3, repo: "o/r" });
  // A malformed record is left out.
  for (const bad of ["", "w&takeover=1", "x".repeat(65), null]) {
    assert.deepEqual(openPayload({ console: true, repo: "o/r", record: bad }), { console: 1, repo: "o/r" });
  }
});

test("unanswered reads the peers a session list did not hear from", () => {
  const { unanswered, UNANSWERED_HEADER } = load();
  assert.equal(UNANSWERED_HEADER, "x-ralphy-unanswered");
  // No header: every peer answered.
  assert.deepEqual([...unanswered(null)], []);
  assert.deepEqual([...unanswered(undefined)], []);
  assert.deepEqual([...unanswered("")], []);
  assert.deepEqual([...unanswered("01ARZ3NDEKTSV4RRFFQ69G5FAZ")], ["01ARZ3NDEKTSV4RRFFQ69G5FAZ"]);
  assert.deepEqual(
    [...unanswered(" 01ARZ3NDEKTSV4RRFFQ69G5FAZ , 01ARZ3NDEKTSV4RRFFQ69G5FAX,")],
    ["01ARZ3NDEKTSV4RRFFQ69G5FAZ", "01ARZ3NDEKTSV4RRFFQ69G5FAX"],
  );
});
