// Unit tests for assets/ui/wb-projects-store.ts, the Alpine store `projects`:
// the open project, the list of projects, the fleet, the agents and the live
// sessions. It is built here without Alpine (ADR-0073 D3, D6), as `main.ts`
// builds it for `Alpine.store`; the reads run on a page of the harness, whose
// stores are real.
import { test } from "node:test";
import assert from "node:assert/strict";
import { loadShell } from "./harness.mjs";
import { projectsStore } from "../assets/ui/wb-projects-store.ts";

test("a new store has no open project and no projects", () => {
  const store = projectsStore();
  assert.equal(store.openSlug, null);
  assert.deepEqual(store.projects, []);
  // Each page has its own list.
  assert.notEqual(projectsStore().projects, store.projects);
});

test("setOpen and setProjects are the writers, and the open row is found by its ref", () => {
  const store = projectsStore();
  const local = { slug: "owner/repo", branch: "main" };
  const peer = { key: "01KY/owner/repo", slug: "owner/repo", daemon: "01KY", branch: "dev" };
  store.setProjects([local, peer]);
  assert.equal(store.openProject(), null);
  assert.equal(store.openProjectBranch(), "current");
  // The same slug on a peer is another row.
  store.setOpen(store.repoRef(peer));
  assert.equal(store.openProject(), peer);
  assert.equal(store.openProjectBranch(), "dev");
  assert.equal(store.rowOpen(local), false);
  assert.equal(store.rowOpen(peer), true);
  store.setOpen(null);
  assert.equal(store.openSlug, null);
});

// The branch dialog and the Changes view write a row's git fields only
// through the store.
test("setRowGit sets the git fields it is given on the row, and leaves the others", () => {
  const store = projectsStore();
  const row = { slug: "owner/repo", branch: "main", branches: ["main"], head: { kind: "branch", name: "main" } };
  store.setProjects([row]);
  store.setRowGit(row, { branch: "side" });
  assert.deepEqual(row, { slug: "owner/repo", branch: "side", branches: ["main"], head: { kind: "branch", name: "main" } });
  store.setRowGit(row, { branches: ["main", "new"], branch: "new" });
  assert.deepEqual(row.branches, ["main", "new"]);
  assert.equal(row.branch, "new");
  store.setRowGit(row, { head: { kind: "detached", sha: "abc1234" } });
  assert.deepEqual(row, { slug: "owner/repo", branch: "new", branches: ["main", "new"], head: { kind: "detached", sha: "abc1234" } });
});

// The sidebar gives the rows its search keeps; the files and the wake read
// every project. A peer's group comes from the peer list either way.
test("fleetGroups groups the rows it is given, and every project without them", () => {
  const store = projectsStore({ messages: { flash() {} } });
  const local = { slug: "owner/a", branch: "main" };
  const remote = { key: "01KY/owner/b", slug: "owner/b", daemon: "01KY", branch: "main" };
  store.setProjects([local, remote]);
  store.fleetPeers = [{ daemon_id: "01KY", name: "wsl", environment: "WSL: U", state: "reachable" }];
  const rows = (groups) => groups.map((g) => [g.daemon, g.rows.map((r) => r.slug)]);
  assert.deepEqual(rows(store.fleetGroups([local])), [["", ["owner/a"]], ["01KY", []]]);
  assert.deepEqual(rows(store.fleetGroups()), [["", ["owner/a"]], ["01KY", ["owner/b"]]]);
});

// --- the projects, the fleet and the sessions (moved from app.test.mjs, #621) ---

const VISIBLE = { document: { visibilityState: "visible", hasFocus: () => true } };

test("wakePeerFor wakes the daemon of a sleeping peer's row only", () => {
  const state = loadShell().state.$store.projects;
  const peer = "01KY0000000000000000000000";
  const woken = [];
  state.wakePeer = (daemon) => woken.push(daemon);
  let groups = [{ daemon: peer, state: "asleep", nudgeable: true }];
  state.fleetGroups = () => groups;
  state.wakePeerFor(`${peer}/owner/repo`);
  assert.deepEqual(woken, [peer]);
  // CONTROLS: a local row and an awake peer are not woken.
  state.wakePeerFor("owner/repo");
  groups = [{ daemon: peer, state: "online", nudgeable: true }];
  state.wakePeerFor(`${peer}/owner/repo`);
  assert.deepEqual(woken, [peer]);
});

