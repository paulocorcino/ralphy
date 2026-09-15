# A run may edit in its own checkout; the repo's `.ralphy/` home stays with the primary working tree

Status: **proposed** (2026-09-15) — decided, not yet implemented. This ADR
gates the checkout-per-run track; no code lands before it is accepted.

_Extends the run branch policy in [ADR-0002](./0002-core-agnostic-adapter-boundary.md)
(the core owns branches; adapters never do) with a third branch mode. Amends
[ADR-0033](./0033-interactive-usage-stateless-scan.md) §6 (usage attribution by path),
[ADR-0042](./0042-cursor-adapter.md) §flags and [ADR-0043](./0043-gemini-adapter.md)
(vendor worktree flags stay off — now with the reason they can), and
[ADR-0047](./0047-run-state-snapshot-channel.md) §7 (which asserts concurrent
runs in a repo are supported; this is the mechanism that makes that true).
Retires the undocumented `console_worktree` experiment in the daemon registry._

## The premise this reverses, and why it was right until now

Ralphy runs **in place**: `prepare_branch` checks out `afk/run-<stamp>` in the
one working tree the repo has, the agent edits there, and the closing matrix
returns the operator to their branch (`crates/ralphy-core/src/runner/branch.rs`,
`runner.rs:538-600`). This was never an ADR — it is the founding commit
(`43208e30`, "Ralph as a global, in-place issue runner"): in place "reuses the
warm build cache; refuses to start on a dirty working tree", and the earlier
worktree layout had only existed to keep the guard script's path stable.
`CONTEXT.md` opens with "a global, in-place runner". That reasoning holds for
one run at a time on a developer's own checkout, which is what the runner was.

Three things have since made the single working tree the limiting fact:

1. **Two runs in one repo collide.** `.ralphy/plan.md`, `issue.json`,
   `exec.md` and `run.lock` are single-slot (`types.rs:182-189, 291-297`;
   `runlock.rs:63-74` overwrites). Without `--if-idle` a second run only
   warns (`run.rs:114-120`) and then edits the same tree. Yet ADR-0047 §7
   states "Concurrent runs in a repo are a supported case (PRD #296, user
   story 9)" and designed the snapshot carrier per-document for it. The
   snapshot is ready for a second run; the working tree is not.
2. **The operator loses their checkout for the duration.** A run on
   `--branch-mode new` moves HEAD; the operator cannot work in the repo
   until it ends. That is acceptable for an AFK run and not for the
   workbench, whose consoles and Changes panel now sit next to a running run.
3. **An experiment already leaked around the gap.** `console_worktree`
   (`crates/ralphy-daemon/src/registry.rs:22-37`, `session.rs:163-204`,
   commit `7f9030a`) appends `claude --worktree` to a workbench console. Its
   own doc comment records the defect: the child "moves itself to a
   directory the daemon never learns", so the tree view, Changes and fleet
   describe a directory the console no longer edits in, and says the way out
   is "Ralphy creating the worktree and passing it as `cwd`". That is this
   ADR.

The guard hook's rule stays exactly as written: `git worktree` is "the
orchestrator's business, not the agent's" (`crates/ralphy-cli/src/guard.rs:60-63`).
This ADR gives the orchestrator that business.

## Decision

### 1. `Workspace` has two roots: the checkout and the repo home

`Workspace` today derives every path from one `repo_root`
(`crates/ralphy-core/src/types.rs:158-298`). It gains a second root, and every
`.ralphy/*` path is assigned to exactly one of them:

| Root | Meaning | Paths |
|---|---|---|
| **`checkout_root`** | the working tree the agent edits in and commits from; the agent's cwd | `issue.json`, `plan.md`, `plan-charter.md`, `exec.md`, `handoffs.md`, `references.md`, `verify-failure.md`, `protocol-failure.md`, `environment.md`, `skills/`, `plugin/`, `gemini-home/`, `cursor-config/`, `runs/<stamp>/` |
| **`repo_home`** | `<primary working tree>/.ralphy`; durable per-repo state that outlives a run | `settings.json`, `knowledge/` (incl. `KNOWLEDGE.md`, `raw/`, `citations.jsonl`), `runstate/`, `run.lock.d/` (§6), `cmd-costs.json`, `init-state.json`, `issues-draft.json`, `diagnosis.json` |

The rule for the split: **scratch a run writes for one issue follows the
checkout; anything a later run or the daemon reads stays with the primary.**
`runs/<stamp>/` is per run and therefore per checkout — but see §7 for what
the daemon still needs from it.

A hook process has no `Workspace`: `hook post` writes `cmd-costs.json` under
the hook payload's project root (`crates/ralphy-cli/src/hook.rs:149`), which
in `worktree` mode is the checkout. Hooks that write durable state resolve
`repo_home` as `<git rev-parse --git-common-dir>/../.ralphy`, which is the
primary's `.ralphy/` in a linked worktree and the same directory otherwise —
the one derivation, shared with usage-scan (§8).

