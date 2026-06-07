# Ralphy runner — autonomous overnight issue worker (Windows)

Works GitHub issues labelled **`ready-for-agent`** (or **`AFK`**) unattended, on
your Claude **subscription quota** (no Anthropic API key, so no per-token bill).
It never pushes and never opens a PR — you review the branch and **merge by hand**
in the morning.

The queue vocabulary follows [Matt Pocock's canonical triage roles](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills),
with the `AFK` shorthand treated as a **synonym** for `ready-for-agent`:

| Issue labels | Ralphy does |
|---|---|
| `ready-for-agent` **or** `AFK` | works it, **closes** it when green |
| `ready-for-human` / `HITL` | **not in the queue** — never queried, never worked |
| `stop-before` (on a queue issue) | **pauses the run before it** — see below |

An issue qualifies if it carries **any** queue label. `ready-for-human` is the
canonical human-only role; Ralphy never queries it, so those issues are never
picked up. If the repo has a `docs/agents/triage-labels.md` mapping (written by
his setup skill), the label it maps `ready-for-agent` to is added to the set too.
`-QueueLabel` (a list) replaces the set entirely.

This is a **global tool**: it lives outside any project (e.g. `~\ralphy\`) and
operates on whatever repo you point it at with **`-RepoPath`** (default: the
current directory), **in place**.

Best-of-both design, merging the [Ralphy loop](https://ghuntley.com/ralphy/) with a
plan-then-interactive-execute flow:

- **Plan** with `claude -p` on the stronger model (**Opus, medium effort**): it
  reads the codebase, judges complexity, and **picks the execution model**
  (`sonnet` for mechanical/localized work, `opus` for genuinely complex). Issues
  labelled `stagedplan` are planned via the **`staged-plan` skill**.
- **Execute** in **one interactive Claude session per issue** on the chosen
  model (medium effort), with **Remote Control** on so you can follow and
  intervene from the Claude mobile app (each session is named `ralphy-<n>`). The
  session ends *itself* by printing `RALPHY_DONE_EXIT`; a **Stop hook** flags it
  and the runner reclaims the process. `-HeadlessExec` swaps this for a
  `claude -p` loop (premium-metered; use only where there's no console/TTY).
- **Branch, your choice** (`-BranchMode`): the default `new` cuts a fresh
  `afk/run-<stamp>` from `-BaseBranch` (default `origin/main`) and commits every
  issue onto it, leaving your current branch untouched. `-BranchMode current`
  commits straight onto the branch the repo is already on (no new branch,
  `-BaseBranch` ignored). Either way it works **in the target repo itself** — no
  worktree, so the warm build cache (`target/`, `node_modules`, …) is reused. A
  clean working tree is required in both modes.
- **Stop at first non-green**: the moment an issue does **not** finish green
  (BLOCKED / timeout / stuck / usage limit), the whole run stops and hands you
  the branch as it stands. Completed issues stay committed; the stalled issue's
  partial commits are left in place to inspect.
- **Closes the cycle on green**: a green queue issue is **closed** by the runner
  (with a comment pointing at the run branch — the label is left untouched; you
  still merge by hand). `-DryRun` never closes.
- **Pause mid-sequence with `stop-before`**: label one queued issue `stop-before`
  and the run stops **before** working it — every issue earlier in the sequence
  still runs. Remove the label and re-run to continue. `stop-before` is a fixed
  label (create it in your repo); `-OnlyIssue` overrides it. Use it to inspect or
  test something before the agent reaches a particular issue, without unmarking
  everything after it.
- **Subscription-friendly**: no USD cap (there's no API spend). A usage limit is
  a normal **stop** — the runner reports the reset time and you re-run manually.

```
ralphy.ps1 -RepoPath <repo>
  └─ precondition: target working tree is clean
     create afk/run-<stamp>  (in place, off -BaseBranch)
     for each open ready-for-agent issue, ascending #:
        STOP?    : issue labelled stop-before → PAUSE before it (earlier ones ran)
        PLAN     : claude -p (Opus/medium)  → .ralphy/plan.md
                   · emits "## Execution model: sonnet|opus" (complexity judgment)
                   · `stagedplan`-labelled issues plan via the staged-plan skill
        EXECUTE  : claude (interactive, chosen model, +Remote Control "ralphy-<n>")
                   → does every step, commits each → prints RALPHY_DONE_EXIT
                   → Stop hook flags it → runner reclaims the process
        OUTCOME  : DONE       → close the issue, next issue
                   non-green  → STOP the whole run, hand over the branch
     end: clean run → return repo to its original branch (run branch kept)
          stopped   → leave repo ON the run branch for inspection
```

## Files

| File | Role |
|------|------|
| `ralphy.ps1` | Orchestrator: `-RepoPath`, queue, in-place run branch, plan, interactive/headless execute. |
| `prompt.plan.md` | Standard planning pass (`-p`) → `.ralphy/plan.md`. |
| `prompt.plan.staged.md` | Planning pass for `stagedplan`-labelled issues — uses the `staged-plan` skill. |
| `prompt.execute.md` | The execution session's charter (copied to `.ralphy/exec.md`). |
| `execplans.md` | The planning philosophy the prompts encode (observable acceptance, anchored steps, decide-and-justify). |
| `guard.ps1` | `PreToolUse` hook — destructive-command deny-list + tooling self-protection. |
| `stop_exit_hook.ps1` | `Stop` hook — writes the exit signal to the flag file. |
| `<repo>/.ralphy/runs/<stamp>/` | Per-run logs + generated `ralphy.settings.json`, under the **target** repo (gitignore `.ralphy/`). |

The hooks are injected only into the runner's `claude` calls via `--settings`,
so your normal interactive Claude use is untouched.

## How the interactive session self-terminates

This is the crux. For each issue the runner launches `claude` in a **new console
window** (so it gets a TTY) with `--settings` pointing at a Stop hook and an env
var `RALPHY_FLAG_FILE`. The initial prompt is passed as a single pre-quoted
argument string (an `-ArgumentList` array drops a multi-word prompt — only the
first word survives):

1. The agent works the plan, then prints `RALPHY_DONE_EXIT` (or
   `RALPHY_BLOCKED_EXIT <reason>`).
2. On its next turn-end, the **Stop hook** (`stop_exit_hook.ps1`) sees the token
   (from the payload or the transcript) and writes `DONE`/`BLOCKED …` to
   `RALPHY_FLAG_FILE`. It does **not** kill anything.
3. The orchestrator polls that file (every 3s). When it appears, it kills the
   process tree (`$proc.Kill($true)`) and moves on. A `-MaxMinutesPerIssue`
   timeout and the global `-DeadlineHours` are the anti-hang backstops.

## Safeguards (unattended)

- **`guard.ps1` deny-list** (`PreToolUse`): blocks `git push`, `reset --hard`,
  `clean`, `rebase`, branch switches, `git worktree`, `gh pr merge/close`,
  recursive deletes, pipe-to-shell, and writes to secrets / `.git/` / **Ralphy's
  own tooling** (anchored on the tool dir's absolute path). Required because the
  run uses `--dangerously-skip-permissions`.
- **In-place, clean-tree precondition**: the run refuses to start if the target
  working tree is dirty, so it never clobbers your uncommitted work. The agent
  is blocked from `git checkout`, so it stays on the run branch.
- **Per-issue wall timeout** (`-MaxMinutesPerIssue`, default 45) + global
  **`-DeadlineHours`** (default 8).
- **Never pushes, never opens a PR**: the agent only commits locally. Delivery
  is the single run branch, left for you to review and merge by hand.
- **Stop at first non-green**: one stalled issue stops the run, so a bad issue
  can't burn the whole queue's quota.
- **Model routing**: planning judges complexity and runs execution on the
  smallest sufficient model (`sonnet` vs `opus`), saving quota.

## Prerequisites

- `claude` (Claude Code CLI), logged in to your **subscription** (no
  `ANTHROPIC_API_KEY`). Auto-located, falls back to
  `%USERPROFILE%\.local\bin\claude.exe`.
- `gh` authenticated (`gh auth status`).
- PowerShell 7 (`pwsh`).
- The target repo: a **clean** working tree. A reachable `-BaseBranch` (default
  `origin/main`; the runner does a best-effort `git fetch origin` first).
  (`.ralphy/` is auto-added to the repo's `.gitignore` on the first run.)
- Per-project build toolchains as needed (e.g. an issue that builds an extra
  feature needs that feature's deps on `PATH`, or it will time out).

## Usage

```powershell
# 1) Plan only, one issue. No execution, no commits. Inspect .ralphy/plan.md.
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -OnlyIssue 13 -DryRun

# 2) One issue, full plan + interactive execution. Follow it from the Claude
#    mobile app (session "ralphy-13"). Commits onto afk/run-<stamp>.
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -OnlyIssue 13

# 3) The overnight run across the whole ready-for-agent queue (ascending order).
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -DeadlineHours 8

# -RepoPath defaults to the current directory:
cd C:\Dev\foo; pwsh -File ~\ralphy\ralphy.ps1 -OnlyIssue 13

# Cut the run from a different base; force a model for every issue (overrides the
# plan's judgment); disable Remote Control; or use headless -p (premium-metered):
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -BaseBranch feature/x
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -ExecModel opus
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -NoRemoteControl
pwsh -File ~\ralphy\ralphy.ps1 -RepoPath C:\Dev\foo -HeadlessExec
```

Morning review (in the target repo):

```powershell
git -C C:\Dev\foo log --oneline origin/main..afk/run-<stamp>   # what landed
git -C C:\Dev\foo diff origin/main..afk/run-<stamp>            # the full diff
# happy with it?
git -C C:\Dev\foo checkout main; git -C C:\Dev\foo merge afk/run-<stamp>
# not happy? just delete the branch:
git -C C:\Dev\foo branch -D afk/run-<stamp>
```

If the run **stopped** (non-green), the repo is left checked out on the run
branch so you can fix the stalled issue in place, then commit and continue.

## Validate a new setup incrementally

1. `-RepoPath <repo> -OnlyIssue N -DryRun` — confirms plan generation and the
   `.ralphy/plan.md` shape; makes no commits and removes the empty run branch.
2. `-RepoPath <repo> -OnlyIssue N` — full interactive execution onto
   `afk/run-<stamp>`. Inspect with `git -C <repo> log origin/main..afk/run-<stamp>`.
3. Only then trust the unattended queue (`-DeadlineHours 8`).

## Notes

- **Logs/scratch live in the target repo** at `<repo>/.ralphy/` — both the live
  per-issue scratch (`plan.md`, `exec.md`, `issue.json`) the agent reads and the
  archived `runs/<stamp>/` logs. The runner auto-adds `.ralphy/` to the target
  repo's `.gitignore` on the first run, so artifacts never leak into commits.
- **One base per run.** For issues that need different bases, run twice with
  different `-BaseBranch`.
- **Usage limit = stop.** The runner reports the reset time; re-run manually
  after it. (The older auto-reschedule was dropped — it conflicts with the
  new-branch-per-run model.)

## Credits

- **Triage vocabulary** — the canonical roles (`ready-for-agent`,
  `ready-for-human`, `needs-triage`, `needs-info`, `wontfix`) are
  **[Matt Pocock](https://github.com/mattpocock)'s**, from his
  [engineering skills](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).
  Ralphy adopts them as-is; see [docs/triage-roles.md](docs/triage-roles.md) and
  [ADR-0001](docs/adr/0001-triage-vocabulary-and-stop-before.md).
- **The Ralph loop** — the unattended plan-execute-commit pattern is
  [Geoffrey Huntley](https://ghuntley.com/ralphy/)'s.

## License

GPLv3 — see [LICENSE](LICENSE). Copyright (C) 2026 Paulo Corcino.
