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
// Sidebar row label: just the repo name (last segment), UPPERCASED. The
// full `owner/repo` already shows in the top crumb, so trimming the owner here
// declutters the accordion.
/** A project row of the sidebar (app.js `loadRepos`). Not typed field by
 * field yet (ADR-0075, the last phase narrows it). */
type Project = any;
/** A `worktree.list` reply for one repo. */
type Listing = any;

function repoLabel(p: Project) {
  return (projectName(p).split("/").pop() || p.slug).toUpperCase();
}

// What the operator calls the project: the daemon's `name` (`owner/repo`, or
// the folder of a remoteless repo, whose slug is a `path-<hash>` key). A row
// from a daemon older than that field falls back to the slug.
function projectName(p: Project) {
  return p.name || p.slug;
}

// What a tooltip says to tell two projects apart: `owner/repo`, or the full
// folder of a remoteless repo. Never the hash key.
function projectTitle(p: Project) {
  return projectName(p) !== p.slug && p.path ? p.path : projectName(p);
}

// The row's tooltip. A PEER row says its environment and never its branch:
// the branch belongs to a checkout this daemon can see, and a peer's is not
// one of those.
function rowTitle(p: Project) {
  const who = projectTitle(p);
  if (p.daemon) {
    return `${who} · ${p.env}`;
  }
  const label = headLabel(p);
  if (!label) return who;
  return `${who} · ${label}${p.dirty ? " (uncommitted changes)" : ""}`;
}

// What the project's HEAD is called: its branch, else the short sha of a
// detached HEAD (as the Changes panel names it), else nothing.
function headLabel(p: Project) {
  if (p.branch) return p.branch;
  if (p.head && p.head.kind === "detached") return p.head.sha || "";
  return "";
}

// Switching is possible only when the daemon can reach the repo on disk.
// NOT gated on `remote`: a local-only repo (no GitHub) is still a git
// checkout with branches — it's an *unreachable* path (state offline) the
// daemon can't run `git branch`/`checkout` against.
function canSwitchBranch(p: Project) {
  return p.state !== "offline";
}

// Under a selected checkout (#407) the tooltip describes the WORKTREE the
// switch will land in — its branch and its dirtiness from the listing — never
// the primary's branch beside a worktree name.
function branchChipTitle(p: Project, checkout: string | null | undefined, listing: Listing) {
  if (!canSwitchBranch(p)) return "Could not switch the branch: the project cannot be reached.";
  const dirty = chipDirty(p, checkout, listing);
  // The tooltip has room the chip does not: `<branch> · <worktree>`.
  const where = checkoutEntry(checkout, listing) ? ` · ${checkout}` : "";
  return (dirty ? "Switch branch (uncommitted changes): " : "Switch branch: ") + chipLabel(p, checkout, listing) + where;
}

// The `worktree.list` entry of the selected checkout, or `null` when there is
// no selection or the listing has not landed.
function checkoutEntry(checkout: string | null | undefined, listing: Listing) {
  if (!checkout || !listing || !Array.isArray(listing.worktrees)) return null;
  return listing.worktrees.find((w: any) => w && w.name === checkout) || null;
}

// The forge link for one issue, or `null` when there is nothing honest to
// link to. Both remote spellings resolve — `https://github.com/owner/repo.git`
// and `git@github.com:owner/repo` — and a non-GitHub forge yields null rather
// than a guessed URL, because a link that 404s is worse than no link.
function issueUrl(remoteUrl: string | null | undefined, number: number) {
  const m = githubRemote(remoteUrl);
  if (!m) return null;
  return `https://github.com/${m[1]}/${m[2]}/issues/${number}`;
}

// The owner and repo of a GitHub remote, or `null`. The host is anchored:
// `github.com` must BE the host (after an optional scheme and user), not a
// part of another host's name or path.
function githubRemote(remoteUrl: string | null | undefined) {
  return (remoteUrl || "").match(/^(?:[a-z+]+:\/\/)?(?:[^@/]+@)?github\.com[/:]([^/]+)\/(.+?)(?:\.git)?\/?$/);
}

// Whether the remote is on GitHub: what the sidebar dot shows.
function isGitHubRemote(remoteUrl: string | null | undefined) {
  return githubRemote(remoteUrl) !== null;
}

// Whether the repo has a worktree at all — what shows the Files bar's
// checkout chip, and nothing else (ADR-0063 amendment 2026-09-16 b).
function hasWorktrees(listing: Listing) {
  return !!listing && Array.isArray(listing.worktrees) && listing.worktrees.length > 0;
}

