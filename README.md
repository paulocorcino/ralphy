# Ralph runner — autonomous overnight issue worker (Windows)

Works GitHub issues labelled **`AFK`** unattended, onto a **single run branch**,
on your Claude **subscription quota** (no Anthropic API key, so no per-token
bill). It never pushes and never opens a PR — you review the branch and **merge
by hand** in the morning.

This is a **global tool**: it lives outside any project (e.g. `~\ralph\`) and
operates on whatever repo you point it at with **`-RepoPath`** (default: the
current directory), **in place**.

Best-of-both design, merging the [Ralph loop](https://ghuntley.com/ralph/) with a
plan-then-interactive-execute flow:

- **Plan** with `claude -p` on the stronger model (**Opus, medium effort**): it
  reads the codebase, judges complexity, and **picks the execution model**
  (`sonnet` for mechanical/localized work, `opus` for genuinely complex). Issues
  labelled `stagedplan` are planned via the **`staged-plan` skill**.
- **Execute** in **one interactive Claude session per issue** on the chosen
  model (medium effort), with **Remote Control** on so you can follow and
  intervene from the Claude mobile app (each session is named `ralph-<n>`). The
  session ends *itself* by printing `RALPH_DONE_EXIT`; a **Stop hook** flags it
  and the runner reclaims the process. `-HeadlessExec` swaps this for a
  `claude -p` loop (premium-metered; use only where there's no console/TTY).
- **One branch per run**: a fresh `afk/run-<stamp>` is cut from `-BaseBranch`
  (default `origin/main`) **in the target repo itself**, and every issue is
  committed onto it. No worktree — the warm build cache (`target/`,
  `node_modules`, …) is reused.
- **Stop at first non-green**: the moment an issue does **not** finish green
  (BLOCKED / timeout / stuck / usage limit), the whole run stops and hands you
  the branch as it stands. Completed issues stay committed; the stalled issue's
  partial commits are left in place to inspect.
- **Subscription-friendly**: no USD cap (there's no API spend). A usage limit is
  a normal **stop** — the runner reports the reset time and you re-run manually.

```
ralph.ps1 -RepoPath <repo>
  └─ precondition: target working tree is clean
     create afk/run-<stamp>  (in place, off -BaseBranch)
     for each open AFK issue, ascending #:
        PLAN     : claude -p (Opus/medium)  → .ralph/plan.md
                   · emits "## Execution model: sonnet|opus" (complexity judgment)
                   · `stagedplan`-labelled issues plan via the staged-plan skill
        EXECUTE  : claude (interactive, chosen model, +Remote Control "ralph-<n>")
                   → does every step, commits each → prints RALPH_DONE_EXIT
                   → Stop hook flags it → runner reclaims the process
        OUTCOME  : DONE       → continue to the next issue
                   non-green  → STOP the whole run, hand over the branch
     end: clean run → return repo to its original branch (run branch kept)
          stopped   → leave repo ON the run branch for inspection
```

## Files

| File | Role |
|------|------|
| `ralph.ps1` | Orchestrator: `-RepoPath`, queue, in-place run branch, plan, interactive/headless execute. |
| `prompt.plan.md` | Standard planning pass (`-p`) → `.ralph/plan.md`. |
| `prompt.plan.staged.md` | Planning pass for `stagedplan`-labelled issues — uses the `staged-plan` skill. |
| `prompt.execute.md` | The execution session's charter (copied to `.ralph/exec.md`). |
| `execplans.md` | The planning philosophy the prompts encode (observable acceptance, anchored steps, decide-and-justify). |
| `guard.ps1` | `PreToolUse` hook — destructive-command deny-list + tooling self-protection. |
| `stop_exit_hook.ps1` | `Stop` hook — writes the exit signal to the flag file. |
| `<repo>/.ralph/runs/<stamp>/` | Per-run logs + generated `ralph.settings.json`, under the **target** repo (gitignore `.ralph/`). |

The hooks are injected only into the runner's `claude` calls via `--settings`,
so your normal interactive Claude use is untouched.

## How the interactive session self-terminates

This is the crux. For each issue the runner launches `claude` in a **new console
window** (so it gets a TTY) with `--settings` pointing at a Stop hook and an env
var `RALPH_FLAG_FILE`. The initial prompt is passed as a single pre-quoted
argument string (an `-ArgumentList` array drops a multi-word prompt — only the
first word survives):

1. The agent works the plan, then prints `RALPH_DONE_EXIT` (or
   `RALPH_BLOCKED_EXIT <reason>`).
2. On its next turn-end, the **Stop hook** (`stop_exit_hook.ps1`) sees the token
   (from the payload or the transcript) and writes `DONE`/`BLOCKED …` to
   `RALPH_FLAG_FILE`. It does **not** kill anything.
3. The orchestrator polls that file (every 3s). When it appears, it kills the
   process tree (`$proc.Kill($true)`) and moves on. A `-MaxMinutesPerIssue`
   timeout and the global `-DeadlineHours` are the anti-hang backstops.

## Safeguards (unattended)

- **`guard.ps1` deny-list** (`PreToolUse`): blocks `git push`, `reset --hard`,
  `clean`, `rebase`, branch switches, `git worktree`, `gh pr merge/close`,
  recursive deletes, pipe-to-shell, and writes to secrets / `.git/` / **Ralph's
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
- The target repo: a **clean** working tree, and `.ralph/` in its `.gitignore`
  (Ralph writes scratch + logs there). A reachable `-BaseBranch` (default
  `origin/main`; the runner does a best-effort `git fetch origin` first).
- Per-project build toolchains as needed (e.g. an issue that builds an extra
  feature needs that feature's deps on `PATH`, or it will time out).

## Usage

```powershell
# 1) Plan only, one issue. No execution, no commits. Inspect .ralph/plan.md.
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -OnlyIssue 13 -DryRun

# 2) One issue, full plan + interactive execution. Follow it from the Claude
#    mobile app (session "ralph-13"). Commits onto afk/run-<stamp>.
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -OnlyIssue 13

# 3) The overnight run across the whole AFK queue (ascending order).
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -DeadlineHours 8

# -RepoPath defaults to the current directory:
cd C:\Dev\foo; pwsh -File ~\ralph\ralph.ps1 -OnlyIssue 13

# Cut the run from a different base; force a model for every issue (overrides the
# plan's judgment); disable Remote Control; or use headless -p (premium-metered):
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -BaseBranch feature/x
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -ExecModel opus
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -NoRemoteControl
pwsh -File ~\ralph\ralph.ps1 -RepoPath C:\Dev\foo -HeadlessExec
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
   `.ralph/plan.md` shape; makes no commits and removes the empty run branch.
2. `-RepoPath <repo> -OnlyIssue N` — full interactive execution onto
   `afk/run-<stamp>`. Inspect with `git -C <repo> log origin/main..afk/run-<stamp>`.
3. Only then trust the unattended queue (`-DeadlineHours 8`).

## Notes

- **Logs/scratch live in the target repo** at `<repo>/.ralph/` — both the live
  per-issue scratch (`plan.md`, `exec.md`, `issue.json`) the agent reads and the
  archived `runs/<stamp>/` logs. Add `.ralph/` to the target repo's `.gitignore`
  once.
- **One base per run.** For issues that need different bases, run twice with
  different `-BaseBranch`.
- **Usage limit = stop.** The runner reports the reset time; re-run manually
  after it. (The older auto-reschedule was dropped — it conflicts with the
  new-branch-per-run model.)
