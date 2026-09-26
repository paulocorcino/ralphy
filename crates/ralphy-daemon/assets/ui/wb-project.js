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
   project's checkouts (ADR-0063 §2/§4); `chipLabel`, `checkoutAfter` and
   `checkoutAfterListing` are what the chip says under a selected checkout and
   when that selection is dropped (#406, #409). The acts that row can start — opening the branch modal,
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

  // Under a selected checkout (#407) the tooltip describes the WORKTREE the
  // switch will land in — its branch and its dirtiness from the listing — never
  // the primary's branch beside a worktree name.
  function branchChipTitle(p, checkout, listing) {
    if (!canSwitchBranch(p)) return "Could not switch the branch: the project cannot be reached.";
    const dirty = chipDirty(p, checkout, listing);
    // The tooltip has room the chip does not: `<branch> · <worktree>`.
    const where = checkoutEntry(checkout, listing) ? ` · ${checkout}` : "";
    return (dirty ? "Switch branch (uncommitted changes): " : "Switch branch: ") + chipLabel(p, checkout, listing) + where;
  }

  // The `worktree.list` entry of the selected checkout, or `null` when there is
  // no selection or the listing has not landed.
  function checkoutEntry(checkout, listing) {
    if (!checkout || !listing || !Array.isArray(listing.worktrees)) return null;
    return listing.worktrees.find((w) => w && w.name === checkout) || null;
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

  // Whether the repo has a worktree at all — what shows the Files bar's
  // checkout chip, and nothing else (ADR-0063 amendment 2026-09-16 b).
  function hasWorktrees(listing) {
    return !!listing && Array.isArray(listing.worktrees) && listing.worktrees.length > 0;
  }

  // The console's "new worktree" prompt gate (#405, reshaped 2026-09-16):
  // the typed name is the worktree AND its branch (ADR-0063 §2), `base` is
  // what the prompt shows and what the create sends, so the two cannot
  // drift. `null` — the prompt refuses — for a name the daemon would not
  // take (one path segment, not a flag), one a worktree already has, a
  // detached base (`HEAD`: no branch to cut from), or before the listing
  // ever answered.
  function worktreeCreateRow(listing, base, name) {
    if (!listing || !Array.isArray(listing.worktrees)) return null;
    base = String(base || "");
    if (!base || base === "HEAD") return null;
    name = String(name || "").trim();
    if (worktreeNameProblem(listing, name)) return null;
    return { label: `Create worktree “${name}” from ${base}`, base, name };
  }

  // Why the daemon would refuse `name` as a new worktree, or "" when it would
  // take it. The rules are the daemon's `well_shaped_ref` (dispatch/argv.rs)
  // plus one path segment (no "/"), so the prompt refuses before any send.
  function worktreeNameProblem(listing, name) {
    name = String(name || "").trim();
    if (!name) return "Enter a name.";
    if (name.startsWith("-")) return "The name cannot start with “-”.";
    if (/\s/.test(name)) return "The name cannot contain spaces. Use “-” instead.";
    const bad = name.match(/[\u0000-\u001f\u007f\/\\~^:?*[]/);
    if (bad) {
      return /[\u0000-\u001f\u007f]/.test(bad[0])
        ? "The name cannot contain control characters."
        : `The name cannot contain “${bad[0]}”.`;
    }
    for (const seq of ["..", "@{"]) {
      if (name.includes(seq)) return `The name cannot contain “${seq}”.`;
    }
    if (name === "@") return "The name cannot be “@”.";
    if (name.startsWith(".") || name.endsWith(".")) return "The name cannot start or end with “.”.";
    if (name.endsWith(".lock")) return "The name cannot end with “.lock”.";
    if (listing && Array.isArray(listing.worktrees) && listing.worktrees.some((w) => w && w.name === name)) {
      return `A worktree named “${name}” already exists.`;
    }
    return "";
  }

  // What the create row's tooltip and the per-branch action say about
  // gitignored files: one sentence, here so the two cannot drift.
  const CARRY_OVER_NOTE = "Ignored files are copied only when worktree.copy or worktree.share in settings.json names them.";


  // The branch chip's text under a selected checkout (#406, ADR-0063 §4):
  // the WORKTREE's branch when the `worktree.list` entry is known (`HEAD`
  // for a detached one) — the checkout chip beside it already names the
  // tree (amendment 2026-09-16 b), so the name is not repeated — the bare
  // `<name>` until the listing lands (no checkout chip yet, so the name is
  // the only hint), and the project's own branch with no selection. NEVER
  // the primary's branch under a selection — that would name a branch the
  // tree is not on.
  function chipLabel(p, checkout, listing) {
    if (!checkout) return p.branch;
    const entry = checkoutEntry(checkout, listing);
    if (!entry) return checkout;
    return entry.branch || "HEAD";
  }

  // The chip's dirty dot: the selected worktree's dirtiness when one is
  // selected (the tree the act lands in), the primary's otherwise (#407).
  function chipDirty(p, checkout, listing) {
    if (!checkout) return !!p.dirty;
    const entry = checkoutEntry(checkout, listing);
    return !!entry && entry.dirty === true;
  }

  // ---- the agent's state as a dot (ADR-0059 §5/§6) -----------------------
  //
  // The daemon renders each live session's `agent_state` (staleness already
  // applied); the workbench folds many sessions into one word. Precedence is
  // what the operator should look at first: a `waiting` agent beats
  // everything (it is asking), `working` beats the rest, an `unknown` (a
  // green that aged out) beats `done`. `null` when no session says anything —
  // a vendor without hooks, a shell console, a peer on an older build.
  const STATE_RANK = { waiting: 4, working: 3, unknown: 2, done: 1, blocked: 4 };
  function agentStateOf(sessions) {
    let best = null;
    for (const s of sessions || []) {
      const state = s && s.agent_state && s.agent_state.state;
      if (!state || !(state in STATE_RANK)) continue;
      if (!best || STATE_RANK[state] > STATE_RANK[best]) best = state;
    }
    return best;
  }

  // The dot per worktree row: the sessions living in that checkout (`primary`
  // is the sessions with no checkout), folded by `agentStateOf`. `mine` is
  // the repo's own sessions — the caller has already matched the repo ref.
  function worktreeStates(rows, mine) {
    const out = {};
    for (const row of rows || []) {
      const here = (mine || []).filter((s) =>
        row.primary ? !s.checkout : s.checkout === row.name,
      );
      const state = agentStateOf(here);
      if (state) out[row.name] = state;
    }
    return out;
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

  // What the selection is after a `worktree.list` re-read: `null` when the
  // selected name is absent from a listing that ANSWERED — the directory is
  // gone whatever the remove reply said (`branch kept` is an error with the
  // directory gone). A `null` listing says nothing and keeps it.
  function checkoutAfterListing(checkout, listing) {
    if (!checkout) return null;
    if (!listing || !Array.isArray(listing.worktrees)) return checkout;
    return listing.worktrees.some((w) => w && w.name === checkout) ? checkout : null;
  }

  return {
    repoLabel,
    rowTitle,
    agentStateOf,
    worktreeStates,
    canSwitchBranch,
    branchChipTitle,
    issueUrl,
    hasWorktrees,
    worktreeCreateRow,
    worktreeNameProblem,
    CARRY_OVER_NOTE,
    chipLabel,
    chipDirty,
    checkoutAfter,
    checkoutAfterListing,
  };
})();
