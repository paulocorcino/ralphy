# A prompt file is a fifth one-shot; `schedule` can time it; a precheck can veto it

Status: **proposed** (2026-09-15) — decided, **not scheduled**: no issue is
open until an operator asks for a timed prompt. Fifth and
last in the track opened by [ADR-0058](./0058-checkout-per-run.md);
independent of the other four.

_Extends [ADR-0026](./0026-native-scheduling-command.md) §2 (the targets
`schedule install` blesses) and [ADR-0031](./0031-consolidation-dispatches-on-the-selected-agent.md)
D1 (the one-shots every adapter carries as free functions). Amends the
[ADR-0040](./0040-agent-adapter-onboarding-contract.md) Tier 1 `tasks.rs`
row and the [ADR-0008](./0008-token-usage-tracking.md) D6 phase set. Keeps
ADR-0026 §1 exactly: the OS timer is the scheduler; Ralphy never is._

## The gap between the four one-shots and "run this on a cadence"

Every adapter carries four one-shot sessions as free functions —
`diagnose_repo`, `draft_issues`, `triage_issues`, `consolidate_knowledge`
(`crates/ralphy-agent-*/src/tasks.rs`; ADR-0031 D1; ADR-0040 Tier 1) —
each with a fixed charter from `assets/prompts` and a fixed artifact or
validator, dispatched by a hand-written `match Agent` in the CLI. And
`ralphy schedule install <run|triage>` registers an OS timer that invokes
`ralphy run --if-idle` or `ralphy triage --if-idle --yes`
(`crates/ralphy-cli/src/schedule/spec.rs:73-92`).

There is no way to say *"every Monday, with Codex, read this prompt and
tell me what it finds"*. The pieces exist — `run_text_session`
(`crates/ralphy-adapter-support/src/json_session.rs:165`) drives a prompt
to a log; the timer machinery takes a target and an interval — but no
target carries an operator-written prompt, and no CLI verb runs one. The
operator writes a shell script around `claude -p`, outside the ledger,
outside the guard, outside the run lock.

## Decision

### 1. `ralphy prompt run`: a one-shot whose charter is the operator's file

New subcommand:

```
ralphy prompt run --file <path> --agent <a> [--model M] [--effort E]
                  [--max-minutes N] [--repo <path>] [--precheck "<cmd>"]
```

It runs a **fifth one-shot**, `run_prompt`, on every adapter — one more
free function next to the four (ADR-0031 D1; ADR-0040 Tier 1 `tasks.rs`
row grows by one) — over `run_text_session` (Gemini through its exit-code
ladder, as its other one-shots do). The prompt file's bytes are the
session's prompt; the session's cwd is the repo root; the output is the
session log, printed to stdout and kept at `.ralphy/runs/<stamp>/prompt.log`.
Model and effort default to the adapter's own defaults, the same way
`consolidate` resolves them; `--max-minutes` defaults to 30.

It writes a ledger line with phase `prompt` (ADR-0008 D6's closed set
`plan | execute | protocol-repair | repair | consolidate` gains one), so the
Spend view accounts for it like any other session.

**It runs under the guard settings, not the hook-less ones.** The four
existing one-shots write `SETTINGS_JSON` without hooks
(`crates/ralphy-agent-claude/src/tasks.rs:45,110`) because their charters
are read-only by prompt. An operator's prompt is not; a timer-fired session
that can `git push` because nobody was watching is the one thing this ADR
must not create. Claude gets the `PreToolUse` guard; other vendors get what
their adapter has (ADR-0002: a capability, not a guarantee) and the prompt
docs say so.

`--precheck` is §3.

### 2. `schedule install prompt`: the third blessed target

```
ralphy schedule install prompt --file <path> --agent <a> --every 1d
                               [--name <n>] [--precheck "<cmd>"] [--repo]
```

`Target` (`spec.rs:73`) gains `Prompt { file, agent, name, precheck }`.
Its invocation is `ralphy prompt run --file <abs> --agent <a> --if-idle
[--precheck …]` — `--if-idle` so a prompt never lands on top of a live run,
the same anti-overlap the other two targets pass.

The task name and the crontab tag are keyed `<slug>:<repo>` today
(`platform.rs:27-40`), one per target per repo. For `prompt` the slug
becomes `prompt-<name>`, where `--name` defaults to the file's stem, so
several prompts on one repo coexist and `schedule status`/`remove` address
each by name. `run` and `triage` tags are byte-identical to today.

