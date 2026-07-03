# Ralphy

[![Built with Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform: Windows | Linux](https://img.shields.io/badge/platform-Windows_%7C_Linux-0078D6)](#prerequisites)
[![License: GPL v3](https://img.shields.io/badge/license-GPLv3-blue)](LICENSE)
[![Powered by Claude Code](https://img.shields.io/badge/powered_by-Claude_Code-d97757)](https://claude.com/claude-code)

**Ralphy works your GitHub issue backlog while you sleep — and hands you a branch to review in the morning.**

You label the issues you trust an agent to handle. Ralphy plans each one, has a coding
agent write the code, commits the work, and closes the issue when it's green. It **never
pushes and never opens a PR** — you review the branch and **merge by hand**. It runs on
your **coding-agent subscription** (Claude, ChatGPT/Codex, or your OpenCode provider — no
API key, so no per-token bill).

> **Scope:** Ralphy runs on **Windows and Linux** (both built and tested in CI). It
> drives one coding-agent CLI per run, picked with `--agent`:
> **[Claude Code](https://claude.com/claude-code)** (the default), **Codex**, or
> **OpenCode**.

```text
You, before bed:                          Ralphy, overnight:                 You, morning:
┌──────────────────────────┐              ┌────────────────────────┐         ┌────────────────────────┐
│ label issues you trust   │   ──────▶    │ plan → code → commit   │  ───▶   │ review the branch,     │
│ an agent to handle       │              │ → close, issue by issue│         │ merge what you like    │
└──────────────────────────┘              └────────────────────────┘         └────────────────────────┘
```

---

## Quick start

```powershell
# Windows (PowerShell)
# 1) Try one issue, plan only — no code changes, no commits. Inspect .ralphy/plan.md.
ralphy run --repo C:\Dev\foo --only-issue 13 --dry-run

# 2) Run that one issue for real. Commits land on a fresh afk/run-<stamp> branch.
ralphy run --repo C:\Dev\foo --only-issue 13

# 3) The overnight run: the whole queue, ascending order, with an 8-hour budget.
ralphy run --repo C:\Dev\foo --deadline-hours 8
```

```bash
# Linux (bash) — same flags, POSIX paths
ralphy run --repo ~/dev/foo --only-issue 13 --dry-run
ralphy run --repo ~/dev/foo --only-issue 13
ralphy run --repo ~/dev/foo --deadline-hours 8
```

`--repo` defaults to the current directory, so from inside the repo you can just run
`ralphy run --only-issue 13`. Work the same setup up incrementally: `--dry-run` one
issue, then one issue for real, and only then trust the unattended overnight queue.

## Prerequisites

- A **clean working tree** in the target repo (Ralphy refuses to start on uncommitted
  work) and a reachable base branch (default `origin/main`).
- **`gh`** authenticated — check with `gh auth status`.
- The **agent CLI** for your `--agent` choice, signed in to its subscription (no API key):
  - `claude` (default) — Claude Code CLI
  - `codex` — signed in with `codex login` (use `--agent codex`)
  - `opencode` — a provider set up with `opencode auth login` (use `--agent opencode`)
- The Ralphy binary on your `PATH` (`ralphy.exe` on Windows, `ralphy` on Linux) — see
  [docs/BUILDING.md](docs/BUILDING.md).
- Whatever build tools the issues themselves need on `PATH` (an issue that builds a
  feature needs that feature's deps, or it will time out).

Ralphy works **in place** on whatever repo you point `--repo` at — no worktree, so your
warm build cache (`target/`, `node_modules`, …) is reused.

## How it works

For every queued issue, in ascending number order:

1. **Plan** — the agent reads the codebase and writes a `.ralphy/plan.md` you can
   inspect. On Claude, the plan also picks the execution model (a small model for
   mechanical work, a strong one for complex work). Issues labelled `stagedplan` are
   planned with the bundled `staged-plan` skill.
2. **Execute** — the agent works the plan and commits each step. On Claude you can
   follow along and step in from the Claude **mobile app** (each session is named
   `ralphy-<n>`); Codex and OpenCode run quietly in the background.
3. **Verify gate** — before closing, Ralphy itself re-runs the commands the plan listed
   under `## Verify` (e.g. `cargo fmt --check`, `cargo test`) over the committed code. The
   issue only closes if they pass — "green" stops meaning *the agent said so* and starts
   meaning *the runner saw the verification pass on the code you'll merge*. Either way it
   posts a comment recording each command and its exit code. See
   [Verifying before close](#verifying-before-close).
4. **Close on green** — once the gate passes, Ralphy closes the issue with a comment
   pointing at the run branch. You still merge by hand.

If an issue **doesn't** finish cleanly (blocked, stuck, out of time, or the verify gate
fails), the whole run
**stops** and hands you the branch as it stands — so one bad issue can't burn the rest of
the night. Finished issues stay committed; the stalled one's partial work is left for
you to inspect.

## Which issues get worked

An issue is in the queue if it carries **any** queue label. The defaults are
`ready-for-agent` and its shorthand `AFK`:

| Label | What Ralphy does |
|---|---|
| `ready-for-agent` **or** `AFK` | works it, closes it when green |
| `ready-for-human` / `HITL` | never touched — not in the queue |
| `triage-agent` | evaluated by `ralphy triage`; parks the issue out of the run queue until triaged |
| `stagedplan` | planned with the `staged-plan` skill (still needs a queue label to be picked up) |

**Human-return precedence** (ADR-0016): a label that returns an issue to a human —
`ready-for-human`/`HITL`, `needs-info`, `needs-triage`, `wontfix`, or `triage-agent`
— outranks any queue label. A queued issue that also carries one is **skipped with a
visible reason** and the run continues; `--only-issue` does not override it.

Two extra controls:

- **`## Blocked by` in the issue body** — if it names an issue that's still open, Ralphy
  **skips** the issue (later ones still run) until the blocker is closed. A `## Blocked by`
  inside a `ralphy triage` consolidated-spec comment gates the queue the same way.
- **`stop-before` label** — put it on a queued issue and the run stops **before** working
  it; every earlier issue still runs. Remove it and re-run to continue. (Create the
  `stop-before` label in your repo first.)

`--queue-label` (repeatable) replaces the default label set entirely.

## Choosing an agent

`--agent` picks the CLI for the whole run (default `claude`):

| `--agent` | Runs | Notes |
|---|---|---|
| `claude` (default) | Claude Code, live session | Mobile Remote Control, model routing, auto-resume on usage limits |
| `codex` | `codex exec`, headless | Scales effort on one model; stops and reports on a usage limit |
| `opencode` | `opencode run`, headless | Fixed model; set effort with `--exec-variant`; stops and reports on a usage limit |

All three run on a **subscription, not a metered API key** — Ralphy makes sure your
subscription login stays the one in charge. The same `reviewer` and `staged-plan` skills
ship to every agent automatically, so a run never depends on what's installed on your
machine, and your global skills are left untouched.

**Split planner and executor.** `--agent` picks the executor; `--plan-agent` (default:
the `--agent` value) picks the planner, so you can plan with one agent and execute with
another. The plan is vendor-neutral markdown, so any planner's plan runs under any
executor. The canonical split is `--agent opencode --plan-agent claude` — Claude plans on
its subscription, OpenCode's coder model executes:

```powershell
ralphy run --agent opencode --plan-agent claude
```

Usage-limit handling is per-phase: a Claude planner can wait out a plan-time reset while
the OpenCode executor stops on an execute-time limit (an explicit `--stop-on-limit`
forces both phases to stop).

## Everyday flags

```powershell
ralphy run --agent codex                   # use Codex instead of Claude
ralphy run --agent opencode                # use OpenCode
ralphy run --agent opencode --plan-agent claude  # Claude plans, OpenCode executes
ralphy run --base-branch feature/x         # cut the run branch from another base
ralphy run --branch-mode current           # commit onto the current branch (no new branch)
ralphy run --exec-model opus               # force the execution model for every issue
ralphy run --exec-variant high             # OpenCode effort passthrough
ralphy run --no-remote-control             # turn off mobile Remote Control (Claude)
ralphy run --queue-label my-label          # use your own queue label
ralphy run --no-telegram                   # mute the Telegram monitor for this run
ralphy run --if-idle                       # no-op (exit 0) if a run is already active — for schedulers
```

Run `ralphy run --help` for the full list (planning model/effort, time budgets, and
more).

### Scheduled runs (`--if-idle`)

Ralphy is *the run, not the cron*: put `ralphy run --if-idle` on a timer (Windows
Task Scheduler, cron, GitHub Actions) and the queue drains on schedule. Every run
holds a presence lock (`.ralphy/run.lock`) for its lifetime; an `--if-idle`
invocation that finds a live run logs `skipped: run in progress since <time>,
pid <X>` and exits 0, so a timer never piles a run onto a live one and the
scheduler's history shows no false failures. A stale lock left by a crash or
reboot is ignored and taken over. Without the flag a live lock only warns —
intentional concurrency stays your call. Copy-pasteable recipes per platform,
each with its traps (working directory, non-interactive auth, log capture):
[docs/scheduling.md](docs/scheduling.md).

### Branch modes

`--branch-mode new` (the default) cuts a fresh `afk/run-<stamp>` branch from
`--base-branch` and commits every issue onto it, leaving your current branch untouched.
`--branch-mode current` commits straight onto the branch you're already on. Either way a
clean working tree is required. For issues that need different bases, run twice with
different `--base-branch`.

## Persistent settings

Anything you'd otherwise retype every run can be persisted per-repo in
`.ralphy/settings.json` via `ralphy config`. The resolution order is always **per-run
flag > `settings.json` > built-in default**, so a flag still wins for a one-off:

```powershell
ralphy config set opencode.model kimi-for-coding/k2p7   # OpenCode execution model default
ralphy config set base_branch origin/develop            # default base for the run branch
ralphy config set branch_mode current                   # default branch mode
ralphy config set verify.command "cargo test"           # per-repo fallback verify gate
ralphy config set claude.default_exec_model opus        # Claude run defaults (claude.*):
ralphy config set claude.max_minutes_per_issue 120      #   plan_model, plan_effort,
ralphy config set claude.max_minutes_per_issue 0        #   0 = no per-issue cap (default 90)
ralphy config get                                        #   exec_effort, … — see config --help
ralphy config unset opencode.model                      # clear a key
```

List the models an agent offers (OpenCode only — Codex/Claude have no listing command):

```powershell
ralphy models --agent opencode
```

## Morning review

```powershell
git -C C:\Dev\foo log --oneline origin/main..afk/run-<stamp>   # what landed
git -C C:\Dev\foo diff origin/main..afk/run-<stamp>            # the full diff

# happy with it?
git -C C:\Dev\foo checkout main; git -C C:\Dev\foo merge afk/run-<stamp>

# not happy? just delete the branch:
git -C C:\Dev\foo branch -D afk/run-<stamp>
```

If the run **stopped** (didn't finish green), the repo is left on the run branch so you
can fix the stalled issue in place, then commit and continue.

## Knowledge cache and consolidation

Every green close leaves a note at `.ralphy/knowledge/issue-<N>.md` with the
environment facts and working commands extracted from the issue's handoff —
future sessions read these instead of re-deriving environment procedures. The
folder grows across runs, and the same trap naturally gets re-recorded by
several issues. Periodically (end of a milestone, or when the loose notes pile
up), curate it:

```powershell
ralphy consolidate --repo C:\Dev\foo
```

A one-shot agent session (Claude) merges all loose notes into a single
`.ralphy/knowledge/KNOWLEDGE.md` — organized by topic, deduplicated, with
provenance — and the consumed notes are archived under `knowledge/raw/`.
Planner and executor sessions read `KNOWLEDGE.md` first, then any newer loose
notes. If the session fails or produces nothing, the notes stay loose for a
retry.

## Running unattended, safely

Ralphy is built to run while you sleep, so it ships its own guardrails:

- **Clean-tree precondition** — it won't start on a dirty working tree, so it never
  clobbers uncommitted work, and the agent is kept on the run branch.
- **Never pushes, never opens a PR** — the agent only commits locally. The single run
  branch is the delivery; you review and merge it by hand.
- **Time budgets** — a per-issue limit (`--max-minutes-per-issue`, default 90) and a
  global `--deadline-hours` keep a hung issue from running forever.
- **Stop at first failure** — one stalled issue stops the run instead of burning the rest
  of the budget.
- **Runner-enforced verify gate** — Ralphy re-runs the plan's `## Verify` commands itself
  before closing an issue, so an issue closes only when the runner *saw* the verification
  pass — not because the agent said it was done. See
  [Verifying before close](#verifying-before-close).
- **Command guardrails** (Claude) — destructive commands like `git push`, `reset --hard`,
  branch switches, and `gh pr merge` are blocked; recursive deletes are allowed inside the
  worktree and the system temp dir (build artifacts, e2e browser profiles) but blocked
  everywhere else. For Codex/OpenCode, safety rests on the isolated run branch and the
  built-in self-review.

## Verifying before close

For a tool that closes issues unattended overnight, "green = the agent said so" is the
central trust gap: an agent can declare *done* without the work actually being verifiable.
Ralphy closes that gap with a **runner-enforced verify gate** (ADR-0011). After the agent
reports done — but **before** the issue is closed — the runner itself re-runs a set of
commands the plan declared, over the committed code, and **only closes if they pass**.

The planner emits a `## Verify` section in `.ralphy/plan.md`, one command per line:

```markdown
## Verify

cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- **Technology-agnostic** — the gate runs whatever commands the plan names and checks
  exit codes. It knows nothing about Rust/Node/Python; the same machinery verifies
  `cargo test`, `pytest`, `npm test`, or `make check`.
- **Direct argv, no shell** — each line runs as `argv` directly (no `&&`, pipes, or
  globs), which makes `## Verify` portable Windows↔Linux for free. The runner chains the
  commands, runs them sequentially, and stops at the first non-zero exit. A command that
  truly needs a shell writes `sh -c "…"` explicitly.
- **Bounded** — the gate runs inside the per-issue time budget; a hung verification fails
  the gate rather than going green by silence.

**Pass** → the issue closes on the existing green path. **Fail** → the issue stays open,
the run stops, and the branch is handed back with the work intact. Either way, Ralphy
posts a comment recording **each command, its exit code, and (on failure) a tail of the
output** — what you read in the morning to see why an issue did or didn't close.

**Resolution precedence:**

1. `## Verify` in the plan (per-issue, planner-emitted) — strongest.
2. `verify.command` in `.ralphy/settings.json` (per-repo default) — used when a plan has
   no `## Verify` section. Set it with `ralphy config set verify.command "cargo test"`.
3. Nothing resolves → the issue closes on the agent's self-report with a **loud warning**
   in the log (the absence of a gate is always a visible decision, never a silent hole).

`## Verify: none` on its own line is the **only** explicit opt-out — for an issue with
nothing machine-verifiable — and it skips the per-repo fallback.

## Usage limits

There's no dollar cap to set — there's no API spend. On **Claude** and **Codex**, when you
hit a usage limit Ralphy **waits for the reset and resumes the same issue** automatically
(pass `--stop-on-limit` if you'd rather it stop and report). Both emit a trustworthy reset
time — Codex an absolute timestamp, Claude a relative one. **OpenCode** always stops and
reports — re-run once the limit clears.

## Cost reporting

You don't pay per token, but Ralphy still **measures** what each run consumed so you can
see how efficient a task was. Every run harvests the token counts each agent CLI already
reports and accumulates them durably per project in an append-only ledger
(`.ralphy/usage.jsonl`). The end-of-run footer shows the run total and the project's
cumulative balance as a token meter (`↑` input, `⚡` cache write, `❄` cache read,
`↓` output) plus a read-time USD estimate priced per model (`~$?` when a model has no
known price). USD is only ever a read-time projection — it never enters the ledger, so
re-pricing never rewrites history.

Read the ledger after the fact with `ralphy usage`:

```powershell
ralphy usage                       # the project balance: total tokens + estimated USD
ralphy usage --by model            # group by model (also: phase, actor, version)
ralphy usage --since 2026-06-01    # only rows on/after a date
ralphy usage --format csv          # export (also: json) instead of the table
ralphy usage --project owner/repo  # read another project's ledger
```

## Telegram run monitor (optional)

Since a run is unattended, Ralphy can post a live **status card** to a Telegram chat and
keep it updated through the whole run — planning, execution, usage-limit waits, and the
final summary — with a quick ping at the moments that matter. It's read-only; the bot
just reports. Once set up it's on by default for real runs; mute one run with
`--no-telegram`.

```powershell
ralphy telegram setup    # store the bot token, then send /start to capture your chat
ralphy telegram test     # send a ping to confirm it works
ralphy telegram status   # show the configured chat and a masked token
ralphy telegram disable  # remove the stored config
```

## Credits

- **The Ralph loop** — the unattended plan-execute-commit pattern is
  [Geoffrey Huntley](https://ghuntley.com/ralphy/)'s.
- **Triage vocabulary** — the canonical labels (`ready-for-agent`, `ready-for-human`, …)
  are **[Matt Pocock](https://github.com/mattpocock)'s**, from his
  [engineering skills](https://github.com/mattpocock/skills/tree/main/skills/engineering/setup-matt-pocock-skills).

## License

GPLv3 — see [LICENSE](LICENSE). Copyright (C) 2026 Paulo Corcino.
