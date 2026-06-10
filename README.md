# Ralphy

[![Built with Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform: Windows](https://img.shields.io/badge/platform-Windows-0078D6?logo=windows)](#prerequisites)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue)](LICENSE)
[![Powered by Claude Code](https://img.shields.io/badge/powered_by-Claude_Code-d97757)](https://claude.com/claude-code)

**Ralphy works your GitHub issue backlog while you sleep — and hands you a branch to review in the morning.**

You label the issues you trust an agent to handle. Ralphy plans each one, has a
coding agent write the code, commits the work, and closes the issue when it's green.
It **never pushes and never opens a PR** — you review the branch and **merge by hand**.
It runs on your **coding-agent subscription** (Claude, ChatGPT/Codex, or your OpenCode
provider — no API key, so no per-token bill).

> **Scope (for now):** Ralphy is a **Windows** tool. It drives one coding-agent CLI
> per run, selected with `--agent`: **[Claude Code](https://claude.com/claude-code)**
> (the default), **Codex** (`codex exec`), or **OpenCode** (`opencode run`). Other
> platforms aren't supported yet.

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

1. **Plan** — a planning pass (the default Claude agent uses `claude -p` on **Opus,
   medium effort**) reads the codebase, judges the issue's complexity, and writes a
   `.ralphy/plan.md` you can inspect. On Claude the plan also **picks the execution
   model** (`sonnet` for mechanical/localized work, `opus` for complex work); other
   agents express complexity differently (see [Agents](#agents)). Issues labelled
   `stagedplan` are planned with the bundled **`staged-plan`** skill.
2. **Execute** — one execution session per issue on the chosen agent. It works the
   plan, commits each step, and ends *itself* by printing a done sentinel; Ralphy
   detects that, reclaims the process, and moves on. The default Claude agent runs an
   **interactive PTY session** ended by a Stop hook, and with **Remote Control** on
   (the default) you can follow and intervene from the Claude mobile app — each session
   is named `ralphy-<n>`. Codex and OpenCode run **headless** and detect completion
   from their own output.
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

## Agents

One agent CLI drives a whole run, chosen with `--agent` (default `claude`). The core
never learns which vendor it holds — each adapter is an isolated crate that maps its
vendor's signals onto the same outcome the queue understands ([ADR-0002](docs/adr/0002-core-agnostic-adapter-boundary.md)).

| `--agent` | CLI | Session | Complexity routing | Auth |
|---|---|---|---|---|
| `claude` (default) | `claude` | Interactive PTY, Stop hook, mobile Remote Control | Plan picks `sonnet` vs `opus` | Claude subscription |
| `codex` | `codex exec` | Headless, native completion signals | Reasoning effort on one model (low/medium/high) | ChatGPT subscription (`codex login`) |
| `opencode` | `opencode run` | Headless, `--format json` event stream | Deterministic — fixed model, optional `--exec-variant` | Your OpenCode provider (`opencode auth login`) |

All three run on a **subscription, not a metered API key** — Ralphy clears
`ANTHROPIC_API_KEY`, and the Codex/OpenCode adapters scrub the provider keys their CLIs
would otherwise auto-detect, so your subscription auth stays authoritative. The same
`reviewer` + `staged-plan` skills and the same execution charter ship to every agent;
only the delivery format differs (Claude plugin vs. discovered `SKILL.md` skills). See
[ADR-0004](docs/adr/0004-codex-adapter.md) (Codex) and
[ADR-0005](docs/adr/0005-opencode-adapter.md) (OpenCode).

> **Codex/OpenCode never auto-resume on a usage limit** — their resets aren't reliably
> parseable, so `--stop-on-limit` is forced for both (see [Usage limits](#usage-limits)).
> Only the Claude path can wait-and-resume.

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

- **The agent CLI for your `--agent` choice**, logged in to its **subscription**
  (no API key):
  - `claude` (default) — Claude Code CLI. Auto-located; falls back to
    `%USERPROFILE%\.local\bin\claude.exe`.
  - `codex` — signed in via `codex login` (`--agent codex`).
  - `opencode` — a provider configured via `opencode auth login` (`--agent opencode`).
- **`gh`** authenticated (`gh auth status`).
- *(optional)* a **Telegram bot** if you want the live run monitor — see
  [Telegram run monitor](#telegram-run-monitor).
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
ralphy run --repo C:\Dev\foo --agent codex             # drive Codex (codex exec) instead of Claude
ralphy run --repo C:\Dev\foo --agent opencode          # drive OpenCode (opencode run)
ralphy run --repo C:\Dev\foo --base-branch feature/x   # cut the run branch from another base
ralphy run --repo C:\Dev\foo --branch-mode current     # commit onto the current branch (no new branch)
ralphy run --repo C:\Dev\foo --exec-model opus         # force the exec model for every issue
ralphy run --repo C:\Dev\foo --exec-variant high       # OpenCode --variant (effort) passthrough
ralphy run --repo C:\Dev\foo --no-remote-control       # turn off mobile Remote Control (Claude)
ralphy run --repo C:\Dev\foo --headless-exec           # claude -p loop instead of a PTY (no TTY, e.g. CI)
ralphy run --repo C:\Dev\foo --queue-label my-label    # replace the default queue label set
ralphy run --repo C:\Dev\foo --no-telegram             # mute the Telegram monitor for this run
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

Ralphy runs the agent with full autonomy (Claude/OpenCode `--dangerously-skip-permissions`,
Codex `-s danger-full-access -a never`), so it ships its own guardrails:

- **Destructive-command deny-list** (`ralphy hook guard`, a `PreToolUse` hook) — blocks
  `git push`, `reset --hard`, `clean`, `rebase`, branch switches, `git worktree`,
  `gh pr merge/close`, recursive deletes, pipe-to-shell, and writes to secrets, `.git/`,
  or **Ralphy's own tooling**. This hook is **Claude-only** (Codex/OpenCode have no
  equivalent hook); for those agents safety rests on the isolated run branch and the
  reviewer self-review.
- **In-place, clean-tree precondition** — the run refuses to start on a dirty working
  tree, so it never clobbers uncommitted work. The agent is blocked from `git checkout`,
  so it stays on the run branch.
- **Time budgets** — a per-issue wall timeout (`--max-minutes-per-issue`, default 45)
  and a global `--deadline-hours` are the anti-hang backstops.
- **Never pushes, never opens a PR** — the agent only commits locally. Delivery is the
  single run branch, left for you to review and merge by hand.
- **Stop at first non-green** — one stalled issue stops the run.
- **Model routing** (Claude) — planning runs execution on the smallest sufficient model
  (`sonnet` vs `opus`), saving quota. Codex scales by reasoning effort; OpenCode is
  deterministic (see [Agents](#agents)).

The Claude hooks are injected **only** into Ralphy's own `claude` calls (via
`--settings`), so your normal interactive Claude use is untouched.

## Usage limits

There's no USD cap to set — there's no API spend. On the **Claude** agent, the runner
**waits for the reset and auto-resumes the same issue** by default: it re-runs execution
against the committed history and the live `plan.md` (never `claude --resume`). A
progress-aware cap abandons an issue after two consecutive resumes that commit nothing,
and a reset landing past the run deadline stops instead of waiting. `--stop-on-limit`
restores the old stop-and-report behaviour. See
[ADR-0003](docs/adr/0003-usage-limit-handling.md).

For **Codex** and **OpenCode**, `--stop-on-limit` is **forced** — their resets aren't
reliably parseable (Codex's rolling window, OpenCode's silent in-CLI backoff), so
auto-resume would risk an hours-long hang. They stop and report the limit for you to
re-run after it clears.

## Telegram run monitor

Because a run is unattended and the agent's output is teed to log files (never your
terminal), Ralphy can post a live **status card** to a Telegram chat and edit it in
place across the whole run — planning, per-issue execution, usage-limit sleeps, stops,
and the final summary — plus a short push at the milestones that matter (start, sleeping
on a limit, resuming, final outcome). It's **read-only**: the bot reports, it accepts no
commands. Once configured it's **on by default** for every real run (dry runs never
notify); mute a single run with `--no-telegram`. See
[ADR-0007](docs/adr/0007-telegram-notifier.md).

```powershell
ralphy telegram setup   # store the bot token, then send /start to capture your chat
ralphy telegram test    # send a ping to confirm token + chat
ralphy telegram status  # show the configured chat and a masked token
ralphy telegram disable  # remove the stored config
```

The token can also come from `RALPHY_TELEGRAM_TOKEN` (overrides the stored one). Config
lives in a permission-restricted global file (`%APPDATA%\ralphy\` on Windows).

## Bundled skills

The skills the prompts depend on (`reviewer`, `staged-plan`) ship **inside the binary**
(`assets/plugin/`). Every run materializes them under the target repo's `.ralphy/`, in
whatever form the chosen agent discovers: Claude gets a plugin dir
(`.ralphy/plugin/`, passed with `--plugin-dir`), Codex gets `.agents/skills/`, and
OpenCode gets `.ralphy/skills/` pointed at via injected `skills.paths`. Either way a run
never relies on whatever skills happen to be installed on the machine — and your global
`~/.claude/skills/` is never touched.

## Files & scratch

| Path | Role |
|------|------|
| `assets/prompts/prompt.plan.md` | Standard planning pass (`-p`) → `.ralphy/plan.md`. |
| `assets/prompts/prompt.plan.staged.md` | Planning pass for `stagedplan` issues — uses the `staged-plan` skill. |
| `assets/prompts/prompt.plan.opencode.md` | OpenCode planning variant (no `## Execution model` tier line; OpenCode-neutral reviewer step). |
| `assets/prompts/prompt.execute.md` | The execution session's charter, reused verbatim across all agents (copied to `.ralphy/exec.md`). |
| `assets/plugin/` | The `reviewer` + `staged-plan` skills, embedded into the binary at build time and re-targeted per agent (plugin / `.agents/skills` / `.ralphy/skills`). |
| `crates/` | The Rust workspace: `ralphy-core`, `ralphy-cli`, `ralphy-pty`, the per-agent adapters (`ralphy-agent-claude`/`-codex`/`-opencode`), and shared `ralphy-adapter-support`. |
| `docs/execplans.md` | The planning philosophy the prompts encode. |
| `docs/adr/` | Architecture decision records (triage vocabulary, branch modes, blocked-by gating, usage-limit handling, the core-agnostic adapter boundary, the Codex/OpenCode adapters, the console UI, and the Telegram notifier). |
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
