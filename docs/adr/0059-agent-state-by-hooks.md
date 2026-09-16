# An agent's state — working, waiting, done — comes from the vendor's hooks and travels as one event; the run and the console both publish it

Status: **accepted** (2026-09-15) — implemented the same day; the as-built
drifts are recorded in the amendment at the end, and the 2026-09-16
amendment corrects §1 (no interrupt flag) and §4 (`PostToolUse` is in). Written after
[ADR-0058](./0058-checkout-per-run.md) and independent of it.

_Extends [ADR-0039](./0039-event-vocabulary-owned-by-core-emit.md) with one
event, [ADR-0047](./0047-run-state-snapshot-channel.md) §5 with one additive
field, and [ADR-0036](./0036-workbench-daemon-integration-protocol.md) §8 with
one field on the session plane. Touches [ADR-0032](./0032-daemon-mode-supervised-launcher.md)
§10 (the daemon writes one settings file it does not interpret). Changes no
CloudEvents wire shape already published (ADR-0019); adds one type._

## What the operator cannot see today

Two surfaces show an agent, and neither can say what it is doing.

- **A run.** The snapshot's `phase.state` is `starting | planning | executing |
  sleeping | consolidating` (`crates/ralphy-cli/src/runstate/state.rs:514-530`).
  "Executing" covers an agent mid-tool-call, an agent sitting at a permission
  prompt, and an agent that asked a question and is waiting for an answer
  that will never come. The only thing that distinguishes them is the idle
  watchdog firing 20 or 45 minutes later (`idle_minutes`). The Telegram card
  and the workbench Runs panel inherit the same blindness.
- **A console.** `SessionInfo` (`crates/ralphy-daemon/src/session.rs:617-633`)
  is `id, repo, agent, kind, started_at, environment, name` plus a scrollback
  ring and `has_exited`. Nothing knows whether the console's agent is idle,
  working, or waiting for the operator — which is the one thing an operator
  on a phone wants to know.

Ralphy already has the mechanism half-built. The Claude adapter registers
three hooks on execute — `Stop`, `PreToolUse` (the guard), `PostToolUse`
(the verify-cost timer) — in `ralphy.settings.json`
(`crates/ralphy-agent-claude/src/settings.rs:233-260`), and each hook is a
separate `ralphy hook <x>` process whose only channel back is a file under
`.ralphy/` (`$RALPHY_FLAG_FILE`, `cmd-costs.json`;
`crates/ralphy-cli/src/hook.rs`). The Stop hook is, in fact, already an
agent-state hook: it says "done". It is consumed by the adapter's PTY loop
and never published.

## Decision

### 1. The vocabulary is four states, orthogonal to the run phase

An agent is `working`, `waiting`, `done`, or `blocked`. It is a property of
the agent process, not of the run: a run in phase `executing` has an agent
that is `working` or `waiting`; a run in `sleeping` has no agent at all.
`phase.state` is untouched.

| State | Meaning | Claude Code hook that produces it |
|---|---|---|
| `working` | the agent is in a turn: thinking, calling a tool | `UserPromptSubmit`, `PreToolUse` (any tool but `AskUserQuestion`), `PostToolUse` (any tool — amended 2026-09-16) |
| `waiting` | the agent needs the operator: a permission prompt or a question | `PermissionRequest`; `PreToolUse` with `tool_name == "AskUserQuestion"` |
| `done` | the turn ended; the agent is idle at its prompt | `Stop`, `SessionStart` |
| `blocked` | the vendor reports it cannot continue without something external | reserved; no Claude hook produces it — vendors with a blocking notification map to it |

`waiting` carries a `detail`: the tool name and, for a question, the
question text (from the hook's `tool_input`), so the surface can show *what*
the agent is asking, not only that it is asking. ~~A `Stop` that the vendor
marks as an interrupt (`stop_hook_active` / `is_interrupt` in the payload)
is `done` with `interrupted: true`.~~ — struck 2026-09-16: no hook observes
an interrupt (see the amendment); the vocabulary carries no interrupt flag.

Not in the vocabulary: `idle` (that is `done`), `permission` (that is
`waiting` with a detail), and any state inferred from terminal output.

### 2. One event, one function: `emit::agent_state`

Per ADR-0039 §1 the vocabulary is owned by `ralphy_core::emit` and each
consumed lifecycle event has exactly one function. This ADR adds one:

```rust
pub fn agent_state(state: &str, since: &str, detail: Option<&str>)
```

decoded into `RunEvent::AgentState { .. }` in `ralphy-cli` (ADR-0039 §5)
and covered by the §2 round-trip test. Downstream, for free, on the ADR-0024
seam:

- **Snapshot** (ADR-0047 §5): `PhaseBlock` gains an additive
  `agent: Option<AgentBlock { state, since, detail }>`; `v`
  stays 1 (§6). `since` follows the `PhaseBlock::since` convention — set on
  a state change, not on every rewrite.
- **CloudEvents** (ADR-0019): `dev.ralphy.issue.agent_state`, subject
  `issue/<n>`, data `{ state, since, detail }`.
- **Telegram** (ADR-0007): a `waiting` state is a push ("agent is asking:
  …"); `working`/`done` transitions are folded into the card, not pushed.

The adapter, not the hook process, calls `emit::agent_state`. A hook cannot:
it is a different process with no tracing subscriber.

### 3. Transport is a file the adapter tails; the hook never blocks the agent

New subcommand `ralphy hook status`. It reads the hook payload on stdin,
appends one JSON line `{ "event", "tool_name", "tool_input", "ts" }` to the
path in `$RALPHY_STATUS_FILE`, prints `{}` to stdout, and exits 0. It is a
no-op when the variable is unset — the same shape as `run_stop_hook`
(`hook.rs:162-166`) — so a hook installed into a settings file that leaks
into a non-Ralphy session does nothing. **It never fails the agent**: a
write error is logged to stderr and the exit is still 0, and the `{}` on
stdout is printed first, because a permission hook with empty stdout fails
closed in the vendor.

The adapter sets `RALPHY_STATUS_FILE=<run_dir>/agent-status.jsonl` next to
`RALPHY_FLAG_FILE` and tails the file in the drive loop it already runs
every 500 ms (`interactive.rs`, `headless.rs`), mapping events to states
per §1 and calling `emit::agent_state` on each change. The snapshot engine's
250 ms tick (ADR-0047 A4) then folds the event like any other.

Why a file and not a listener: a Ralphy run is a CLI process with no server;
the daemon may not interpret the repo (ADR-0036 §3) and does not import core
(ADR-0032 §10); and files are already the channel the Stop hook, the
verify-cost gate and the stop sentinel (ADR-0054) use. A loopback HTTP
server would be a third process contract to secure and keep alive for what
is a few hundred bytes a minute.

### 4. The Claude hook set grows, on plan as well as execute

`exec_settings_json` registers, in addition to today's three:

| Event | Matcher | Command |
|---|---|---|
| `SessionStart` | — | `hook status` |
| `UserPromptSubmit` | — | `hook status` |
| `PreToolUse` | `*` | `hook status` (a second entry; the guard keeps its narrow `Bash\|Edit\|Write\|MultiEdit\|NotebookEdit` matcher and its own command — the two are independent hooks on the same event) |
| `PermissionRequest` | `*` | `hook status` |
| `PostToolUse` | `*` | `hook status` (added 2026-09-16 — a second entry on execute; the Bash timer keeps its first slot) |
| `SubagentStop` | — | `hook status` (folded as detail on the lead's state; a subagent never owns the state) |

Deliberately **not** registered: `Notification` (it duplicates
`PermissionRequest` with less structure); `PreCompact` (compaction is not a
state the operator acts on); and ~~`PostToolUse` /~~ `PostToolUseFailure` /
`SubagentStart`, because they add no state `PreToolUse` did not already
set — every one maps to `working` — and each hook is a process spawn. This
workspace is spawn-bound on Windows (`docs/BUILDING.md`: the suite is bound by process creation, not Rust),
so a status hook on every tool boundary would double the per-tool overhead
the guard already costs for nothing the operator can see. (`PostToolUse` was
moved out of this list on 2026-09-16: it IS the state `PreToolUse` cannot
set — the end of a `waiting`. See the amendment.) The set above is
one spawn per tool call, one per prompt, one per turn end.

The **plan phase** gets the same status hooks. Today plan runs with the
hook-less `SETTINGS_JSON` (`lib.rs:193`), which is why a planner stuck at a
question is invisible until the watchdog. The guard stays execute-only —
the plan charter forbids writes by prompt, and a `PreToolUse` guard on a
read-only session would only add latency.

`PermissionRequest` deserves one sentence: Ralphy launches with
`--dangerously-skip-permissions`, so the vendor should never raise it. If it
does, that is precisely a `waiting` the operator must see.

### 5. The daemon injects the same hooks into a console and publishes on the session plane

`spec_for` (`session.rs:180-224`) passes `--settings <daemon dir>/sessions/<id>.settings.json`
for Claude consoles, a file the daemon writes with the §4 hook set whose
commands are `<ralphy exe> hook status` — the exe the daemon already resolves
for Spawn verbs (`dispatch::ralphy_exe`). The PTY environment gets
`RALPHY_STATUS_FILE=<daemon dir>/sessions/<id>.agent-status.jsonl`. The
daemon tails that file with the watcher it already has (`watch.rs`) and
applies the §1 mapping — a pure fold over event names, in the daemon's own
crate, with the mapping table duplicated from the adapter and pinned by a
shared fixture so the two cannot drift. It does not import core.

`SessionInfo` gains `agent_state: Option<{ state, since, detail }>`,
surfaced on `/api/sessions` and pushed on the presence socket when it
changes. This is the **session plane gaining a field** (ADR-0036 §8), not a
fourth plane: the state belongs to the session that owns the PTY.

A console the operator opens with their own `~/.claude/settings.json` hooks
keeps them: the vendor merges `--settings` over the user file, and hook
entries are additive.

### 6. Staleness: tied to the idle watchdog, cleared on exit, not persisted

A `working` older than `idle_minutes` (the same knob that reaps a wedged
run) renders as `unknown`; a `waiting` never goes stale (a question is
still a question). A PTY exit clears the state. Neither the adapter nor the
daemon persists agent state across a restart: a run that restarts re-derives
it from the next hook; a console does not survive a daemon restart at all.

### 7. Vendors without hooks emit nothing

This is an adapter capability under ADR-0002, not a core guarantee. Claude
is the first and only adapter in this ADR. Gemini has `SessionStart` and
`AfterAgent` hooks (`docs/research/gemini-cli-adapter-spike.md`); Codex,
Cursor and Copilot have hook files of their own; each is a follow-up under
its adapter's ADR-0040 wiring inventory with its own mapping table.
Until then their `agent` block is absent, which every reader already
handles (`Option`).

Terminal-title heuristics, spinner detection and stdout pattern matching are
explicitly out. A state that is guessed is worse than a state that is
absent: the operator acts on it.

### 8. What does not change

- `phase.state` and `IssueBlock.status` vocabularies (`state.rs`), the
  `run.heartbeat` event, and every existing CloudEvent.
- The Stop hook's `DONE`/`BLOCKED` flag-file protocol (ADR-0003 completion
  detection). `hook status` is a second, independent hook on `Stop`.
- The guard's matcher and command.
- The daemon's three-plane model; the daemon still spawns nothing on its own
  and reads only bytes.

## Considered options — rejected

- **A loopback HTTP listener in the run, hooks POST to it.** Adds a port, a
  token, a spool for when the POST fails, and a process contract the daemon
  would have to mirror for consoles. The file is the spool with nothing in
  front of it.
- **Infer state from the PTY stream** (spinners, `✳` idle glyphs, title
  keywords). Vendor-version-fragile, and wrong in exactly the case that
  matters — a permission prompt looks like idle text. Hooks are the vendor's
  own statement.
- **A new `phase.state` value such as `waiting`.** Conflates the run's phase
  with the agent's condition; `sleeping` has no agent, `executing` has one
  in any of three states. Orthogonal field.
- **Persist the last state across restart.** A state written before a
  restart describes a turn nobody observed the end of; showing it as fresh
  misleads. Re-derive.
- **Register `Notification`.** Same signal as `PermissionRequest` with a
  free-text message instead of a tool name.
- **Make the daemon call into the adapter's mapping.** ADR-0032 §10: the
  daemon never imports core or an adapter. A duplicated table pinned by a
  shared fixture costs less than the dependency.

## Consequences

- The workbench can show, per run and per console, "working / asking:
  <question> / idle", and the Telegram card can push the middle one.
  Item 4 of the same track (review notes pasted into a console) is gated on
  this: a paste is safe only when the state says the agent is at its prompt.
- Plan-phase hangs at a question surface immediately instead of after the
  watchdog window.
- One new subcommand (`hook status`), one new `emit::` function, one new
  `RunEvent` variant, one new CloudEvent type, one additive snapshot field,
  one additive `SessionInfo` field, one settings file the daemon writes.
- ADR-0039's vocabulary table, ADR-0047 §5's field list, ADR-0036 §8 and
  ADR-0032 §10 each carry a one-line amendment pointing here. `docs/events.md`
  gains the new type.
- Changelog: `feature` — "the workbench and the Telegram card show when an
  agent is waiting for you, and what it is asking".

## Implementation notes (not decisions)

(1) `hook status` + `RALPHY_STATUS_FILE`, unit-tested with the vendor's
payload fixtures; (2) `emit::agent_state` + `RunEvent` + round-trip test +
snapshot field + CloudEvent; (3) Claude adapter: hook set on plan and
execute, tail-and-map in the drive loop; (4) Telegram push on `waiting`;
(5) daemon: settings file, env, watcher fold, `SessionInfo` field, presence
push; (6) workbench: badge on Runs and on console chrome; (7) docs and
amendments. Each step is one issue; (1)–(3) are green without the daemon.

## Amendment (2026-09-15): as built

Implemented in four commits following the implementation notes: the run-path
spine (`ralphy hook status`, `emit::agent_state`, `RunEvent::AgentState`,
`phase.agent` on the snapshot, `dev.ralphy.issue.agent_state`, the Telegram
`waiting` push), the Claude adapter (hook set on plan and execute, a
`status::Watcher` thread tailing `<run_dir>/agent-status.jsonl` on every
child shape — plan, PTY and headless), the daemon console path, and the
workbench dots. Where the built thing differs from the text above:

- **§3 — the tail is a thread, not the drive loop.** The plan phase blocks
  in `wait_with_output` and the headless path in a shared runner, so one
  `Watcher` thread polling every 500 ms serves all three children; the PTY
  drive loop did not need a fourth reader.
- **§4 — `SubagentStop` is registered and folds to nothing.** "Folded as
  detail on the lead's state" would have needed a state to fold into; the
  fixture pins it as a no-op, and the lead's own `Stop` says what matters.
- **§5 — polled, not pushed.** `agent_state` rides `/api/sessions`, which
  the shell already polls on every 2 s presence tick; the presence socket
  was not extended. The staleness window is the daemon's own constant
  (45 min, `agent_state::STALE_AFTER`), restated from core's interactive
  default because the daemon does not import core. The clock it ages is
  the last **folded line**, not the last transition: a `working` that keeps
  producing `PreToolUse` lines is alive and stays green (review fix,
  2026-09-16). A repeated `waiting` with the same detail is one event on
  both paths — the same permission asked twice is one buzz. The session id is
  reserved before the spawn (`SessionManager::reserve_id`) so the files are
  named by it and exist before the child that reads them.
- **§5 — the shared fixture is the adapter's file, included by path.** The
  daemon's test `include_str!`s
  `crates/ralphy-agent-claude/tests/fixtures/agent_state_mapping.json`; no
  crate dependency, and moving the file reds both.
- **Workbench (note 6).** A dot before the console's title, on the project
  row (`waiting` outranks `live`), on the picker's worktree rows (joined on
  `checkout`), and on the Go-to list; a `waiting` dot's tooltip carries the
  detail. Not built, deliberately: an unread/bold state and a notification
  bell — the workbench has no toast surface and no focus tracking, and each
  is its own design.
- **Vendors.** Claude only, as §7 says. The other adapters' hooks remain
  ADR-0040 follow-ups.

## Amendment (2026-09-16): `PostToolUse` is in; the interrupt flag is out

Two corrections after the review of the first day's build, each grounded in
the vendor's own hooks reference (`code.claude.com/docs/en/hooks`, read
2026-09-16).

**§4 — `PostToolUse` on every tool.** The rejection above rested on
"`PostToolUse` adds no state `PreToolUse` did not already set". It adds the
one state `PreToolUse` cannot: the *end* of a `waiting`. Without it the
sequence is `PreToolUse(AskUserQuestion) → waiting`, the operator answers,
and the dot stays yellow until the agent's *next* `PreToolUse` or `Stop` —
seconds of thinking for a question, and for a granted permission the whole
run time of the tool that was waiting (`PermissionRequest(Bash) → waiting`,
grant, `cargo test` for three minutes, still yellow). The yellow dot is the
one signal the operator acts on; a yellow that lies costs the feature its
trust. So `PostToolUse` matcher `*` is registered on plan, execute and the
console, folding to `working`; the `Tail`'s dedupe swallows the
`working → working` repeats, so the emitted transitions do not change
except where they should. The cost is one more `hook status` spawn per
tool call, which the vendor runs in parallel with the tool's result — the
§4 spawn argument still holds for `Notification`, `PreCompact`,
`PostToolUseFailure` and `SubagentStart`, none of which end a state.
`AskUserQuestion` alone was considered and rejected: it fixes the question
and not the permission.

**§1 — there is no `done{interrupted}`.** The vendor's reference says of
`Stop`: *"Does not run if the stoppage occurred due to a user interrupt."*
And `stop_hook_active` means *"Claude Code is already continuing as a
result of a stop hook"* — which is exactly what Ralphy's own sentinel
`hook stop` causes on every turn it sends back for a missing trailer. As
built on 2026-09-15 the fold read `stop_hook_active` as an interrupt, so
every sentinel-retried turn reported `done{interrupted}` into the snapshot
and the CloudEvent for a turn nobody interrupted. `is_interrupt` exists only
on `PostToolUseFailure`, and "cancelling a running tool does not fire this
hook". No hook observes an operator's interrupt; the field is removed from
`emit::agent_state`, `RunEvent::AgentState`, `AgentBlock`, the CloudEvent
data and the status line — none of it had shipped. What an interrupt looks
like now: nothing arrives, the state stays `working` until the §6 staleness
window ages it to `unknown` or the next `UserPromptSubmit` starts a turn.
That is the honest reading — §7's rule, a guessed state is worse than an
absent one, applies to a guessed flag.

**Fixture.** `agent_state_mapping.json` gains the two `PostToolUse` rows
and a `PermissionRequest` with no `tool_name` (detail `permission`), and
loses the interrupt row; both folds are pinned by it as before.