// The console's "new worktree" prompt gate (#405, reshaped 2026-09-16):
// the typed name is the worktree AND its branch (ADR-0063 §2), `base` is
// what the prompt shows and what the create sends, so the two cannot
// drift. `null` — the prompt refuses — for a name the daemon would not
// take (one path segment, not a flag), one a worktree already has, a
// detached base (`HEAD`: no branch to cut from), or before the listing
// ever answered.
function worktreeCreateRow(listing: Listing, base: string, name: string) {
  if (!listing || !Array.isArray(listing.worktrees)) return null;
  base = String(base || "");
  if (!base || base === "HEAD") return null;
  name = String(name || "").trim();
  if (worktreeNameProblem(listing, name)) return null;
  return { label: `Create worktree “${name}” from ${base}`, base, name };
}

// The prompt's input mask: `raw` with every character the daemon would
// refuse taken out, so a refused key never shows in the field. What a mask
// cannot know while the name is typed (a final "." or ".lock", "@" alone, a
// taken name) stays with `worktreeNameProblem`, checked on submit.
function maskWorktreeName(raw: string) {
  return String(raw || "")
    .replace(/[\s\u0000-\u001f\u007f/\\~^:?*[]/g, "")
    .replace(/^[-.]+/, "")
    .replace(/\.{2,}/g, ".")
    .replace(/@\{+/g, "@");
}

// Why the daemon would refuse `name` as a new worktree, or "" when it would
// take it. The rules are the daemon's `well_shaped_ref` (dispatch/argv.rs)
// plus one path segment (no "/"), so the prompt refuses before any send.
function worktreeNameProblem(listing: Listing, name: string) {
  name = String(name || "").trim();
  if (!name) return "Enter a name.";
  if (name.startsWith("-")) return "The name cannot start with “-”.";
  if (/\s/.test(name)) return "The name cannot contain spaces. Use “-” instead.";
  const bad = name.match(/[\u0000-\u001f\u007f/\\~^:?*[]/);
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
  if (listing && Array.isArray(listing.worktrees) && listing.worktrees.some((w: any) => w && w.name === name)) {
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
function chipLabel(p: Project, checkout: string | null | undefined, listing: Listing) {
  if (!checkout) return headLabel(p);
  const entry = checkoutEntry(checkout, listing);
  if (!entry) return checkout;
  return entry.branch || "HEAD";
}

// The chip's dirty dot: the selected worktree's dirtiness when one is
// selected (the tree the act lands in), the primary's otherwise (#407).
function chipDirty(p: Project, checkout: string | null | undefined, listing: Listing) {
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
const STATE_RANK: Record<string, number> = { waiting: 4, working: 3, unknown: 2, done: 1, blocked: 4 };
function agentStateOf(sessions: any[] | null | undefined) {
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
function worktreeStates(rows: any[] | null | undefined, mine: any) {
  const out: Record<string, string> = {};
  for (const row of rows || []) {
    const here = (mine || []).filter((s: any) =>
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
function checkoutAfter(checkout: string | null | undefined, reply: any) {
  if (!checkout) return null;
  const unknown = reply && reply.status === "error" && reply.message === "unknown checkout";
  return unknown ? null : checkout;
}

// What the selection is after a `worktree.list` re-read: `null` when the
// selected name is absent from a listing that ANSWERED — the directory is
// gone whatever the remove reply said (`branch kept` is an error with the
// directory gone). A `null` listing says nothing and keeps it.
function checkoutAfterListing(checkout: string | null | undefined, listing: Listing) {
  if (!checkout) return null;
  if (!listing || !Array.isArray(listing.worktrees)) return checkout;
  return listing.worktrees.some((w: any) => w && w.name === checkout) ? checkout : null;
}

export const WBProject = {
  repoLabel,
  projectName,
  projectTitle,
  rowTitle,
  headLabel,
  agentStateOf,
  worktreeStates,
  canSwitchBranch,
  branchChipTitle,
  issueUrl,
  isGitHubRemote,
  hasWorktrees,
  worktreeCreateRow,
  worktreeNameProblem,
  maskWorktreeName,
  CARRY_OVER_NOTE,
  chipLabel,
  chipDirty,
  checkoutAfter,
  checkoutAfterListing,
};

// A classic script still reads this name (ADR-0075 D9).
if (typeof window !== "undefined") window.WBProject = WBProject;

declare global {
  interface Window {
    WBProject: typeof WBProject;
  }
}
