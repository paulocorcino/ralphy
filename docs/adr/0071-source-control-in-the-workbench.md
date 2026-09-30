# Source control in the workbench: one change set, refusals as values, fast-forward only

Status: accepted
Kind: feature
Protects: integrity of change, consistency of the workbench

## Context

The workbench reads and changes a repo's working tree: the Changes panel, the
sync row, stage, unstage, commit, discard, fetch, pull and push, and the
selected checkout. These rules were decided in the source-control PRDs (#297,
#314) and the checkout follow-ups (#407, #408), and were recorded only in
CONTEXT.md. On 2026-09-30 the glossary was cut back to definitions, so the
rules move here without change. This ADR records decisions already made and
built; it decides nothing new.

The code: `ralphy_core::changes`, `ralphy_core::sync`, `ralphy_core::worktree`
and `ralphy_core::checkouts`, reached by the daemon's verbs (ADR-0036).

## Decision

**D1. The change set is the single definition of a dirty tree.** It is read by
`ralphy_core::changes`: `git::is_clean_ignoring_ralphy` is
`changes(repo)?.is_empty()`, and run artifacts under the repo-root `.ralphy/`
never count. Each entry also carries an index-side and a worktree-side status.
For a tracked change these are git's two `XY` characters (a side is absent
where git reports `.`), so a path staged and then edited again is one entry
visible on both sides. Two kinds are not read per character: an untracked path
has no `XY` and counts as worktree-only, and an unmerged path is worktree-only
whatever its `XY` reads, because an unresolved conflict is not what a commit
would contain. The single status stays the derived projection — the first
non-`.` side — and is what the clean-tree definition reads.

**D2. Sync status makes no network call.** It is read by `ralphy_core::sync`,
so the counts are stale by design and always travel with a last-fetch stamp:
the mtime of `FETCH_HEAD`, absent until something actually fetched. "No
upstream" and "detached" are states, never zeroed counts.

**D3. Fetch is the operator's own act, never a timer's.** Nothing in Ralphy
refreshes remote-tracking refs on a schedule.

**D4. Pull is fast-forward only.** A diverged branch, a detached HEAD, a
missing upstream or an obstructing working tree each refuse by value, carrying
their own reason as prose. git's error string is never relayed, and no merge
or rebase is ever started.

**D5. Working-tree operations refuse by value.** They are owned by
`ralphy_core::worktree`, a sibling of `ralphy_core::sync`, not part of it: the
upstream relation and the working tree are different questions. Every refusal
is a value carrying its own prose, never an `Err` and never git's error
string: staging a path the change set does not name, committing with nothing
staged, and committing with an empty message are all answers to a reasonable
question. An `Err` is reserved for a real failure: git missing, an
unconfigured `user.email`, a repo that cannot be read.

**D6. The change set is what may be acted on.** Which side of a rename counts
is per act: **unstage** takes both paths, because `restore --staged` needs the
old one to undo the deletion half, while **stage** and **discard** take the new
path only, the old one naming no working-tree content. git is invoked with
`--literal-pathspecs`, so no filename is ever read as a pattern, and `commit`
decides before it writes: no path that returns a refusal has run
`git commit`.

**D7. Discard restores before it deletes.** Discard is the one irreversible
act here, and it has two cases with different recoverability. A tracked path's
working tree is restored from the index, which is HEAD when nothing is staged,
so a staged change is never thrown away by discarding the same path's
working-tree edit. An untracked entry is deleted, and no commit and no reflog
can bring it back; git reports an untracked directory as one entry
(`newdir/`), which is why the deletion is `git clean -d` and not a file
remove. Discard decides before it writes too: the whole list is partitioned
first, and in a mixed batch the restores run before the deletions, so the
unrecoverable act happens last.

**D8. The selected checkout is where source control acts.** While a project
has a selected checkout (ADR-0063; the desk field of the ADR-0050 amendment),
the Changes panel, the diff, the sync row and the branch chip act on it: the
git-backed verbs run in the worktree as cwd, so a stage, commit, discard, diff
or branch switch lands there and never on the primary (#407). **New console**
opens the agent inside the selected checkout for every vendor; the worktree is
the child's cwd, the session record and the `session-open` frame carry its
name, and the console title reads `<agent> · <name>`. A live console keeps its
checkout when the selection changes (#408). File writes stay on the primary
until a later slice, and the first `unknown checkout` reply drops the
selection.

## Compliance

Not required: this is a feature ADR. Each rule is pinned by behaviour tests in
`ralphy-core` (`changes`, `sync`, `worktree`), `crates/ralphy-cli/tests/verbs/`
and the daemon's `tests/command_changes*.rs`.
