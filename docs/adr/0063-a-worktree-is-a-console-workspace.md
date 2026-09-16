# A worktree is a console's workspace, not a run's; the workbench creates it, opens consoles in it, and removes it

Status: **accepted** (2026-09-15) — implemented by #403 (list, picker
section), #404 (§5 attribution), #405 (add), #406 (`checkout`, Observe
family, desk field), #407 (git-backed family), #408 (console spawn,
`console_worktree` retired), #409 (remove behind the gates, browser flow)
and #410 (docs, amendments); PRD #402. Replaces
the *console* half of the `console_worktree` experiment and takes over the
git mechanics of [ADR-0058](./0058-checkout-per-run.md) §2 and §5 (deferred)
without its run-path changes.

_Extends [ADR-0036](./0036-workbench-daemon-integration-protocol.md) §2 with
one Query and two Mutate verbs and one optional argument on every repo-scoped
verb. Amends [ADR-0032](./0032-daemon-mode-supervised-launcher.md) §2
(the console spawn takes a checkout), [ADR-0033](./0033-interactive-usage-stateless-scan.md)
§6 (usage attribution by path), [ADR-0050](./0050-desk-layout-is-daemon-state.md)
(one desk field) and retires `RepoEntry::console_worktree`. Leaves
[ADR-0042](./0042-cursor-adapter.md) §flags and [ADR-0043](./0043-gemini-adapter.md)
at "never" for the vendors' own worktree flags, now with the mechanism that
makes "never" free._

## Two consoles on one repo share one HEAD

A workbench project is one registry row: one path, one working tree, one
HEAD (`crates/ralphy-daemon/src/registry.rs:22`). Every console opened on it
is spawned with that path as `cwd` (`lib.rs:1513-1519`), and the branch chip
in the Files bar switches that one HEAD through `branch.switch`
(`assets/ui/app.js:771-800`, `dispatch.rs:758`). Two agents on the same
project therefore edit the same files, and switching the branch for one
switches it under the other.

The experiment that ships today, `console_worktree`, hands the problem to
the vendor: `claude --worktree` (`session.rs:163-176`). Its own comment
records the price — "the child moves itself to a directory the daemon never
learns, so this session's `cwd` — and therefore the tree view, the Changes
panel and the fleet — still describe the repo root the console no longer
edits in" — and the exit: "graduating this beyond an experiment means Ralphy
creating the worktree and passing it as `cwd`". Only Claude has the flag;
Cursor and Gemini are held at "never" (ADR-0042 §flags, ADR-0043) because
a vendor-owned worktree is a tree the orchestrator cannot see.

ADR-0058 designed a worktree **per run** and was deferred: a run is an AFK
run, and a two-root `Workspace` taxes every path resolution in core and
seven adapters for a case the schedule already answers. The console case is
different in every way that made that one expensive: a console is not a
run, has no `Workspace`, no plan, no hooks, no ledger of its own, and the
daemon already resolves its `cwd` at exactly one site.

## Decision

### 1. A worktree is created, listed and removed by `ralphy worktree`

New CLI subcommands, next to `branch` in `mutate.rs`, all resolving the repo
through `git::resolve_toplevel` and — because a worktree's own toplevel is
the worktree — normalising to the **primary** tree via
`git rev-parse --git-common-dir` before anything else:

```
ralphy worktree add <name> [--base <ref>] [--repo <path>]
ralphy worktree list --format json [--repo <path>]
ralphy worktree remove <name> [--repo <path>]
```

- **`add`**: `git worktree add --no-track -b <name> .ralphy/worktrees/<name>
  <base>` (ADR-0058 §2 verbatim; `--no-track` so a name that matches a
  remote branch does not silently track it), then `git config --local
  branch.<name>.base <base>` so the base survives for a later diff. `<base>`
  defaults to the primary tree's current branch. `<name>` is a branch name
  and a directory name at once: it must pass `git check-ref-format
  --branch` and contain no path separator. Refused when the branch already
  exists or is checked out anywhere (`worktree list --porcelain` scan).
- **`list`**: `git worktree list --porcelain -z`, filtered to entries under
  `.ralphy/worktrees/`, as `{ "primary": <path>, "worktrees": [{ "name",
  "path", "branch", "base", "dirty" }] }`. Worktrees the operator made by
  hand elsewhere are not the workbench's and are not listed.
