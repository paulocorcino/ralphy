# Getting started: onboarding a project

Bringing a repo from "never seen Ralphy" to "drains its queue overnight." There
are two ways in — the guided `ralphy init` command, and the manual path if you'd
rather wire it yourself. Either way the goal is the same: an issue with a queue
label that Ralphy can plan, code, verify, and close on a branch you merge by hand.

## Before you start

A repo is ready for a run when all of these hold:

- **A clean working tree** in the target repo — Ralphy refuses to start on
  uncommitted work (it commits onto a branch and won't clobber your changes).
- **A reachable base branch** — default `origin/main`.
- **`gh` authenticated** — check with `gh auth status`.
- **The agent CLI for your `--agent` choice**, signed in to its subscription
  (no API key):
  - `claude` (default) — Claude Code CLI, signed in.
  - `codex` — `codex login` (`--agent codex`).
  - `opencode` — a provider via `opencode auth login` (`--agent opencode`).
- **The `ralphy` binary on your `PATH`** (`ralphy.exe` on Windows, `ralphy` on
  Linux) — see [docs/BUILDING.md](./BUILDING.md).
- **Whatever the issues themselves need on `PATH`** — an issue that builds a
  feature needs that feature's toolchain, or its verify gate will fail.

Ralphy works **in place** on whatever `--repo` points at — no worktree — so your
warm build cache (`target/`, `node_modules/`, …) is reused.

## The guided path: `ralphy init`

`ralphy init` is the interactive onboarding command
([ADR-0012](./adr/0012-init-onboarding-command.md)). It takes an *unprepared* repo
and scaffolds everything a run needs. Rust owns every gate, git action, label, and
question; it spawns an agent session only for the read/judgment work (diagnosing
the repo, reshaping a backlog into issues), and each session gets a fully-assembled
non-interactive prompt.

```powershell
ralphy init                       # onboard the current directory
ralphy init --repo C:\Dev\foo     # onboard another repo
ralphy init --agent claude        # pick the agent that drives the judgment steps
```

What it does, in order (each stage is checkpointed to
`.ralphy/init-state.json`, so a re-run **resumes** and never duplicates issues):

1. **Environment gate** — hard-fails unless ≥1 agent CLI is present *and logged
   in* (proven by a hello-world call), `gh` is authenticated, it's a git repo with
   a GitHub `origin`, and `python` is present. A dirty tree is fine here (unlike
   `run`) — it's handled in step 4.
2. **Repo diagnosis** (agent, read-only) — scans the repo from a neutral cwd (so
   the target's `CLAUDE.md`/`AGENTS.md` are read as *data*, never auto-loaded as
   instructions) and returns a structured report: existing project vs empty,
   language/build, backlog/milestone docs, existing skill dirs, remote host.
3. **Q&A** (Rust console) — questions **pre-filled by the diagnosis**; you confirm
   or correct findings rather than answering blind.
4. **Git safety** — if the tree is dirty, it shows `git status` and asks to commit
   a snapshot (refusal aborts — a clean tree is required to keep `init`'s changes
   in a reviewable diff), then cuts a `ralphy/init` branch so nothing touches your
   main branch.
5. **Scaffold** — writes `.ralphy/` and the `docs/agents/*` config the engineering
   skills read.
6. **Skills download** — sparse-checkout of the engineering skills, pinned to the
   binary's version (warn-and-continue on failure; these are for your own use, not
   required by `ralphy run`).
7. **Labels** — creates the full vocabulary on GitHub (queue + human + the 5 triage
   labels + `stop-before`), idempotent (skips ones that exist), after listing them
   for confirmation.
8. **Backlog → issues** (agent, conditional) — turns milestone docs into a PRD +
   GitHub Milestone and a loose backlog into tracer-bullet issues. Always
   **preview-then-confirm**: the agent drafts locally, `init` summarizes ("will
   create 14 issues, 3 in milestone X"), you confirm, *then* it publishes.
9. **Checkpoint** — records each completed stage (including created issue numbers).
10. **Verify + handoff** — confirms the artifacts exist and prints your next step
    (`ralphy run --only-issue <N> --dry-run`). A real dry-run smoke test is
    *offered*, not automatic.

When `init` finishes you have labels, a scaffolded workspace, and (if you had a
backlog) queued issues — skip straight to [Your first run](#your-first-run).

## The manual path

If you'd rather not run `init` — say the repo already has issues and you just want
Ralphy to start working them — you need only two things:

1. **A queue label on GitHub.** The defaults are `ready-for-agent` and its
   shorthand `AFK`. Create one (`gh label create ready-for-agent`), or point
   Ralphy at a label you already use with `--queue-label my-label` (repeatable;
   it replaces the default set). Optionally create a `stop-before` label too, for
   flow control.
2. **That label on at least one open issue** you trust an agent to handle.

That's the whole contract: an issue is in the queue if it carries any queue label.
See [README → Which issues get worked](../README.md#which-issues-get-worked) for
the full label semantics (human-return precedence, `## Blocked by`, `stop-before`).

## Your first run

Work up to the unattended overnight queue incrementally — never trust the full
queue before you've watched one issue go end to end.

```powershell
# 1) Plan only — no code changes, no commits. Inspect .ralphy/plan.md afterwards.
ralphy run --only-issue 13 --dry-run

# 2) Run that one issue for real. Commits land on a fresh afk/run-<stamp> branch.
ralphy run --only-issue 13

# 3) The overnight run: the whole queue, ascending order, with an 8-hour budget.
ralphy run --deadline-hours 8
```

```bash
# Linux — same flags, POSIX paths
ralphy run --only-issue 13 --dry-run
ralphy run --only-issue 13
ralphy run --deadline-hours 8
```

`--repo` defaults to the current directory, so from inside the repo the flag is
optional. Read `.ralphy/plan.md` after the dry-run to see exactly what the agent
intends — including its `## Verify` gate — before you let it write code.

## The morning after

```powershell
git -C C:\Dev\foo log --oneline origin/main..afk/run-<stamp>   # what landed
git -C C:\Dev\foo diff origin/main..afk/run-<stamp>            # the full diff

# happy with it?
git -C C:\Dev\foo checkout main; git -C C:\Dev\foo merge afk/run-<stamp>

# not happy? just delete the branch:
git -C C:\Dev\foo branch -D afk/run-<stamp>
```

Ralphy **never pushes and never opens a PR** — the run branch is the delivery, and
you merge by hand. If a run **stopped** (didn't finish green), the repo is left on
the run branch so you can fix the stalled issue in place, then commit and continue.

## Next steps

- [Configuration reference](./configuration.md) — persist the defaults you'd
  otherwise retype every run (base branch, models, verify gate, …).
- [Scheduling](./scheduling.md) — put `ralphy run --if-idle` on a timer so the
  queue drains on its own.
- [Event contract](./events.md) — stream a live feed of the run to an HTTP
  endpoint (dashboards, a web platform).
- [README](../README.md) — the full tour: agents, usage limits, cost reporting,
  Telegram monitor, knowledge cache.