In `new` and `current` branch modes both roots are the same directory. Every
existing path test (`types.rs:353-371`, `changes.rs:294-316`) keeps passing
unchanged; the two-root `Workspace` is observable only in `worktree` mode.

The core module that owns the linked checkout is `ralphy_core::checkout`.
It is not named `worktree` because `ralphy_core::worktree` already means
stage/unstage/commit/discard (`CONTEXT.md` → *Working-tree operations*), and a
second module of the same name would make prose ambiguous.

### 2. `worktree` is a third branch mode; the default does not move

`BranchMode` (`crates/ralphy-core/src/runner/branch.rs:13-16`) gains
`Worktree`. `--branch-mode worktree` and `branch_mode = "worktree"` in
`settings.json` select it. The default stays `new`: an operator who has
never read this ADR sees no change.

In `worktree` mode `prepare_branch`:

1. verifies the primary tree the same way it does today (clean ignoring
   `.ralphy/`, not detached) — a dirty primary still refuses, because the
   operator's uncommitted work is what a run must never build on top of;
2. runs `git worktree add --no-track -b afk/run-<stamp> <path> <base>`.
   `--no-track` is deliberate: the run branch must not inherit the base's
   upstream, or `sync.status` in the workbench would report it "behind" a
   remote it was never meant to track;
3. records lineage: `git config --local branch.afk/run-<stamp>.base <base>`.
   The closing matrix, `rev_list_count`, and any later "compare against
   base" verb read this instead of re-deriving the base; on failure the key
   is unset so nothing trusts a stale value;
4. runs `gitignore::ensure_ralphy_ignored` in the checkout (it lands on the
   run branch exactly as today, `branch.rs:74-80`).

`Repo` (`crates/ralphy-core/src/repo.rs`) gains `worktree_add`,
`worktree_remove`, `worktree_list` and `worktree_prune`; `GitRepo` implements
them with `git worktree …`; test fakes record them like `checkout_new_branch`.

### 3. Where the checkout lives

Default: `<repo_home>/worktrees/<stamp>` — i.e. `.ralphy/worktrees/<stamp>`
inside the primary. Two properties follow for free: the directory is already
gitignored (`.ralphy/` is), and it is on the same filesystem as the primary,
which matters because a `git worktree add` across a WSL/Windows filesystem
boundary is an order of magnitude slower and the daemon's file watcher
(ADR-0052) does not cross it. Operators who need another location set
`worktree.dir` in `settings.json`; a relative value resolves against the
primary root.

The checkout path is the run's identity on disk; the branch name
`afk/run-<stamp>` is unique by construction, so no collision handling is
needed.

### 4. Ignored-file carry-over is declared in `settings.json`

A linked worktree starts without the primary's gitignored files. Two keys,
both optional, both **warn-only** — a missing or non-ignored entry logs and
is skipped, never fails the run:

- `worktree.copy: [".env", ".vscode/"]` — literal paths that exist in the
  primary **and** are gitignored (`git check-ignore` decides); copied into
  the checkout. Secrets and editor config belong here. A copy, not a link,
  so the agent's edits cannot leak back into the primary.
- `worktree.share: ["node_modules", "target"]` — directories only, existing
  and gitignored; **symlinked** (junction first on Windows, so no Developer
  Mode is needed). Build caches belong here: copying them is slow and
  defeats the founding commit's "warm build cache" argument; sharing keeps
  it. **A share never falls back to a copy**: the existing
  `link_or_copy_dir` (`crates/ralphy-adapter-support/src/skills.rs:16-24`)
  copies on Windows when the link fails, which is right for a skills
  directory and wrong for `node_modules` — a share that cannot be linked
  is skipped with a warning, and the run proceeds without it.

No setup script. The plan phase already prices the environment
(`assets/prompts/plan/template.md` → *price the environment*) and `## Verify`
runs whatever build the plan names; a hook that runs arbitrary commands
before the agent starts is a second execution surface with no verdict.

### 5. Removal follows fixed gates, in Rust, never by the agent

At run end, in `worktree` mode:

1. **Kill the agent tree first and prove exit** (`ralphy_proc_util::kill_tree`,
   then `try_wait`). A checkout with a live child is never removed.
2. **Refuse if Git says the worktree is locked** (`git worktree list
   --porcelain` shows `locked`). A lock is an external contract; the run
   reports it and leaves the checkout.
3. **A dirty checkout is left in place and reported.** It is already a
   non-green condition (`ResultStatus::NonGreen`); removing it would destroy
   the evidence.
