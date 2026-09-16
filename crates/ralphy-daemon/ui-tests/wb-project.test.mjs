// Unit tests for assets/ui/wb-project.js — how a project reads in the sidebar.
//
// These came over from app.test.mjs with the folds they exercise (ADR-0057 D4).
// They were written as CHARACTERIZATION tests against `shell()` before the
// extraction and are unchanged here except for the receiver: same inputs, same
// expected outputs. That is what makes the extraction a refactor — the
// assertions predate it and did not move an inch.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const UI = join(dirname(fileURLToPath(import.meta.url)), "../assets/ui");
const SRC = readFileSync(join(UI, "wb-project.js"), "utf8");

function load() {
  const window = {};
  new Function("window", SRC)(window);
  return window.WBProject;
}

const wb = load();

test("repoLabel re-labels only a remoteless repo, off its directory name", () => {
  // ADR-0008 D7 keys a repo with no remote as `path-<hash>`, which is twenty
  // useless characters in a fixed column. The basename is what the operator
  // calls it (#332).
  assert.equal(wb.repoLabel({ slug: "path-9f2a1c", path: "C:\\src\\widget" }), "WIDGET");
  assert.equal(wb.repoLabel({ slug: "path-9f2a1c", path: "/home/me/widget" }), "WIDGET");
  // Trailing separators are stripped FIRST, or the basename is the empty string.
  assert.equal(wb.repoLabel({ slug: "path-9f2a1c", path: "C:\\src\\widget\\" }), "WIDGET");
  assert.equal(wb.repoLabel({ slug: "path-9f2a1c", path: "/home/me/widget/" }), "WIDGET");
  // NEGATIVE CONTROL, and the reason the `/` test is not optional: a real
  // GitHub repo genuinely NAMED `path-utils` must never be re-labelled off
  // DISK. `slug_from_url` always yields `owner/repo`, so the slash decides.
  // Note what the guarantee is and is not: the label is still the slug's last
  // segment upper-cased — `owner/path-utils` reads `PATH-UTILS`, not
  // `SOMETHING-ELSE`. The directory is what must not leak in.
  assert.equal(
    wb.repoLabel({ slug: "owner/path-utils", path: "C:\\src\\something-else" }),
    "PATH-UTILS",
  );
  assert.equal(wb.repoLabel({ slug: "owner/repo", path: "C:\\src\\elsewhere" }), "REPO");
  // No path to fall back on: the slug stands, ugly or not.
  assert.equal(wb.repoLabel({ slug: "path-9f2a1c", path: "" }), "PATH-9F2A1C");
});

test("rowTitle says branch and dirtiness for a local repo, environment for a peer", () => {
  assert.equal(
    wb.rowTitle({ slug: "owner/repo", branch: "main", dirty: false }),
    "owner/repo · main",
  );
  assert.equal(
    wb.rowTitle({ slug: "owner/repo", branch: "main", dirty: true }),
    "owner/repo · main (uncommitted changes)",
  );
  // A repo whose branch has not been read yet says only what it knows.
  assert.equal(wb.rowTitle({ slug: "owner/repo", branch: "" }), "owner/repo");
  // A peer row is a different fact: the environment, and never the branch.
  assert.equal(
    wb.rowTitle({ slug: "owner/repo", daemon: true, env: "WSL: Ubuntu-22.04", branch: "main" }),
    "owner/repo · WSL: Ubuntu-22.04",
  );
});

