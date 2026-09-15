/* ---------------------------------------------------------------------------
   How a project reads in the sidebar — the label, the row's tooltip, whether
   its branch can be switched, and the forge URL for one of its issues.

   Lifted out of `app.js` under ADR-0057, and every one of these is a pure
   function of a project record: no `this`, no fetch, no DOM. `shell()` keeps a
   one-line method per fold that forwards here, which is the idiom `app.js`
   already uses for `WBFleet`, `WBChanges` and `WBRun` — a fold with a real
   domain lives in a module, and the component delegates.

   Why these five and not more: they are what a project row SAYS — and, with
   `worktreeRows` and `worktreeCreateRow`, what the branch picker SAYS about a
   project's checkouts (ADR-0063 §2/§4); `chipLabel` and `checkoutAfter` are
   what the chip says under a selected checkout and when that selection is
   dropped (#406). The acts that row can start — opening the branch modal,
   removing the project, refreshing its changes — read and write component
   state and stay where that state is.
   --------------------------------------------------------------------------- */
window.WBProject = (function () {
  // Sidebar row label: just the repo name (last slug segment), UPPERCASED. The
  // full `owner/repo` already shows in the top crumb, so trimming the owner here
  // declutters the accordion.
  function repoLabel(p) {
    // A remoteless repo has no name in its slug: ADR-0008 D7 keys it
    // `path-<hash>`, which reads as twenty useless characters in a fixed 300px
    // column. The directory basename is what the operator calls it. The `/`
    // test is not optional — `slug_from_url` always yields `owner/repo`, so a
    // real GitHub repo named `owner/path-utils` is NOT this case and must
    // never be re-labelled off disk (#332).
    if (!p.slug.includes("/") && p.slug.startsWith("path-")) {
      // Windows and POSIX in one pass. Trailing separators go FIRST, or
      // `C:\src\widget\` basenames to the empty string.
      const base = String(p.path || "")
        .replace(/[\\/]+$/, "")
        .split(/[\\/]/)
        .pop();
      if (base) return base.toUpperCase();
    }
    return (p.slug.split("/").pop() || p.slug).toUpperCase();
  }

  // The row's tooltip. A PEER row says its environment and never its branch:
  // the branch belongs to a checkout this daemon can see, and a peer's is not
  // one of those.
  function rowTitle(p) {
    if (p.daemon) {
      return `${p.slug} · ${p.env}`;
    }
    if (!p.branch) return p.slug;
    return `${p.slug} · ${p.branch}${p.dirty ? " (uncommitted changes)" : ""}`;
  }

  // Switching is possible only when the daemon can reach the repo on disk.
  // NOT gated on `remote`: a local-only repo (no GitHub) is still a git
  // checkout with branches — it's an *unreachable* path (state offline) the
  // daemon can't run `git branch`/`checkout` against.
  function canSwitchBranch(p) {
    return p.state !== "offline";
  }

  function branchChipTitle(p) {
    if (!canSwitchBranch(p)) return "repo unreachable — branch switching unavailable";
    return (p.dirty ? "switch branch (uncommitted changes) — " : "switch branch — ") + p.branch;
  }

  // The forge link for one issue, or `null` when there is nothing honest to
  // link to. Both remote spellings resolve — `https://github.com/owner/repo.git`
  // and `git@github.com:owner/repo` — and a non-GitHub forge yields null rather
  // than a guessed URL, because a link that 404s is worse than no link.
  function issueUrl(remoteUrl, number) {
    const url = remoteUrl || "";
    if (!url.includes("github.com")) return null;
    const m = url.match(/github\.com[/:]([^/]+)\/(.+?)(?:\.git)?\/?$/);
    if (!m) return null;
    return `https://github.com/${m[1]}/${m[2]}/issues/${number}`;
  }

  // The picker's Worktrees rows from a `worktree.list` reply: the primary tree
  // first (its branch is the project's current one), then each workbench
  // worktree in git's listing order. An empty or malformed listing yields NO
  // rows — not even `primary` — so a project without worktrees renders the
  // picker exactly as before the section existed (#403). `dirty` is a strict
  // boolean read: a truthy string is not a dirty tree.
  function worktreeRows(listing, currentBranch, primaryDirty) {
    if (!listing || !Array.isArray(listing.worktrees) || !listing.worktrees.length) {
      return [];
    }
    return [
      {
        name: "primary",
        path: String(listing.primary || ""),
        branch: String(currentBranch || ""),
        dirty: primaryDirty === true,
        primary: true,
      },
      ...listing.worktrees.map((w) => ({
        name: String(w.name || ""),
        path: String(w.path || ""),
        branch: String(w.branch || ""),
        dirty: w.dirty === true,
        primary: false,
      })),
    ];
  }

  // The picker's "+ new worktree from <branch>" row (#405). Offered whenever
  // the daemon ANSWERED the listing — an empty one included, so the first
  // worktree is creatable from the picker — and never before (the static
  // shell stays byte-identical). `base` is what the row's label promises and
  // what the create sends, so the two cannot drift. A detached primary
  // reports `HEAD` as its branch: no base to cut from, no row (the CLI's
  // `--base` is the way there).
  function worktreeCreateRow(listing, currentBranch) {
    if (!listing || !Array.isArray(listing.worktrees)) return null;
    const base = String(currentBranch || "");
    if (!base || base === "HEAD") return null;
    return {
      label: "+ new worktree from " + base,
      base,
      notice: "gitignored files are not copied",
    };
  }

  // The branch chip's text under a selected checkout (#406, ADR-0063 §4):
  // `<worktree branch> · <name>` when the `worktree.list` entry is known
  // (`HEAD` for a detached one), the bare `<name>` until the listing lands,
  // and the project's own branch with no selection. NEVER the primary's branch
  // beside a worktree name — that would name a branch the tree is not on.
  function chipLabel(p, checkout, listing) {
    if (!checkout) return p.branch;
    const entry =
      listing && Array.isArray(listing.worktrees)
        ? listing.worktrees.find((w) => w && w.name === checkout)
        : null;
    if (!entry) return checkout;
    return `${entry.branch || "HEAD"} · ${checkout}`;
  }

  // What the selection is after a verb replied: `null` on the ONE reply that
  // means the worktree is gone (`unknown checkout` — the daemon resolves the
  // name against the pointer file on every read), the same name on anything
  // else. A refused write or a missing file is not a reason to drop it.
  function checkoutAfter(checkout, reply) {
    if (!checkout) return null;
    const unknown = reply && reply.status === "error" && reply.message === "unknown checkout";
    return unknown ? null : checkout;
  }

  return {
    repoLabel,
    rowTitle,
    canSwitchBranch,
    branchChipTitle,
    issueUrl,
    worktreeRows,
    worktreeCreateRow,
    chipLabel,
    checkoutAfter,
  };
})();
