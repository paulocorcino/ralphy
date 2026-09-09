/* ---------------------------------------------------------------------------
   How a project reads in the sidebar — the label, the row's tooltip, whether
   its branch can be switched, and the forge URL for one of its issues.

   Lifted out of `app.js` under ADR-0057, and every one of these is a pure
   function of a project record: no `this`, no fetch, no DOM. `shell()` keeps a
   one-line method per fold that forwards here, which is the idiom `app.js`
   already uses for `WBFleet`, `WBChanges` and `WBRun` — a fold with a real
   domain lives in a module, and the component delegates.

   Why these four and not more: they are what a project row SAYS. The acts that
   row can start — opening the branch modal, removing the project, refreshing
   its changes — read and write component state and stay where that state is.
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

  return { repoLabel, rowTitle, canSwitchBranch, branchChipTitle, issueUrl };
})();