test("canSwitchBranch and branchChipTitle refuse an unreachable repo", () => {
  assert.equal(wb.canSwitchBranch({ state: "ok" }), true);
  assert.equal(wb.canSwitchBranch({ state: "offline" }), false);
  assert.equal(
    wb.branchChipTitle({ state: "offline", branch: "main" }),
    "repo unreachable — branch switching unavailable",
  );
  assert.equal(
    wb.branchChipTitle({ state: "ok", branch: "main", dirty: false }),
    "switch branch — main",
  );
  assert.equal(
    wb.branchChipTitle({ state: "ok", branch: "main", dirty: true }),
    "switch branch (uncommitted changes) — main",
  );
  // NEGATIVE CONTROL for the extraction itself: `branchChipTitle` used to reach
  // its gate through `this.canSwitchBranch`. It now calls the module-local one,
  // and the two must still be the SAME rule — a chip that offers a switch the
  // row refuses is the defect this pins.
  for (const state of ["ok", "offline", "unknown"]) {
    const p = { state, branch: "main" };
    const offers = !wb.branchChipTitle(p).startsWith("repo unreachable");
    assert.equal(offers, wb.canSwitchBranch(p), `the chip and the gate disagree for ${state}`);
  }
});

test("issueUrl builds a link only from a github remote", () => {
  assert.equal(
    wb.issueUrl("https://github.com/owner/repo.git", 42),
    "https://github.com/owner/repo/issues/42",
  );
  // The SSH form, and the trailing-slash form, resolve the same.
  assert.equal(
    wb.issueUrl("git@github.com:owner/repo.git", 42),
    "https://github.com/owner/repo/issues/42",
  );
  assert.equal(
    wb.issueUrl("https://github.com/owner/repo/", 42),
    "https://github.com/owner/repo/issues/42",
  );
  // NEGATIVE CONTROL: null, not a guessed URL. A non-GitHub forge and a repo
  // with no remote must each yield nothing to link to — a link that 404s is
  // worse than no link.
  assert.equal(wb.issueUrl("https://gitlab.com/owner/repo.git", 42), null);
  assert.equal(wb.issueUrl("", 42), null);
  assert.equal(wb.issueUrl(null, 42), null);
  assert.equal(wb.issueUrl(undefined, 42), null);
});

test("worktreeRows puts primary first and reads each entry's branch and dirty flag", () => {
  // The picker's Worktrees section (#403, ADR-0063 §4): `primary` leads with
  // the project's current branch, then the listing in git's own order.
  // Two entries, NOT alphabetical (`zed` before `wt-a`): git's listing order is
  // add order, and a fold that sorted would swap them.
  assert.deepEqual(
    wb.worktreeRows(
      {
        primary: "C:/r",
        worktrees: [
          { name: "zed", path: "C:/r/.ralphy/worktrees/zed", branch: "zed", base: "", dirty: false },
          { name: "wt-a", path: "C:/r/.ralphy/worktrees/wt-a", branch: "wt-a", base: "main", dirty: true },
        ],
      },
      "main",
      false,
    ),
    [
      { name: "primary", path: "C:/r", branch: "main", dirty: false, primary: true },
      { name: "zed", path: "C:/r/.ralphy/worktrees/zed", branch: "zed", dirty: false, primary: false },
      { name: "wt-a", path: "C:/r/.ralphy/worktrees/wt-a", branch: "wt-a", dirty: true, primary: false },
    ],
  );
  // NEGATIVE CONTROL: no worktrees means NO rows — a `primary`-only section
  // would be a new visible thing on every picker, and the AC is byte-identical.
  assert.deepEqual(wb.worktreeRows({ primary: "C:/r", worktrees: [] }, "main", false), []);
  assert.deepEqual(wb.worktreeRows(null, "main", false), []);
  assert.deepEqual(wb.worktreeRows({ worktrees: "nope" }, "main", false), []);
  // `dirty` is a strict boolean: the primary's flag is the project's, an
  // entry's truthy string is not a dirty tree.
  const rows = wb.worktreeRows({ primary: "p", worktrees: [{ name: "x", dirty: "yes" }] }, "main", true);
  assert.equal(rows[0].dirty, true);
  assert.equal(rows[1].dirty, false);
  assert.equal(rows[1].branch, "");
});