- **`remove`**: ADR-0058 §5's gates, in order, each a distinct error:
  refuse a locked worktree (`git worktree lock` is the operator's word);
  refuse a dirty one (`is_clean_ignoring_ralphy` on the worktree path);
  `git worktree remove <path>` with **no** `--force`; then `git branch -d
  <name>` — never `-D` — and if that fails because the branch has commits
  not on its base, say so and leave the branch: a removed *directory* must
  never silently delete *work*.

`add` and `remove` guard the primary tree's run lock like `branch create`
does (`mutate.rs:93`): a live run has the primary tree, and adding a
worktree under its `.ralphy/` while it runs is not something to race.
`list` is a read and does not.

The location is fixed at `<repo>/.ralphy/worktrees/<name>`. It is inside
the registered path (so the registry, the fleet key and the peer relay are
untouched — a worktree is never a second registry row), it is gitignored
(so the primary tree's `git status` and the agents' ripgrep do not see it),
and it is one `rm -rf .ralphy/worktrees` away from gone.

### 2. The daemon learns the verbs and one optional `checkout` argument

Per ADR-0036 §2, every write is a `ralphy` subcommand: `worktree.list`
(Query, `worktree list --format json`), `worktree.add` and `worktree.remove`
(Mutate; payload `name`, optional `base`; composed `worktree add -- <name>`
with the same `--` guard `branch_argv` uses, `dispatch.rs:758-783`).

Every repo-scoped verb gains an optional **`checkout: <name>`** argument.
The daemon resolves it against `worktree list` — never against a client
path — and applies it in one of two ways, by verb family:

- **Observe verbs that take a `rel`** (`tree.*`, `file.*`, `tree.watch`,
  `runs.watch`): the checkout becomes a **prefix on `rel`** —
  `.ralphy/worktrees/<name>/<rel>` — under the same registered root. The
  path confinement (`confine.rs`, ADR-0036 §5) and the tree watcher
  (`watch.rs`, keyed `(repo, rel)`, non-recursive) are untouched: a
  worktree is a subdirectory of the root they already guard and watch.
  Replies carry the operator's `rel`, not the prefixed one.
- **Git-backed verbs** (`changes.*`, `blob.read`, `branch.*`): the
  checkout becomes the **`current_dir`** the composed `ralphy` command runs
  in (`dispatch.rs:997`; the two `execute_oneshot` sites `lib.rs:2289`,
  `lib.rs:4006`), because git finds the worktree by cwd. Argv is unchanged.
  Because `branch.switch` then runs *in the worktree*, the chip switches
  that worktree's HEAD and nobody else's — the collision this ADR exists
  for.

An unknown name is `{ status: "error", message: "unknown checkout" }`.
Absent, every verb runs exactly as today.

`worktree.remove` is additionally refused by the daemon while any live
session's `checkout` (§3) names it: the session table is the daemon's, the
CLI cannot see it, and killing an agent by deleting its cwd is not a gate
the CLI's four can express.

### 3. A console is spawned in a checkout and says so

`GET /api/session?repo=…&agent=…&checkout=<name>` (`SessionQuery`,
`lib.rs:1051`) resolves the name the same way and passes the worktree path
as `cwd` to `spec_for`. `SessionInfo` (`session.rs:617`) gains
`checkout: Option<String>` — the *name*, serialised only when present — so
`/api/sessions`, the fleet and a reattach all know where a console lives.
The bridge's `session-open` frame carries it beside `name`.

`spec_for` loses its `worktree: bool` parameter; the `--worktree` argv arm
and `RepoEntry::console_worktree` are removed (an older `repos.toml` with
the key still loads — `serde` ignores it — and the daemon logs once that
the knob is retired). The vendor never creates a worktree; Ralphy did, and
the tree, Changes and diff describe the directory the console edits in.

Two vendors need one path fix each, no new code: Gemini's policy document
and owned home are resolved from `cwd` (`session.rs:270-277`); in a
checkout they resolve from the **primary** tree (the `.ralphy/` home is
there — ADR-0058 §1's observation, applied to consoles only). Cursor's
`indexing_gate` writes `.cursorindexingignore` into `cwd`; a fresh
worktree has none, so the gate runs against the worktree path and writes
its own. The other vendors need nothing: `.claude/`, `.codex/` and the
like are tracked files or user-home state.

### 4. The workbench: a Worktrees section in the branch picker, a selection on the project

- The branch picker (`app.js:771`, `#332`) gains a **Worktrees** section
  under the branch list: one row per `worktree.list` entry showing
  `<name> · <branch>` and a dirty dot, a `primary` row, and a
  `+ new worktree from <current branch>` row that takes a name (the same
  quick-pick shape as the create row at `app.js:1268`). Picking a row sets
  the project's **selected checkout**; picking `primary` clears it.
- The selected checkout is a per-project field of the desk document
  (ADR-0050; `{ windows, fences }` gains `checkouts: { <repo-ref>: <name> }`),
  so a reload and a second browser agree. The daemon stores it and does
  not validate it (a `worktree list` per desk read would be a git spawn
  per reload); the first verb that answers `unknown checkout` drops the
  selection back to `primary`.
- While a checkout is selected, the project's Files tree, Changes, diff
  and Find run with `checkout` set; the chip shows the worktree's branch
  with the worktree name beside it; **New console** spawns into it.
- A console keeps the checkout it was born in: changing the selection
  affects the next console, never a live one. The console window's title
  shows `<agent> · <checkout>` when `SessionInfo.checkout` is set, and is
  unchanged when it is not.
- The worktree row has a **remove** action that runs `worktree.remove` and
  surfaces each gate's message verbatim ("has a live console",
  "has uncommitted changes", "branch kept: has commits not on main").

With no checkout selected, no `checkout` argument is sent, no `checkout`
field is set, and every panel renders exactly as today.

### 5. Usage attribution follows the common dir

Claude keys its transcript store by launch `cwd`
(`crates/ralphy-usage-scan/src/claude.rs:35-38`, `dashed_cwd`), and Codex,
Kimi and OpenCode record a cwd per session. A console in a checkout would
attribute to `…/.ralphy/worktrees/<name>` and the Spend view would lose it.
The scan's repo matcher is amended (ADR-0033 §6) to also match a cwd that
is a linked worktree of a registered repo — read from the worktree's
`.git` **file** (`gitdir: <primary>/.git/worktrees/<name>`), the same way
the registry reads `.git/HEAD` directly (`registry.rs:77`), so the
stateless scan spawns no git and stays stateless.

### 6. What a worktree is missing, and what this ADR does not do about it

A fresh worktree has no gitignored files: no `.env`, no `.vscode/`, no
`node_modules/`. This ADR does **not** copy or link them. The row's create
action says so in one line ("gitignored files are not copied"), and a
console's first `npm install` is the operator's. ADR-0058 §4's
`worktree.copy`/`worktree.share` settings stay designed and deferred with
it; if the omission bites, that is the seam to reopen — not a vendor
setup script (ADR-0042's reason for "never" on `.cursor/worktrees.json`).

### 7. What does not change

- `ralphy run`, `BranchMode`, `Workspace`, the run lock, the hooks and all
  seven adapters. A run still takes the primary tree; a console in a
  checkout does not hold the primary run lock and is not held by it. A
  scheduled run and two console agents on one repo now coexist without a
  line of run-path code.
- The registry: one row per repo, `path` byte-identical.
- The guard: an agent still may not run `git worktree` (`guard.rs:60-63`);
  the orchestrator does it on the operator's click.
- ADR-0042/0043: the vendors' worktree flags stay off — the reason was
  always "Ralphy owns its trees", and now it does.

## Considered options — rejected

- **Keep `claude --worktree` and teach the daemon to discover the path.**
  The vendor names and places the tree; the daemon would scrape
  `worktree list` and guess which new entry is which console. Also
  Claude-only. The experiment's own comment rejects it.
- **A worktree as a second registry row.** Breaks the fleet key, the peer
  relay, usage attribution and the run lock, all keyed by the path. A
  worktree is a *view* of a repo, not a repo.
- **Sibling directory (`<repo>-<name>/`) instead of `.ralphy/worktrees/`.**
  Outside the registered path, so the daemon's path allowlist and the
  registry both need a second root; and it leaves debris beside the repo
  when the repo is deleted.
- **Auto-create a worktree per console.** The operator picks; a worktree
  per click is a branch per click and a tree of abandoned `wb-3f9c`
  branches. The picker makes the choice one click, not zero.
- **The per-run checkout of ADR-0058 first, consoles on top.** Deferred
  for the reasons in that ADR; this one needs none of its core changes.
- **Copy `.env` & co. on create.** Wants a settings key, an allowlist, a
  Windows story for links (`link_or_copy_dir` copies there) and a policy
  for stale copies — ADR-0058 §4 designed it; not this ADR's problem to
  reopen until an operator hits it.

## Consequences

- An operator opens two consoles on one project in two worktrees from the
  branch picker; each agent edits its own tree on its own branch; the
  tree, Changes and diff show the one the operator selected; a scheduled
  run takes the primary tree meanwhile. Removal is gated so no console is
  orphaned and no work is deleted.
- Three CLI subcommands, three daemon verbs, one optional verb argument
  (a `rel` prefix for Observe, a `cwd` for git-backed verbs), one
  `SessionQuery` field, one `SessionInfo` field, one desk field, one
  picker section, one usage-scan matcher amendment. One experiment
  retired. The confinement, the watcher and the registry are untouched.
- ADR-0036 §2's verb table, ADR-0032 §2, ADR-0033 §6 and ADR-0050's desk
  schema each carry a one-line amendment; ADR-0042 §flags and ADR-0043
  gain a sentence pointing here. `docs/daemon.md` documents the picker.
- Changelog: `feature` — "open each console in its own git worktree from
  the branch picker, so two agents on one project never share a tree".

## Implementation notes (not decisions)

(1) `ralphy worktree add|list|remove` with the gates, unit-tested against a
temp repo (spawn-bound: one fixture per test file, per
`docs/BUILDING.md`); (2) daemon verbs + `checkout` resolution at the two
`execute_oneshot` sites; (3) `SessionQuery.checkout` + `SessionInfo.checkout`
+ `spec_for` signature + `console_worktree` retirement + Gemini/Cursor
path fixes; (4) picker section + desk field + console title, with a
`wb-project` fold test and one `wb_worktree_*.py`; (5) usage-scan
common-dir matcher; (6) docs and amendments. (1)–(2) are green without
the UI; (4) is the first thing an operator sees. Known Windows edge: a
worktree adds `.ralphy/worktrees/<name>/` to every path; a deep
`node_modules` inside it can cross `MAX_PATH` without `core.longpaths` —
the operator's setting, named in `docs/daemon.md`, not worked around.

## Amendment (2026-09-15, #410): as built

Four places this ADR's proposed text drifted from what shipped. The console
route is `GET /ws/session?repo=…&agent=…&checkout=<name>` (§3 above says
`/api/session` — that path is the login session, unrelated to a console;
`lib.rs:732` vs `lib.rs:849`). `sync.*` joined the git-backed family
alongside `changes.*`/`blob.read`/`branch.*` (§2 lists three families; the
ADR-0036 `checkout` amendment adds the fourth), and the Write verbs
(`file.*`, `image.write`) refuse every non-null `checkout` rather than
routing into a worktree — lifting the `.ralphy` denylist for a worktree
write is a later slice (same amendment, lines 669–686). The daemon's
resolver reads the worktree's own `.git` pointer file (`gitdir: …`), never
a `worktree list` child (§3's "resolves it against `worktree list`" —
`crates/ralphy-daemon/src/checkout.rs` module header). The live-console
refusal reads `worktree '<name>' has a live console: close it first`
(`lib.rs:2279`) — §4's "has a live console" is a fragment of this line, not
the whole of it.

## Amendment (2026-09-15, #411): a console remembers its checkout across a daemon restart

§4's "a console keeps the checkout it was born in" held only while the daemon
lived: the desk record carried no checkout, so the `relaunch` verdict, the
placeholder's button and a restart without a prior `session-open` all landed
on the primary tree, silently. The window record now carries `checkout`
(ADR-0050 amendment of the same date) beside the per-repo selection — the
selection is what the *next* console opens in, the record is where *this*
console lives — and every relaunch path reads the record. A recorded worktree
that no longer exists is detected before the launch (`unknown checkout` from
an Observe read, no spawn) and shown as a placeholder naming it; the only
action offered is "relaunch in primary", by that label.

## Amendment (2026-09-15, #412): the title segment is also the control that moves a console

§3's title segment `<agent> · <checkout>` becomes a switcher on an agent
console whose repo has **at least one worktree** — a dropdown of `primary`
plus the same `worktree.list` rows the picker shows (name, branch, dirty
dot), the console's own tree marked. With no worktree the segment does not
exist, as before; a console on the primary of a repo *with* worktrees shows
`primary` so it can be switched into one. Picking another entry confirms
("restarts the session; scrollback is lost"), writes the choice into the
window's desk record (ADR-0050 #411 amendment) before any request, and runs
the window's own relaunch with the new `checkout`. The per-repo selection
(§4) is untouched in both directions: the switcher never moves it and it
never moves a live console. The plain shell console has no switcher (#408).
The listing is read once per repo with an agent console, never per render.