// `/api/repos` carries `head` beside `branch` (#510): a detached HEAD has no
// branch, and the row must still name its commit.
test("loadRepos keeps the head of a detached repo for the row title", async () => {
  const { state: shell } = loadShell();
  const state = shell.$store.projects;
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  const row = {
    slug: "o/r",
    path: "/r",
    reachable: true,
    branch: null,
    head: { kind: "detached", sha: "abc1234" },
    dirty: false,
    remote: null,
  };
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, json: async () => [row] });
  try {
    await state.loadRepos();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(state.projects.length, 1);
  assert.equal(shell.rowTitle(state.projects[0]), "o/r · abc1234");
});

// The sidebar dot says GitHub only when github.com is the remote's host.
test("loadRepos marks a row github only for a remote hosted on github.com", async () => {
  const state = loadShell().state.$store.projects;
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  const row = (slug, remote) => ({ slug, path: "/" + slug, reachable: true, branch: "main", dirty: false, remote });
  const rows = [
    row("o/gh", "git@github.com:o/gh.git"),
    row("o/fake", "https://github.com.evil.example/o/fake"),
    row("o/none", null),
  ];
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, json: async () => rows });
  try {
    await state.loadRepos();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    state.projects.map((p) => [p.slug, p.remote]),
    [["o/gh", "github"], ["o/fake", "local"], ["o/none", "local"]],
  );
});

// Answers each fetch with the next reply in `replies` (`{ status, body }`).
function scriptedFetch(replies) {
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => {
    const { status, body } = replies.shift() || { status: 500, body: null };
    return { ok: status === 200, status, json: async () => body };
  };
  return () => (globalThis.fetch = realFetch);
}

test("a failed project read after a good one keeps the list, marked not current", async () => {
  const state = loadShell().state.$store.projects;
  state.loadFleet = () => {};
  state.refreshLive = () => {};
  const restore = scriptedFetch([
    { status: 200, body: [{ slug: "a/b", path: "/ab", reachable: true }] },
    { status: 500, body: null },
  ]);
  try {
    await state.loadRepos();
    await state.loadRepos();
  } finally {
    restore();
  }
  assert.equal(state.projects.length, 1);
  assert.equal(state.projects[0].slug, "a/b");
  assert.match(state.reposError, /Not current: the daemon answered 500/);
});

test("a failed first project read is empty and says why", async () => {
  const state = loadShell().state.$store.projects;
  state.refreshLive = () => {};
  const restore = scriptedFetch([{ status: 500, body: null }]);
  try {
    await state.loadRepos();
  } finally {
    restore();
  }
  assert.deepEqual(state.projects, []);
  assert.equal(state.reposError, "Could not load the projects from the daemon: the daemon answered 500.");
});

test("a failed fleet read after a good one keeps the peers and their rows", async () => {
  const state = loadShell().state.$store.projects;
  const fleet = {
    peers: [{ daemon_id: "p1", name: "wsl", environment: "WSL: U", state: "reachable" }],
    repos: [{ key: "p1/o/r", slug: "o/r", daemon_id: "p1", reachable: true }],
  };
  const restore = scriptedFetch([
    { status: 200, body: fleet },
    { status: 502, body: null },
  ]);
  try {
    await state.loadFleet();
    state.setProjects(state.projects.filter((p) => !p.daemon));
    await state.loadFleet();
  } finally {
    restore();
  }
  assert.equal(state.fleetPeers.length, 1);
  assert.equal(state.projects.filter((p) => p.daemon === "p1").length, 1);
  assert.match(state.fleetError, /Not current: the daemon answered 502/);
});

test("a failed session read after a good one keeps the list, marked not current", async () => {
  const state = loadShell().state.$store.projects;
  const restore = scriptedFetch([
    { status: 200, body: [{ id: 1, repo: "o/r" }] },
    { status: 500, body: null },
  ]);
  try {
    await state.refreshLive();
    await state.refreshLive();
  } finally {
    restore();
  }
  assert.equal(state.liveSessions.length, 1);
  assert.match(state.sessionsError(), /Not current/);
});

