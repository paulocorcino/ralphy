# Ralphy

A global, in-place runner that works a repo's GitHub issue queue unattended on a
Claude subscription, committing each issue and handing back a branch to merge by
hand. Its triage vocabulary follows Matt Pocock's canonical roles — the full set
of five and Ralphy's stance on each is in [docs/triage-roles.md](./docs/triage-roles.md).

## Language

**Run**:
One invocation of the runner over a repo's queue, identified by a timestamp.
_Avoid_: session (that's one issue's Claude execution within a run).

**Queue label**:
The label that puts an open issue into the run, worked in ascending issue-number
order. Canonical `ready-for-agent`; `AFK` is an accepted synonym.
_Avoid_: todo, backlog.

**Human label**:
The canonical `ready-for-human` (synonym `HITL`). Marks an issue as human-only —
it is **never** queried, so the agent never works it. Carries no other runtime
behaviour in Ralphy.
_Avoid_: blocked, manual.

**Green**:
An issue whose execution finished cleanly (the agent emitted `RALPHY_DONE_EXIT`),
as opposed to a non-green stop (blocked / timeout / stuck / usage limit).

**The cycle / close-on-green**:
A green queue issue is closed by the runner so it leaves the queue; its label is
left untouched and the human still merges the branch by hand.

**Run branch**:
Where commits land. `BranchMode new` cuts a fresh `afk/run-<stamp>` off the base;
`BranchMode current` commits onto the branch the repo is already on.

**Adapter**:
The isolated unit holding everything specific to one agent CLI vendor (Claude
Code today; Codex, OpenCode later), behind the core's agent contract. Each
adapter owns its own execution mode and completion protocol.
_Avoid_: driver, plugin, backend.

**Execution mode** (interactive vs headless):
How an adapter drives its CLI — an adapter/billing concern, **never** the core's.
For Claude Code, interactive (over a PTY) bills against the subscription, while
headless `-p` is metered programmatically (API-like) from 2026-06-15, so the
Claude adapter defaults to interactive to save cost. A PTY exists only to give an
interactive CLI a TTY; it is an adapter capability, not core infrastructure.
_Avoid_: -p mode, batch.

**Complexity routing**:
A Ralphy-invented capability where the planner judges an issue's complexity and
*picks* the execution model (Claude: `sonnet` for mechanical, `opus` for complex).
An **optional adapter capability**, not a core guarantee — a deterministic adapter
(fixed model + fixed effort) is a first-class citizen. Distinct from **effort**,
which is a deterministic knob the operator sets, not an auto-judged choice.
_Avoid_: model selection (too broad), auto-model.

**Supervised session**:
Live human oversight of a *running* agent session — following it and intervening
mid-flight, via Remote Control (mobile) or an on-screen terminal (local/Tauri).
The human is in the loop while the agent works. Distinct from the **Human label**
triage role: there the agent never works the issue at all.
_Avoid_: HITL (reserved for the triage role), human-in-the-loop.

**stop-before**:
A fixed control label (not configurable) on one queued issue that halts the run
**before** that issue is worked — everything earlier in the sequence is done
first. The human creates the label, removes it from the issue, and re-runs to
continue. Not a triage role — a flow control, named for its semantics.
_Avoid_: pause, hold, breakpoint.

## Relationships

- The **queue** = open issues carrying any **queue label**, ascending by number.
- A green **queue** issue is closed by the runner (the **cycle**); a non-green one
  stops the whole run and hands over the **run branch** for inspection.
- A **human label** issue is never in the **queue**.
- A **stop-before** issue halts the run before itself; the issues before it still run.
- The **core** is execution-mode-agnostic: it asks an **adapter** to work an issue
  and receives an outcome. PTY, interactive sessions, and completion sentinels
  live inside the **adapter**, never in the core.

## Flagged ambiguities

- "AFK" and "ready-for-agent" are treated as synonyms (same for "HITL" /
  "ready-for-human"). Canonical is the Matt Pocock role; the shorthand is a
  transitional alias.
- "HITL" was used to mean both the **Human label** triage role (agent never works
  the issue) and live human oversight of a running session — resolved: HITL is
  **only** the triage role; the oversight concept is a **Supervised session**.
