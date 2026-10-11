# Choosing an agent

`--agent` picks the CLI for the whole run (default `claude`):

| `--agent` | Runs | Notes |
|---|---|---|
| `claude` (default) | Claude Code, live session | Mobile Remote Control, model routing |
| `codex` | `codex exec`, headless | Scales effort on one model |
| `kimi` | `kimi -p`, headless | Fixed model (`kimi-code/k3`) |
| `opencode` | `opencode run`, headless | Fixed model; set effort with `--exec-variant` |
| `copilot` | `copilot`, headless | Checks which models your account can use before the run starts |
| `cursor` | `cursor-agent`, headless | Default model `auto`; a `--model` never changes the default of your own Cursor sessions; turns off Cursor's codebase upload in the repo first, and tells you |
| `gemini` | `gemini`, headless | Default model `auto`; uses its own settings folder, so your `~/.gemini` is never read or changed |

All seven use the login you already have in that CLI, **not a metered API key** — Ralphy
makes sure that login stays the one in charge. The same `reviewer` and `staged-plan` skills
ship to every agent automatically, so a run never depends on what's installed on your
machine, and your global skills are left untouched.

On a usage limit, every agent waits and then resumes the same issue, unless you pass
`--stop-on-limit`. Claude and Codex wait for the reset time they report; the others retry
about every 30 minutes. See [usage limits](usage-and-cost.md#usage-limits).

## Split planner and executor

`--agent` picks the executor; `--plan-agent` (default: the `--agent` value) picks the
planner, so you can plan with one agent and execute with another. The plan is
vendor-neutral markdown, so any planner's plan runs under any executor. The canonical split
is `--agent opencode --plan-agent claude` — Claude plans on its subscription, OpenCode's
coder model executes:

```powershell
ralphy run --agent opencode --plan-agent claude
```

Each phase handles its own usage limit: a Claude planner waits for a plan-time reset, and
an OpenCode executor waits for an execute-time limit. `--stop-on-limit` makes both phases
stop. See [usage limits](usage-and-cost.md#usage-limits).

## Listing models

List the models an agent offers (OpenCode only — Codex/Claude have no listing command):

```powershell
ralphy models --agent opencode
```