test("two fleet reads close together list each peer row once", async () => {
  const state = loadShell().state.$store.projects;
  state.setProjects([{ slug: "a/b", tree: [] }]);
  const fleet = {
    peers: [{ daemon_id: "p1", name: "wsl", environment: "WSL: U", state: "reachable" }],
    repos: [{ key: "p1/o/r", slug: "o/r", daemon_id: "p1", reachable: true }],
  };
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => fleet });
  try {
    await Promise.all([state.loadFleet(), state.loadFleet()]);
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.equal(state.projects.filter((p) => p.daemon === "p1").length, 1);
  assert.equal(state.projects.filter((p) => !p.daemon).length, 1);
});

test("fleet reads asked now share the read in flight, and a later ask reads again", async () => {
  const state = loadShell().state.$store.projects;
  state.setProjects([]);
  let reads = 0;
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    if (url === "/api/fleet") reads += 1;
    return { ok: true, status: 200, json: async () => ({ peers: [], repos: [] }) };
  };
  try {
    await Promise.all([state.readFleetNow(), state.readFleetNow(), state.readFleetNow()]);
    assert.equal(reads, 1);
    await state.readFleetNow();
    assert.equal(reads, 2);
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("the consoles get the store's fleet read with the fleet", () => {
  let hooks = null;
  const { state: shell, window } = loadShell();
  const state = shell.$store.projects;
  window.WBConsole = { ingestFleet: (_groups, h) => (hooks = h) };
  let asked = 0;
  state.readFleetNow = () => {
    asked += 1;
    return Promise.resolve();
  };
  state.shareFleet();
  hooks.read();
  assert.equal(asked, 1);
});

test("a good fleet read is sent to the files", async () => {
  const events = [];
  const state = loadShell({ window: { dispatchEvent: (e) => (events.push(e), true) } }).state.$store.projects;
  const sent = (type) => events.filter((e) => e.type === type).map((e) => e.detail ?? null);
  const realFetch = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, status: 200, json: async () => ({ peers: [], repos: [] }) });
  try {
    await state.loadFleet();
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(sent("workbench:fleet-read"), [null]);
});

test("a project read keeps the peer rows and the live dots until the fleet and the sessions answer", async () => {
  const state = loadShell(VISIBLE).state.$store.projects;
  const peer = { key: "d/x", slug: "x", daemon: "d", state: "idle" };
  state.setProjects([{ slug: "a", state: "working", env: "wsl" }, peer]);
  state._fleetRows = [peer];
  const realFetch = globalThis.fetch;
  // `/api/repos` answers; the fleet and the sessions never do.
  globalThis.fetch = (url) =>
    String(url) === "/api/repos"
      ? Promise.resolve({ ok: true, status: 200, json: async () => [{ slug: "a", reachable: true }] })
      : new Promise(() => {});
  try {
    await state.loadRepos({ git: false });
  } finally {
    globalThis.fetch = realFetch;
  }
  assert.deepEqual(
    state.projects.map((p) => p.slug),
    ["a", "x"],
    "the peer row stays",
  );
  assert.equal(state.projects[0].state, "working", "the live dot stays");
  assert.equal(state.projects[0].env, "wsl");
});

// ADR-0070 D4: a peer the daemon cannot read lists no project, so the sidebar
// says so instead of showing a fleet with that peer missing.
test("a peer the daemon could not read is named in the sidebar", async () => {
  const state = loadShell(VISIBLE).state.$store.projects;
  state.setProjects([]);
  const restore = scriptedFetch([
    {
      status: 200,
      body: {
        peers: [
          { daemon_id: "ok", state: "online" },
          { daemon_id: "malformed:/p/peers", state: "malformed", name: "/p/peers", diagnosis: "cannot list /p/peers: access denied" },
        ],
        repos: [],
      },
    },
  ]);
  try {
    await state.loadFleet();
  } finally {
    restore();
  }
  assert.equal(state.fleetRejectNote, "Could not read a peer: cannot list /p/peers: access denied");
  // NEGATIVE CONTROL: a fleet with no bad peer says nothing.
  assert.equal(state.fleetRejectText([{ state: "online" }]), "");
});
