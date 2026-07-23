# Choosing an agent

`--agent` picks the CLI for the whole run (default `claude`):

| `--agent` | Runs | Notes |
|---|---|---|
| `claude` (default) | Claude Code, live session | Mobile Remote Control, model routing, auto-resume on usage limits |
| `codex` | `codex exec`, headless | Scales effort on one model; stops and reports on a usage limit |
| `kimi` | `kimi -p`, headless | Fixed model (`kimi-code/k3`); stops and reports on a usage limit |
| `opencode` | `opencode run`, headless | Fixed model; set effort with `--exec-variant`; stops and reports on a usage limit |

All four run on a **subscription, not a metered API key** — Ralphy makes sure your
subscription login stays the one in charge. The same `reviewer` and `staged-plan` skills
ship to every agent automatically, so a run never depends on what's installed on your
machine, and your global skills are left untouched.

## Split planner and executor

`--agent` picks the executor; `--plan-agent` (default: the `--agent` value) picks the
planner, so you can plan with one agent and execute with another. The plan is
vendor-neutral markdown, so any planner's plan runs under any executor. The canonical split
is `--agent opencode --plan-agent claude` — Claude plans on its subscription, OpenCode's
coder model executes:

```powershell
ralphy run --agent opencode --plan-agent claude
```

Usage-limit handling is per-phase: a Claude planner can wait out a plan-time reset while
the OpenCode executor stops on an execute-time limit (an explicit `--stop-on-limit`
forces both phases to stop). See [usage limits](usage-and-cost.md#usage-limits).

## Listing models

List the models an agent offers (OpenCode only — Codex/Claude have no listing command):

```powershell
ralphy models --agent opencode
```
