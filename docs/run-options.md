# `ralphy run` — options reference

`ralphy run` works the repo's issue queue onto a branch. This page covers the flags you'll
reach for beyond the everyday `ralphy run --agent <agent> --branch-mode <current|new>`. Run
`ralphy run --help` for the authoritative, always-current list.

## Picking what to work

| Flag | What it does |
|---|---|
| *(none)* | Work the whole queue — every open issue carrying a queue label — in ascending number order. |
| `--only-issue <n>` | Work just that one issue. Great for a first trial. |
| `--issues 5,3,9` | Work exactly these issues, **in the order given**, ignoring queue labels and dependency ordering. Drains the list as a sequence. Mutually exclusive with `--only-issue`. |
| `--queue-label <label>` | Replace the default queue labels (`ready-for-agent`, `AFK`) entirely. Repeatable. |
| `--assignee <login>` | Only queue issues this login is assigned to (`@me` = you). `--no-assignee` disables a persisted filter for one run. |

Two controls live in the **issues themselves**, not on the command line:

- **`## Blocked by` in the issue body** — if it names an issue that's still open, Ralphy
  skips this one (later issues still run) until the blocker closes.
- **`stop-before` label** — put it on a queued issue and the run stops *right before* it;
  every earlier issue still runs. Remove it and re-run to continue.

A label that hands an issue back to a human (`ready-for-human`/`HITL`, `needs-info`,
`needs-triage`, `wontfix`, `triage-agent`) always wins, even over `--only-issue`/`--issues`.
See [ADR-0016](adr/0016-queue-label-precedence.md).

## Choosing the agent

| Flag | What it does |
|---|---|
| `--agent <agent>` | Who executes the run: `claude` (default), `codex`, `opencode`, and more. |
| `--plan-agent <agent>` | Who *plans* — defaults to `--agent`. Lets you plan with one agent and execute with another (e.g. `--agent opencode --plan-agent claude`). |

Full detail, including the split planner/executor and per-agent notes: [agents.md](agents.md).

## Where commits land

| Flag | What it does |
|---|---|
| `--branch-mode new` | *(default)* Cut a fresh `afk/run-<stamp>` branch off the base and commit there, leaving your current branch untouched. |
| `--branch-mode current` | Commit straight onto the branch you're already on (no new branch). |
| `--base-branch <ref>` | The commit-ish a `new` run branch is cut from (default `origin/main`). Ignored by `--branch-mode current`. |

Either mode requires a clean working tree. For issues that need different bases, run twice
with different `--base-branch`.

## Time and safety budgets

| Flag | What it does |
|---|---|
| `--dry-run` | Plan only — no source changes, no commits. Inspect `.ralphy/plan.md` afterwards. |
| `--deadline-hours <h>` | Global wall-clock budget: don't start a new issue past it. |
| `--max-minutes-per-issue <m>` | Per-issue cap (default: no cap; `0` disables). |
| `--idle-minutes <m>` | Reap a child that's made no progress for this long — catches a wedged agent. `0` disables. |
| `--stop-on-limit` | On a usage limit, stop and report instead of waiting for the reset and resuming. See [usage-and-cost.md](usage-and-cost.md#usage-limits). |
| `--if-idle` | Skip this invocation (exit 0) if a run is already active — the anti-overlap flag for schedulers. |

## Models and effort

| Flag | What it does |
|---|---|
| `--plan-model <m>` / `--exec-model <m>` | Force the planning / execution model (defaults come from the agent or `settings.json`). |
| `--plan-effort <e>` / `--exec-effort <e>` | Vendor-neutral reasoning effort. |
| `--exec-variant <v>` | OpenCode effort passthrough (`opencode run --variant`). |
| `--default-exec-model <m>` | Execution model used when the plan emits no complexity judgment. |

## Notifications and output

| Flag | What it does |
|---|---|
| `--no-telegram` | Mute the Telegram monitor for this run. See [telegram.md](telegram.md). |
| `--title <text>` | Override the auto-derived Telegram card title. |
| `--remote-control` / `--no-remote-control` | Turn Claude's mobile Remote Control on/off for this run (off by default). |
| `--verbose` | Print raw `tracing` INFO lines instead of the animated presenter (useful in CI). |

## Anything you'd retype every run

Persist it per-repo with `ralphy config set …` so you can drop the flag. Resolution order is
always **flag > `settings.json` > built-in default**. See
[configuration.md](configuration.md).