4. **`git worktree remove <path>` without `--force`.** If Git refuses, the
   run reports and stops there.
5. **The branch is deleted only when the run produced no commits** — the
   empty-branch case the closing matrix already handles
   (`runner.rs:528-536`) — and always with `git branch -d`, never `-D`.
   **A branch with commits is kept**, which is today's "stay on the run
   branch" semantics expressed for a checkout the operator is no longer
   standing in.
6. `git worktree prune` at the start of every run in `worktree` mode, so a
   crash that left a registration behind does not accumulate.

`--dry-run` in `worktree` mode removes the checkout and the empty branch,
mirroring `restore` (`runner.rs:583-600`).

### 6. The run lock is per checkout; `--if-idle` is per repo

`run.lock` becomes a directory of locks, `<repo_home>/run.lock.d/<runid>`,
one file per live run with the same `LockInfo` shape. `inspect` classifies
the set: `Free` when empty after stale/corrupt cleanup, `HeldAlive` when any
entry names a live pid.

`--if-idle` keeps its meaning — *skip when any run is live in this repo* —
because its caller is a scheduler, and a scheduler must not pile a run onto
a live one whichever checkout it would use. An explicit `ralphy run
--branch-mode worktree` without `--if-idle` proceeds alongside a live run;
in `new`/`current` mode it warns and proceeds exactly as today, since those
modes still share the primary tree.

Two readers, two sources, unchanged in kind: the CLI's `guard_run_lock`
(`runlock.rs:101-113`, behind every Mutate subcommand) inspects the set and
refuses while any entry is alive in the tree it is about to mutate; the
workbench's `writeLocked()` does not read the lock at all — it folds the
run-snapshot list (`app.js:1030`, `wb-changes.js:255`) — and learns the
distinction from the snapshot's `checkout` field (§7): the Changes panel on
the primary is locked while a live run has no `checkout` (it is editing the
primary), and unlocked when every live run reports one.

### 7. The daemon observes checkouts; it never creates them

- `RepoEntry` gains a derived, read-only `checkouts: Vec<Checkout {path,
  branch, runid?}>` from `git worktree list --porcelain` under the registry
  path — bytes, not interpretation (ADR-0036 §3). `head_branch`
  (`registry.rs:70-80`) learns to follow a gitdir-pointer `.git` file
  instead of returning `None`.
- The run snapshot (ADR-0047) gains an additive `checkout: Option<String>`
  (v stays 1, §6). `runs.list` is unchanged: `runstate/` lives in
  `repo_home`, which the daemon already watches.
- `tree.list`, `tree.find`, `tree.grep`, `file.read`, `blob.read`,
  `changes.list` accept an optional `checkout` argument naming one of the
  registry's known checkouts; confinement (`confine.rs`) admits exactly
  those paths. The workbench uses it to point the tree and Changes at the
  checkout a run or console is in. Mutate verbs (`changes.stage` …) take it
  too, still run-lock-aware per §6.
- The Spawn verb `run` gains `--branch-mode worktree` pass-through and
  nothing else. The daemon does not run `git worktree`; a checkout exists
  because a run created it.

`console_worktree` is retired. A console that should edit in isolation is
opened with `cwd = <checkout>` of a run, or of a checkout the operator
created with `ralphy branch create --worktree` (a new flag on the existing
run-lock-aware subcommand, ADR-0036 §6), so the daemon always knows the cwd
it launched. The ADR-0042/0043 rows that keep vendor worktree flags at
"never" stay, now with the reason they can: Ralphy owns the checkout.

### 8. State keyed on the cwd string follows the checkout

Three places key vendor state on the byte-exact cwd: the Claude transcript
directory `~/.claude/projects/<dashed-cwd>` (`crates/ralphy-agent-claude/src/usage.rs:15-25`),
workspace trust in `~/.claude.json` (`interactive.rs:491-506`), and usage-scan
attribution (`crates/ralphy-usage-scan/src/claude.rs:35-38`). All three
resolve through `checkout_root`. Usage-scan additionally attributes a cwd
that is a linked worktree to the registry slug of its primary by reading
`git rev-parse --git-common-dir` — ADR-0033 §6 amended accordingly — so the
Spend view does not show a run's tokens under an unknown project. This is
one change in `ralphy-usage-scan`, and it covers every vendor at once: all
seven session stores record the cwd (Codex `session_meta.cwd`, Copilot
`sessions.cwd`, OpenCode `session.directory`, Gemini `.project_root`, Cursor
and Kimi by path segment), and a linked worktree is a new cwd in each.

