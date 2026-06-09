# Ralphy

[![Built with Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows)](#prerequisites)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue)](LICENSE)
[![Powered by Claude Code](https://img.shields.io/badge/powered_by-Claude_Code-d97757)](https://claude.com/claude-code)

**Ralphy works your GitHub issue backlog while you sleep — and hands you a branch to review in the morning.**

You label the issues you trust an agent to handle. Ralphy plans each one, has Claude
write the code, commits the work, and closes the issue when it's green. It **never
pushes and never opens a PR** — you review the branch and **merge by hand**. It runs
on your **Claude subscription quota** (no Anthropic API key, so no per-token bill).

> **Scope (for now):** Ralphy is a **Windows** tool and drives **[Claude Code](https://claude.com/claude-code)**
> exclusively. Other platforms and agents aren't supported yet.

---

## What you get

At the end of a run you have **one branch** with one or more commits per issue, the
finished issues **closed** on GitHub (with a comment pointing at the branch), and your
original branch and uncommitted work **untouched**. Nothing left your machine — review
the diff, then merge whatever you like.

```text
You, before bed:                          Ralphy, overnight:                 You, morning:
┌──────────────────────────┐              ┌────────────────────────┐         ┌────────────────────────┐
│ label issues you trust   │   ──────▶    │ plan → code → commit   │  ───▶   │ review the branch,     │
│ an agent to handle       │              │ → close, issue by issue│         │ merge what you like    │
└──────────────────────────┘              └────────────────────────┘         └────────────────────────┘
```

## How it works

For every queued issue, in ascending number order, Ralphy runs a two-pass loop:

1. **Plan** — `claude -p` on the stronger model (**Opus, medium effort**) reads the
   codebase, judges the issue's complexity, and **picks the execution model**: `sonnet`
   for mechanical/localized work, `opus` for genuinely complex work. Issues labelled
   `stagedplan` are planned with the bundled **`staged-plan`** skill. Output is a
   `.ralphy/plan.md` you can inspect.
2. **Execute** — one **interactive Claude session per issue** on the chosen model. It
   works the plan, commits each step, and ends *itself* by printing a sentinel; a Stop
   hook flags it and Ralphy reclaims the process and moves on. With **Remote Control**
   on (the default) you can follow and intervene from the Claude mobile app — each
   session is named `ralphy-<n>`.
3. **Close on green** — a green queue issue is **closed** by Ralphy with a comment
   pointing at the run branch. The label is left untouched; you still merge by hand.

The moment an issue does **not** finish green (blocked / timeout / stuck), the whole run
**stops** and hands you the branch as it stands — so one bad issue can't burn the whole
queue's quota. Completed issues stay committed; the stalled issue's partial work is left
in place for you to inspect.

```text
ralphy run --repo <repo>
  └─ precondition: target working tree is clean
     create afk/run-<stamp>  (in place, off --base-branch)
     for each open queued issue, ascending #:
        PLAN     : claude -p (Opus/medium)  → .ralphy/plan.md
                   · emits "## Execution model: sonnet|opus"  (complexity judgment)
                   · `stagedplan` issues plan via the staged-plan skill
        EXECUTE  : claude (interactive, chosen model, +Remote Control "ralphy-<n>")
                   → does every step, commits each → prints the done sentinel
                   → Stop hook flags it → Ralphy reclaims the process
        OUTCOME  : green      → close the issue, on to the next
                   non-green  → STOP the whole run, hand back the branch
     end: clean run → return repo to its original branch (run branch kept)
          stopped   → leave repo ON the run branch for inspection
```

## The queue: which issues get worked

An issue is in the queue if it carries **any** queue label. The defaults follow
[Matt Pocock's canonical triage roles](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills),
with `AFK` as a shorthand synonym:

| Issue label(s) | What Ralphy does |
|---|---|
| `ready-for-agent` **or** `AFK` | works it, **closes** it when green |
| `ready-for-human` / `HITL` | **not in the queue** — never queried, never worked |
| `stagedplan` | planned via the `staged-plan` skill (still needs a queue label to be picked up) |

Two more controls shape a run:

- **`## Blocked by` in the issue body** — if an issue names a blocker that's still
  **open**, Ralphy **skips** it (no close, no stop) so later issues still run; a future
  run picks it up once the blocker is closed. See [ADR-0002](docs/adr/0002-blocked-by-gating.md).
- **`stop-before` label** — put it on one queued issue and the run stops **before**
  working it; every earlier issue still runs. Remove the label and re-run to continue.
  `--only-issue` overrides it. Use it to inspect something before the agent reaches a
  particular issue. (Create the `stop-before` label in your repo first.)

If your repo has a `docs/agents/triage-labels.md` mapping, the label it maps
`ready-for-agent` to is added to the set too. `--queue-label` (repeatable) **replaces**
the default set entirely.

## Prerequisites

- **`claude`** (Claude Code CLI), logged in to your **subscription** (no
  `ANTHROPIC_API_KEY`). Auto-located; falls back to `%USERPROFILE%\.local\bin\claude.exe`.
- **`gh`** authenticated (`gh auth status`).
- A **clean working tree** in the target repo, and a reachable `--base-branch`
  (default `origin/main`; Ralphy does a best-effort `git fetch origin` first).
- The `ralphy.exe` binary on your `PATH` — see [docs/BUILDING.md](docs/BUILDING.md).
- Per-project build toolchains as the issues need them (an issue that builds a feature
  needs that feature's deps on `PATH`, or it will time out).

Ralphy is a **global tool**: it lives outside any project and operates on whatever repo
you point it at with `--repo` (default: the current directory), **in place** — no
worktree, so the warm build cache (`target/`, `node_modules`, …) is reused.

## Usage

```powershell
# 1) Plan only, one issue. No execution, no commits. Inspect .ralphy/plan.md.
ralphy run --repo C:\Dev\foo --only-issue 13 --dry-run

# 2) One issue, full plan + interactive execution. Follow it from the Claude
#    mobile app (session "ralphy-13"). Commits onto afk/run-<stamp>.
ralphy run --repo C:\Dev\foo --only-issue 13

# 3) The overnight run across the whole queue (ascending order), 8-hour budget.
ralphy run --repo C:\Dev\foo --deadline-hours 8

# --repo defaults to the current directory:
cd C:\Dev\foo; ralphy run --only-issue 13
```

Common knobs:

```powershell
ralphy run --repo C:\Dev\foo --base-branch feature/x   # cut the run branch from another base
ralphy run --repo C:\Dev\foo --branch-mode current     # commit onto the current branch (no new branch)
ralphy run --repo C:\Dev\foo --exec-model opus         # force the exec model for every issue
ralphy run --repo C:\Dev\foo --no-remote-control       # turn off mobile Remote Control
ralphy run --repo C:\Dev\foo --headless-exec           # claude -p loop instead of a PTY (no TTY, e.g. CI)
ralphy run --repo C:\Dev\foo --queue-label my-label    # replace the default queue label set
```

Run `ralphy run --help` for the full flag list (planning model/effort, per-issue and
global wall-clock budgets, usage-limit behaviour, …).

### Morning review

```powershell
git -C C:\Dev\foo log --oneline origin/main..afk/run-<stamp>   # what landed
git -C C:\Dev\foo diff origin/main..afk/run-<stamp>            # the full diff

# happy with it?
git -C C:\Dev\foo checkout main; git -C C:\Dev\foo merge afk/run-<stamp>

# not happy? just delete the branch:
git -C C:\Dev\foo branch -D afk/run-<stamp>
```

If the run **stopped** (non-green), the repo is left checked out on the run branch so
you can fix the stalled issue in place, then commit and continue.

### Branch modes

`--branch-mode new` (the default) cuts a fresh `afk/run-<stamp>` from `--base-branch`
and commits every issue onto it, leaving your current branch untouched.
`--branch-mode current` commits straight onto the branch the repo is already on (no new
branch, `--base-branch` ignored). Either way the work happens **in the target repo
itself**, and a clean working tree is required.

> **One base per run.** For issues that need different bases, run twice with different
> `--base-branch`.

## Safeguards (it runs unattended)

Ralphy runs Claude with `--dangerously-skip-permissions`, so it ships its own guardrails:

- **Destructive-command deny-list** (`ralphy hook guard`, a `PreToolUse` hook) — blocks
  `git push`, `reset --hard`, `clean`, `rebase`, branch switches, `git worktree`,
  `gh pr merge/close`, recursive deletes, pipe-to-shell, and writes to secrets, `.git/`,
  or **Ralphy's own tooling**.
- **In-place, clean-tree precondition** — the run refuses to start on a dirty working
  tree, so it never clobbers uncommitted work. The agent is blocked from `git checkout`,
  so it stays on the run branch.
- **Time budgets** — a per-issue wall timeout (`--max-minutes-per-issue`, default 45)
  and a global `--deadline-hours` are the anti-hang backstops.
- **Never pushes, never opens a PR** — the agent only commits locally. Delivery is the
  single run branch, left for you to review and merge by hand.
- **Stop at first non-green** — one stalled issue stops the run.
- **Model routing** — planning runs execution on the smallest sufficient model
  (`sonnet` vs `opus`), saving quota.

The hooks are injected **only** into Ralphy's own `claude` calls (via `--settings`), so
your normal interactive Claude use is untouched.

## Usage limits

There's no USD cap to set — there's no API spend. On a usage limit the runner **waits
for the reset and auto-resumes the same issue** by default: it re-runs execution against
the committed history and the live `plan.md` (never `claude --resume`). A progress-aware
cap abandons an issue after two consecutive resumes that commit nothing, and a reset
landing past the run deadline stops instead of waiting. `--stop-on-limit` restores the
old stop-and-report behaviour. See [ADR-0003](docs/adr/0003-usage-limit-handling.md).

## Bundled skills

The skills the prompts depend on (`reviewer`, `staged-plan`) ship **inside the binary**
(`assets/plugin/`). Every run materializes them to the target repo's `.ralphy/plugin/`
and hands them to `claude` with `--plugin-dir`, so a run never relies on whatever skills
happen to be installed on the machine — and your global `~/.claude/skills/` is never
touched.

## Files & scratch

| Path | Role |
|------|------|
| `assets/prompts/prompt.plan.md` | Standard planning pass (`-p`) → `.ralphy/plan.md`. |
| `assets/prompts/prompt.plan.staged.md` | Planning pass for `stagedplan` issues — uses the `staged-plan` skill. |
| `assets/prompts/prompt.execute.md` | The execution session's charter (copied to `.ralphy/exec.md`). |
| `assets/plugin/` | The Claude Code plugin (the `reviewer` + `staged-plan` skills), embedded into the binary at build time. |
| `docs/execplans.md` | The planning philosophy the prompts encode. |
| `docs/adr/` | Architecture decision records (triage vocabulary, branch modes, blocked-by gating, usage-limit handling). |
| `legacy/` | The original PowerShell orchestrator (`ralphy.ps1`, `guard.ps1`, `stop_exit_hook.ps1`), superseded by the Rust binary. |
| `<repo>/.ralphy/` | Per-run logs, generated settings, and live scratch (`plan.md`, `exec.md`, `issue.json`) under the **target** repo. Auto-added to its `.gitignore` on the first run, so artifacts never leak into commits. |

## Validate a new setup incrementally

1. `--only-issue N --dry-run` — confirms plan generation and the `.ralphy/plan.md`
   shape; makes no commits and removes the empty run branch.
2. `--only-issue N` — full interactive execution onto `afk/run-<stamp>`. Inspect with
   `git -C <repo> log origin/main..afk/run-<stamp>`.
3. Only then trust the unattended queue (`--deadline-hours 8`).

## Credits

- **The Ralph loop** — the unattended plan-execute-commit pattern is
  [Geoffrey Huntley](https://ghuntley.com/ralphy/)'s.
- **Triage vocabulary** — the canonical roles (`ready-for-agent`, `ready-for-human`,
  `needs-triage`, `needs-info`, `wontfix`) are
  **[Matt Pocock](https://github.com/mattpocock)'s**, from his
  [engineering skills](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).
  See [docs/triage-roles.md](docs/triage-roles.md) and
  [ADR-0001](docs/adr/0001-triage-vocabulary-and-stop-before.md).

## License

GPLv3 — see [LICENSE](LICENSE). Copyright (C) 2026 Paulo Corcino.