The prompt file is stored as an **absolute path** in the timer and must be
inside the repo or under `~/.ralphy/prompts/`; anything else is refused at
install. A timer must not be a way to execute an arbitrary file that
appears later at an arbitrary path.

ADR-0026 §2 stays true — `schedule install` still "owns the blessed
default, not the full space of timer configurations" — with three blessed
targets instead of two.

### 3. `--precheck`: a command that can veto the session

An optional command, tokenised as argv with no shell (exactly the
`## Verify` rule, `crates/ralphy-core/src/verify.rs:195`), run in the repo
root before the session with a 60-second budget. Exit 0 lets the session
run; anything else — or a spawn failure, or the budget — **skips** it:
exit 0 from `ralphy prompt run`, one `prompt skipped: precheck …` line, and
`emit::run_skipped("precheck")` so the event sink and the snapshot see a
skip, not silence. The precheck's own stdout/stderr go to the log.

This is the deterministic answer to "only when there is something to do"
(`git fetch && test $(git rev-list HEAD..origin/main --count) -gt 0`,
`gh issue list --label needs-review --json number --jq 'length' | grep -v ^0`)
without teaching Ralphy conditions.

### 4. What the prompt session is not

- **Not a run.** No issue, no plan, no verify gate, no commit, no branch
  (it runs in the primary tree, in place — a prompt session is read-mostly
  and ADR-0058's `worktree` mode is for runs that commit). Its ledger line
  has `issue: 0`, like `consolidate`.
- **Not a Spawn verb.** The daemon's Spawn set is closed (`dispatch.rs:126-131`:
  "anything else is unrepresentable"). Firing an operator file from a
  browser needs an allowlist decision of its own; this ADR does not make it.
- **Not a scheduler.** The OS timer's missed-fire behaviour is the OS's
  (schtasks: run as soon as possible after a missed start, if the task says
  so; cron: never). `docs/scheduling.md` states it; Ralphy does not
  re-implement catch-up.

## Considered options — rejected

- **An in-process scheduler with its own recurrence grammar and run log.**
  Runs only while the process runs, needs a persisted queue, a catch-up
  policy, and an overlap policy — every one of which the OS timer plus
  `--if-idle` already provides. ADR-0026 §1 decided this once.
- **Reuse `consolidate` with a `--prompt-file` override.** `consolidate`
  has a validator and an archive step tied to `KNOWLEDGE.md`; a general
  prompt has neither. A separate one-shot is smaller than a mode flag on a
  purpose-built one.
- **Run prompt sessions hook-less like the other one-shots.** Their charters
  are Ralphy's and read-only; this one is the operator's and unbounded.
- **A shell-string precheck.** `## Verify` refused a shell for the same
  reason: quoting differs per OS and a shell is an injection surface from a
  file on disk.
- **A per-run worktree for prompt sessions.** Nothing to commit; the
  isolation buys nothing and costs a checkout per fire.
- **A `prompt` Spawn verb in the daemon now.** Closed set, by design; a
  browser-triggered arbitrary file is a different threat model from a timer
  the operator installed at a shell.
- **Overlap policy inside Ralphy** (queue the fire, kill the previous).
  `--if-idle` skips; a skipped fire is visible in the log and the events.
  Anything more is the in-process scheduler again.

## Consequences

- An operator can time any prompt they can write, on any of the seven
  agents, with the ledger, the guard and the run lock applied, and a
  precheck to make it cheap when there is nothing to do.
- One CLI subcommand (`prompt run`), one one-shot on seven adapters, one
  schedule target, one ledger phase, one `run_skipped` reason. The
  `run`/`triage` timers are byte-for-byte unchanged.
- ADR-0026 §2's target list, ADR-0031 D1's one-shot list, ADR-0040's Tier 1
  and Tier 3 rows, and ADR-0008 D6's phase set each carry a one-line
  amendment. `docs/scheduling.md` documents the target and the OS's own
  missed-fire rules; `docs/run-options.md` documents `prompt run`.
- Changelog: `feature` — "schedule any prompt file to run on an agent, with
  a precheck that skips it when there is nothing to do".

## Implementation notes (not decisions)

(1) `run_prompt` on the Claude adapter + `ralphy prompt run` + ledger phase,
with the guard settings; (2) the six other adapters, one issue each,
copy-shaped from their `consolidate_knowledge`; (3) `--precheck` +
`run_skipped("precheck")`; (4) `Target::Prompt` + slug/tag change +
confinement; (5) docs and amendments. (1) and (3) are green without any
timer.