test("worktreeCreateRow offers the create row whenever the listing arrived, even an empty one", () => {
  // The first worktree must be creatable from the picker (#405): an EMPTY
  // listing still yields the row. `base` is the label's branch, verbatim.
  assert.deepEqual(wb.worktreeCreateRow({ primary: "C:/r", worktrees: [] }, "main"), {
    label: "+ new worktree from main",
    base: "main",
    notice: "gitignored files come along only via settings.json worktree.copy / worktree.share",
  });
  assert.deepEqual(
    wb.worktreeCreateRow({ primary: "C:/r", worktrees: [{ name: "wt-a" }] }, "feat/x"),
    { label: "+ new worktree from feat/x", base: "feat/x", notice: "gitignored files come along only via settings.json worktree.copy / worktree.share" },
  );
  // NEGATIVE CONTROLS: no daemon answer → no row, the static shell stays
  // byte-identical; a malformed listing is not an answer either.
  assert.equal(wb.worktreeCreateRow(null, "main"), null);
  assert.equal(wb.worktreeCreateRow({ worktrees: "nope" }, "main"), null);
  // A detached primary has no branch to cut from: no row, not a dead end.
  assert.equal(wb.worktreeCreateRow({ primary: "C:/r", worktrees: [] }, "HEAD"), null);
  assert.equal(wb.worktreeCreateRow({ primary: "C:/r", worktrees: [] }, ""), null);
});

test("chipLabel names the worktree's branch beside its name, never the primary's", () => {
  // The chip follows the SELECTED checkout (#406, ADR-0063 §4): with none it
  // is the project's branch as before; with one it is `<branch> · <name>`
  // read from the worktree.list entry — `HEAD` for a detached one — and the
  // bare name until the listing lands.
  assert.equal(wb.chipLabel({ branch: "main" }, null, null), "main");
  assert.equal(
    wb.chipLabel({ branch: "main" }, "wt-a", { primary: "C:/r", worktrees: [{ name: "wt-a", branch: "wt-a" }] }),
    "wt-a · wt-a",
  );
  assert.equal(
    wb.chipLabel({ branch: "main" }, "wt-a", { primary: "C:/r", worktrees: [{ name: "wt-a", branch: "" }] }),
    "HEAD · wt-a",
  );
  assert.equal(wb.chipLabel({ branch: "main" }, "wt-a", null), "wt-a");
  // NEGATIVE CONTROL: a listing that does not carry the selected name must
  // not borrow the PRIMARY's branch — `main · wt-a` would name a branch the
  // worktree is not on.
  assert.equal(
    wb.chipLabel({ branch: "main" }, "wt-a", { primary: "C:/r", worktrees: [{ name: "wt-b", branch: "wt-b" }] }),
    "wt-a",
  );
  assert.equal(wb.chipLabel({ branch: "main" }, "", null), "main");
});

test("the chip's tooltip and dirty dot describe the selected worktree, not the primary (#407)", () => {
  // The switch lands in the selected worktree, so its tooltip names THAT
  // tree's branch and its dirtiness comes from the listing; the primary's
  // `p.dirty` and `p.branch` must not leak beside a worktree name.
  const p = { state: "ok", branch: "main", dirty: true };
  const clean = { primary: "C:/r", worktrees: [{ name: "wt-a", branch: "side", dirty: false }] };
  const dirty = { primary: "C:/r", worktrees: [{ name: "wt-a", branch: "side", dirty: true }] };
  assert.equal(wb.branchChipTitle(p, "wt-a", clean), "switch branch — side · wt-a");
  assert.equal(wb.branchChipTitle(p, "wt-a", dirty), "switch branch (uncommitted changes) — side · wt-a");
  assert.equal(wb.chipDirty(p, "wt-a", clean), false, "a dirty primary does not dot a clean worktree");
  assert.equal(wb.chipDirty(p, "wt-a", dirty), true);
  assert.equal(wb.chipDirty({ ...p, dirty: false }, "wt-a", dirty), true);
  // No selection: exactly as before.
  assert.equal(wb.branchChipTitle(p, null, null), "switch branch (uncommitted changes) — main");
  assert.equal(wb.chipDirty(p, null, null), true);
  assert.equal(wb.chipDirty({ ...p, dirty: false }, null, null), false);
  // Selection with no listing yet: the bare name, never the primary's branch,
  // and no dot (nothing is known).
  assert.equal(wb.branchChipTitle(p, "wt-a", null), "switch branch — wt-a");
  assert.equal(wb.chipDirty(p, "wt-a", null), false);
  // Unreachable wins over everything.
  assert.equal(
    wb.branchChipTitle({ ...p, state: "offline" }, "wt-a", dirty),
    "repo unreachable — branch switching unavailable",
  );
});