No adapter needs worktree-specific code. Every adapter already takes its cwd
from `ws.repo_root()` and every per-run path it writes is relative to
`.ralphy/` (`skills/`, `plugin/`, `gemini-home/`, `cursor-config/`,
`runs/<stamp>/`), so they follow the checkout by construction. The one
exception is Cursor's `indexing_gate`, which writes `.cursorindexingignore`
into every enclosing repo root: in a linked worktree it must write into the
checkout as well, or the vendor indexes the run's tree. Vendor worktree
flags (`claude --worktree`, Cursor `-w`/`--worktree-base`, Gemini
`experimental.worktrees`) stay off exactly as ADR-0042/0043 hold them; the
guard forbids the agent from running `git worktree` itself.

`claude --name` is orthogonal: it names the session for the shell
(`session.rs` `claude_console_named`) and says nothing about directories. It
keeps working in any cwd, including a checkout.

### 9. What does not change

- The `Agent` trait and every adapter signature: adapters already receive
  `ws: &Workspace` and use `ws.repo_root()` as cwd; that accessor now returns
  `checkout_root`. A new `ws.repo_home()` exists for the durable paths, and
  the split in §1 is enforced by which accessor each path method uses.
- The guard: the agent may not run `git worktree`, `checkout` or `switch`.
- ADR-0003/0030 limit handling, ADR-0011 verify-gate repair ("fixable in
  place" — the place is now the checkout), ADR-0023 outcome ladder.
- Push: Ralphy still never pushes; `push.autoSetupRemote` and any upstream
  configuration are explicitly not set on the run branch.

## Considered options — rejected

- **Keep in place; document `--if-idle` as the concurrency answer.** That is
  the status quo and it contradicts ADR-0047 §7 in writing. It also leaves
  `console_worktree` as the only isolation story, with the cwd defect its
  own comment describes.
- **Worktree always (drop `new`).** The founding commit's warm-cache and
  "refuse on dirty tree" arguments still apply to the single-developer AFK
  case, and every existing test pins `new`. A mode, not a replacement.
- **Let the vendor create the worktree (`claude --worktree`, Cursor's
  `-w`).** Already tried by `console_worktree`; the daemon never learns the
  path, so nothing above the agent can observe or clean it. ADR-0042/0043
  keep those flags off for that reason.
- **Checkout under `~/.ralphy/worktrees/<slug>/…` (outside the repo).**
  Crosses filesystems on WSL, escapes the daemon's watcher root, and needs a
  second registry key. Inside `.ralphy/` costs nothing and inherits the
  gitignore.
- **A setup script per checkout.** A second execution surface with no
  verdict; the plan phase and `## Verify` already own environment truth.
- **`git branch -D` on removal.** Deleting a worktree must never silently
  discard commits; the only branch Ralphy deletes is one it can prove empty.
- **Prepared/pre-warmed checkouts.** A latency optimisation for an
  interactive click; a batch run does not care about the seconds
  `worktree add` takes.
- **A dotfile (`.worktreeinclude`) for carry-over.** A second config surface
  next to `settings.json`; the keys in §4 say the same thing where every
  other run setting already lives.

## Consequences

- ADR-0047 §7's "concurrent runs in a repo are a supported case" becomes
  true in `worktree` mode; in `new`/`current` mode it remains what it was — a
  warning.
- The workbench can show a run's tree and diff while the operator keeps
  their own branch. The Changes panel's lock (§6) is finer-grained.
- Per-run scratch that used to be at `.ralphy/plan.md` is at
  `.ralphy/worktrees/<stamp>/.ralphy/plan.md` in `worktree` mode. Every
  consumer goes through `Workspace` accessors, so no path is hand-built —
  the split in §1 is the audit list.
- CONTEXT.md gains **Checkout**: "the working tree a run edits in; the
  primary in `new`/`current` mode, a linked worktree under
  `.ralphy/worktrees/<stamp>` in `worktree` mode. The repo home is the
  primary's `.ralphy/` regardless of mode."
- ADR-0033 §6, ADR-0042 §flags, ADR-0043 and ADR-0047 §7 carry a one-line
  amendment pointing here. `console_worktree` is removed from `repos.toml`
  handling with a `fix` changelog fragment ("a console opened in isolation
  is one the workbench can see").
- The claim in CLAUDE.md that runs are in place is unchanged as the default
  and gains the mode.

## Implementation notes (not decisions)

Ordered so each step is green on its own: (1) two-root `Workspace` with both
roots equal, path methods reassigned per §1 — no behaviour change; (2)
`Repo::worktree_*` + `BranchMode::Worktree` + `prepare_branch`/closing matrix;
(3) `run.lock.d/`; (4) `worktree.copy`/`worktree.share`; (5) cwd-keyed state
(§8) and the usage-scan amendment; (6) daemon `checkouts`, `checkout`
argument on verbs, snapshot field, `console_worktree` retired; (7) CONTEXT.md
and the ADR amendments. Each step is one issue.
