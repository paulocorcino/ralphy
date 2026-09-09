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