test("checkoutAfter drops the selection only on unknown checkout", () => {
  // The one reply that means "the worktree is gone" resets the selection;
  // any other error (a missing file, a refused write) keeps it.
  assert.equal(wb.checkoutAfter("wt-a", { status: "error", message: "unknown checkout" }), null);
  assert.equal(wb.checkoutAfter("wt-a", { status: "ok" }), "wt-a");
  assert.equal(wb.checkoutAfter("wt-a", { status: "error", message: "not found" }), "wt-a");
  assert.equal(wb.checkoutAfter("wt-a", { status: "error", reason: "not found" }), "wt-a");
  assert.equal(wb.checkoutAfter("wt-a", null), "wt-a");
  assert.equal(wb.checkoutAfter(null, { status: "error", message: "unknown checkout" }), null);
});

test("checkoutAfterListing drops the selection when the listing no longer carries it", () => {
  // The listing is the truth after a remove: a `branch kept` reply is an error
  // whose directory is gone, so the status cannot decide — the re-read does.
  assert.equal(wb.checkoutAfterListing("wt-a", { primary: "p", worktrees: [{ name: "wt-b" }] }), null);
  assert.equal(wb.checkoutAfterListing("wt-a", { primary: "p", worktrees: [{ name: "wt-a" }] }), "wt-a");
  // A listing that never answered says nothing and keeps the selection.
  assert.equal(wb.checkoutAfterListing("wt-a", null), "wt-a");
  assert.equal(wb.checkoutAfterListing("wt-a", { primary: "p" }), "wt-a");
  assert.equal(wb.checkoutAfterListing(null, { primary: "p", worktrees: [{ name: "wt-a" }] }), null);
});

// ADR-0059: the many-sessions fold. `waiting` outranks everything (the agent
// is asking), `working` the rest, `unknown` beats `done`; rows without a
// state say nothing; no state at all is `null`.
test("agentStateOf ranks waiting over working over unknown over done", () => {
  const s = (state) => ({ agent_state: state ? { state, since: "t" } : undefined });
  assert.equal(wb.agentStateOf([]), null);
  assert.equal(wb.agentStateOf([s(null), { agent: "console" }]), null);
  assert.equal(wb.agentStateOf([s("done")]), "done");
  assert.equal(wb.agentStateOf([s("done"), s("unknown")]), "unknown");
  assert.equal(wb.agentStateOf([s("unknown"), s("working")]), "working");
  assert.equal(wb.agentStateOf([s("working"), s("waiting"), s("done")]), "waiting");
  assert.equal(wb.agentStateOf([s("bogus")]), null, "an unknown word is no state");
});

// The dot per worktree row: sessions join rows on `checkout` (`primary` is
// the sessions with none); a row with no session has no entry.
test("worktreeStates joins the repo's sessions to its rows by checkout", () => {
  const rows = [
    { name: "primary", primary: true },
    { name: "wt-a", primary: false },
    { name: "wt-b", primary: false },
  ];
  const mine = [
    { id: 1, agent_state: { state: "working", since: "t" } },
    { id: 2, checkout: "wt-a", agent_state: { state: "done", since: "t" } },
    { id: 3, checkout: "wt-a", agent_state: { state: "waiting", since: "t" } },
    { id: 4, checkout: "wt-c", agent_state: { state: "waiting", since: "t" } },
  ];
  assert.deepEqual(wb.worktreeStates(rows, mine), { primary: "working", "wt-a": "waiting" });
  assert.deepEqual(wb.worktreeStates(rows, []), {});
  assert.deepEqual(wb.worktreeStates([], mine), {});
});
