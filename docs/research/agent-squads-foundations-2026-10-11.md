# Agent interoperability: the foundations before the factory

Date: 2026-10-11. Status: **decided in a design session** (§12a, G1–G16);
the ADR "Squads" is not written yet, and no code exists. §1–§11 are the
first draft, written before the decisions; where they disagree with §12a,
§12a wins.

Companion: [mastra-2026-10-11.md](./mastra-2026-10-11.md).

## Evidence labels

Every claim carries one label:

- **[checked]** read in the code or an ADR in this session.
- **[reported]** from a subagent report in this session; the file:line it
  cites was not opened again (key ones were spot-checked: the `Agent` trait,
  `Outcome`, null stdin in `dispatch/spawn.rs`, the verb list in
  `dispatch.rs`, `STALE_AFTER`, the Claude `SendMessage` note in
  `session/spec.rs`).
- **[docs]** from Mastra or vendor documentation (see the research file).
- **[memory]** from training or earlier sessions; **verify before use**.
- **[proposal]** a suggestion in this document.

## 1. The question

The factory (issue → plan → build → review → done, with agents in every
stage) is the end state. It stands on three foundation questions:

1. **Observe:** how does one agent (or a human) see what another agent is
   doing?
2. **Notify:** how does one party put a message in front of an agent that is
   already running, and how does an agent tell another?
3. **Attention:** how does an agent get a human's attention when it needs a
   decision, and how does the answer get back?

Plus one cross-cutting question: **through which protocol** does each of
these reach each vendor?

## 2. How an agent is created today: Ralphy and Mastra

### 2.1 Ralphy [checked]

- The port is `ralphy_core::agent::Agent` (`crates/ralphy-core/src/agent.rs`):
  `name()`, `plan(&Issue, &Workspace) -> Plan`,
  `execute(&Plan, &Workspace) -> Execution`. It is a **batch** contract: one
  call in, one domain result out (`Outcome`: `Done`, `Blocked`, `Timeout`,
  `Stuck`, `Limit`). It has no message, no stream, no "send more input".
- An adapter does not own an agent loop. It **drives a vendor CLI that owns
  the loop**: Claude as a PTY TUI on execute and `-p --output-format
  stream-json` on plan; the other six vendors as one-turn-per-process
  headless children [reported].
- The prompt is a one-line pointer to files in `.ralphy/` (`plan.md`,
  `issue.json`, `handoffs.md`, `knowledge/`) [reported].
- Agent to agent today is **files and issue comments, in sequence**: planner →
  `plan.md` → executor; executor → `## Handoff` comment → `handoffs.md` →
  the next issue's planner; per-issue knowledge → `KNOWLEDGE.md` [reported].
- Agent state (`working` / `waiting` / `done` / `blocked`) comes from the
  vendor's hooks through a JSONL file the adapter or the daemon tails
  (ADR-0059) [checked]. Only Claude has hooks wired [reported].
- Interactive agents are **workbench sessions**: a vendor TUI in a daemon PTY,
  typed into by the browser that holds the **writer slot** (CONTEXT.md)
  [checked].

### 2.2 Mastra [docs]

- An agent is an object in the same process: model + instructions + tools +
  memory. **Mastra owns the loop.**
- The unit of conversation is a **thread** (stored). A live **session**
  (AgentController) holds mode, model, permissions and an event bus.
- Input reaches a running loop through `sendMessage` / `queueMessage` /
  `sendSignal`; a reasoning-only request is cancelled and sent again with the
  new input.
- Subagents are tool calls of the parent. Other vendors' agents enter as
  subagents through ACP (`AcpAgent`), vendor SDKs, or A2A.

### 2.3 The difference that decides the design

| | Ralphy | Mastra |
|---|---|---|
| Who owns the agent loop | the vendor CLI | Mastra |
| Where an "inject a message" can happen | only where the vendor opens a door: hooks, PTY input, a native protocol | anywhere in its own loop |
| Cost model | the operator's vendor subscriptions | API keys per token |
| Unit of work | issue in a queue (forge) | thread |
| Agent-to-agent today | files and comments, in sequence | tool call (subagent), signals |

**Consequence [proposal]:** Ralphy must not copy Mastra's harness. Mastra's
problem is "make my own loop durable and steerable". Ralphy's problem is
"talk to loops I do not own". The closest Mastra piece is its **ACP client**
(`AcpAgent`), not its agent class. Building an own agent loop would throw
away Ralphy's main asset (the vendor CLIs, their subscriptions, their tools)
and is out of scope.

## 3. Where each foundation stands today

| Capability | Today | Evidence |
|---|---|---|
| See the state of a run agent | yes: `phase.agent` in the run snapshot, `issue.agent_state` CloudEvent, Telegram push on `waiting` | ADR-0059 [checked]; notifier [reported] |
| See the state of a console agent | yes: `agent_state` on `/api/sessions`, yellow dot "an agent is waiting for you" | ADR-0059 §5 [checked]; `app.ts` [reported] |
| State for the six non-Claude vendors | **no** | ADR-0059 §7 [reported] |
| Runs panel shows the run agent's state | **no**: `wb-runs.ts` has no `agent` field | [reported] |
| See *what* the agent does (tool calls, transcript) | **no**: events carry lifecycle only; `dev.ralphy.agent.*` planned, not built | ADR-0019 §6 [reported] |
| Peer host sees another host's agents | yes, through the local daemon (fleet, ssh tunnel) | ADR-0052, ADR-0067 [reported] |
| Send a message into a running **run** | **no**: null stdin; no steer/answer verb; only `run.stop` | `dispatch/spawn.rs`, `dispatch.rs` [checked] |
| Send input into a **console** | yes, but only by the browser with the writer slot | ADR-0051 [checked] |
| Agent → agent message | **no** Ralphy path; Claude consoles may use the vendor's own `SendMessage` by display name, not mediated by Ralphy | `session/spec.rs` [checked] |
| Agent → human: "I need you" | partly: `waiting` state; triage `escalate` comment; `ready-for-human` blocker | ADR-0059, ADR-0018, ADR-0014 [checked/reported] |
| Human gets an alert when not looking | only Telegram (one-way); no browser/OS notification | ADR-0007 [reported] |
| Human answers a waiting run agent | **no**; answers work only between runs (labels, comments) | [reported] |
| MCP / ACP / A2A in code | **none**; MCP only appears as guards that *disable* vendor MCP servers (Copilot, Cursor) | [reported] |
| `docs/events.md` matches the envelope | **no**: four emitted types are not in the catalog | [reported] |

Reading of the table [proposal]: **observe** is about 60% built, **notify**
is about 0%, **attention** is about 30% built and has no answer path for
runs.

## 4. Protocol options, by vendor

What each vendor offers for a two-way conversation. Ralphy uses none of the
two-way options today [reported].

| Vendor | Used today | Two-way option the vendor offers | Status of that claim |
|---|---|---|---|
| claude | PTY TUI; `-p stream-json` out | `--input-format stream-json` (messages in on stdin); hooks with `additionalContext`; a `Stop` hook that blocks the stop with a reason | [memory] — verify |
| codex | `codex exec … -o <file> -` | `codex app-server`: JSON-RPC, thread → turn → item, server-initiated approvals, `turn/steer`, `turn/interrupt` | [docs/reported]; method names not checked against a schema |
| gemini | headless `stream-json` | `--acp` / `--experimental-acp` | spike doc mentions it [reported] |
| opencode | `run --format json` | `opencode acp`, `opencode serve` (HTTP) | [docs] Mastra lists `opencode acp` |
| kimi | `-p … stream-json` | an `acp` subcommand | spike doc [reported] |
| copilot | `--output-format json` | ACP listed in the ACP registry | [reported] third-party |
| cursor | `-p stream-json --resume` | Cursor SDK (Mastra wraps it) | [docs] |
| all seven | — | **MCP client**: every vendor CLI can call tools on an MCP server | [memory] — verify per vendor; note Copilot and Cursor adapters disable or refuse extra MCP servers on purpose |

Protocol roles, in one line each:

- **MCP:** an agent calls tools that someone else serves. Direction: agent →
  service. Every vendor speaks it as a **client**.
- **ACP:** an editor (client) drives a coding agent (server) over stdio:
  prompt, stream, permission requests, cancel. Direction: Ralphy → agent.
  Several vendors speak it as a **server**.
- **Vendor-native** (Codex app-server, Claude stream-json input): the same
  role as ACP, one vendor each.
- **A2A:** agent ↔ agent across organizations and the network, with agent
  cards and tasks. Direction: peer ↔ peer.
- **PTY:** the universal fallback: type text, read the screen. Works for every
  TUI. Has no structure.

**80/20 reading [proposal]:** MCP is the only protocol all seven vendors
speak, and it is the right direction for "agent → Ralphy" (observe others,
notify others, call the human). The hard direction is "Ralphy → a running
agent". For that there is no universal protocol; the cheap universal point
is the **turn boundary** (hooks, or the agent reading its inbox through an
MCP tool). Mid-turn steering needs ACP or a vendor-native protocol, one
vendor at a time.

## 5. Functional map

```mermaid
flowchart TB
  subgraph Factory["Factory (end state, later)"]
    F1[Board and stages] --> F2[Stage rules: manual or auto]
    F2 --> F3[Review in a separate agent]
  end

  subgraph Found["Foundations (this document)"]
    D[Agent directory<br/>who is live, where, which vendor]
    O[Observation<br/>agent state + activity]
    M[Messaging<br/>mailbox + delivery at a turn boundary]
    A[Attention<br/>attention request + answer]
    L[Agent link<br/>the protocol to one vendor]
    X[Exposure<br/>ralphy mcp: tools for agents]
  end

  subgraph Today["Exists today"]
    Q[Queue, run, planner/executor, verify gate]
    S[Workbench sessions, writer slot]
    E[Event bus + sinks: snapshot, CloudEvents, Telegram]
    H[Hooks → agent state]
    P[Fleet: peers over loopback or ssh]
  end

  Factory --> Found
  D --> S
  D --> P
  O --> H
  O --> E
  A --> E
  M --> L
  X --> D
  X --> O
  X --> M
  X --> A
  L --> S
```

| Capability | What it answers | Builds on |
|---|---|---|
| **Agent directory** | which agents are live (run agents and console agents), on which host, which vendor, which state | `/api/sessions`, run snapshots, fleet |
| **Observation** | what state an agent is in; later, what it did (tool calls) | ADR-0059 hooks, run snapshot, events |
| **Messaging** | leave a message for an agent; deliver it at the next turn boundary; record that it was read | new |
| **Attention** | one list of "a human must decide something", with the question, the options, the age, and a way to answer | `waiting` state, `ready-for-human`, escalate, verify failure |
| **Agent link** | the per-vendor way to put input into a running agent: PTY, hook reply, native protocol, ACP | the adapters; the console PTY |
| **Exposure** | the tools an agent calls: list agents, read state, send a message, read its inbox, ask a human | new: `ralphy mcp` |

## 6. Domains (bounded contexts)

Names in *italics* are **candidate terms**. Before any of them is used, each
must be checked against CONTEXT.md (`/domain-modeling`). Known traps:
"session" is reserved (avoid it for one issue inside a run; "workbench
session" exists), "channel" is taken (release channel), "HITL" is reserved
for the triage role ("supervised session" is the term for a person following
a live agent).

| Domain | Owns | Candidate terms | Today's owner |
|---|---|---|---|
| Queue and run (core) | issue order, plan, execute, verify, close | existing | `ralphy-core` |
| Agent runtime | how one vendor CLI is driven | adapter, execution mode | `ralphy-agent-*` |
| Live agents | which agents run now, where | *live agent*, *agent directory* | split: daemon (consoles), snapshots (runs) |
| Observation | state, later activity | agent state (exists), *activity* | ADR-0059 fold, in two copies |
| Messaging | messages to agents, delivery, read receipts | *mailbox*, *message*, *delivery point* | none |
| Attention | requests for a human and their answers | *attention request*, *answer* | none (pieces in hooks, labels, Telegram) |
| Interop (exposure) | the MCP tools Ralphy offers to agents | *ralphy mcp* | none |

## 7. Hexagonal view

The new ports sit **in the daemon**, not in core, for three reasons
[proposal, based on checked rules]: the daemon already owns live consoles,
presence and the fleet; a run talks to the daemon only through files
(ADR-0047, ADR-0054), and a mailbox file keeps that rule; and core must stay
free of vendors (ADR-0002). This placement is itself a crate boundary
decision, so it needs an ADR first (AGENTS.md).

```mermaid
flowchart LR
  subgraph Driving["Driving adapters (who asks)"]
    MCP[ralphy mcp<br/>stdio MCP server]
    WB[Workbench verbs]
    TG[Telegram, later two-way]
    CLI[ralphy CLI]
  end

  subgraph Hex["Interop core (daemon)"]
    DIR[[Agent directory]]
    OBS[[Observation]]
    MBX[[Mailbox]]
    ATT[[Attention]]
  end

  subgraph Driven["Driven ports → adapters (who is reached)"]
    LINK{{AgentLink port}}
    LINK --> PTYL[PTY link<br/>console typing]
    LINK --> HOOKL[Hook link<br/>reply at turn boundary]
    LINK --> PULL[Pull link<br/>agent reads inbox via MCP]
    LINK --> ACPL[ACP link<br/>gemini, opencode, kimi…]
    LINK --> NATL[Native link<br/>codex app-server, claude stream-json]
    NOTE{{Notifier port}}
    NOTE --> UIN[Workbench + browser notification]
    NOTE --> TGN[Telegram]
    NOTE --> CEV[CloudEvents]
    STORE{{Store port}}
    STORE --> FILES[Files under the daemon dir / .ralphy]
  end

  MCP --> DIR & OBS & MBX & ATT
  WB --> DIR & OBS & MBX & ATT
  TG --> ATT
  CLI --> MBX
  MBX --> LINK
  ATT --> NOTE
  OBS --> NOTE
  DIR --> STORE
  MBX --> STORE
  ATT --> STORE
```

Port sketch [proposal; names are placeholders]:

- `AgentLink`: `deliver(agent, message) -> Delivered | Queued`,
  `interrupt(agent)` (optional capability), `capabilities() -> {mid_turn,
  turn_boundary, pull_only}`. One implementation per **delivery point**, not
  per vendor; a vendor maps to the best delivery point it has.
- `Notifier`: `raise(AttentionRequest)`, `resolve(id)`. The existing event
  sinks are the first implementations.
- `Store`: append-only files, the same pattern as `agent-status.jsonl`.

## 8. The three foundations as flows

### 8.1 One agent observes another

```mermaid
sequenceDiagram
  participant A as Agent A (any vendor)
  participant M as ralphy mcp
  participant D as Daemon
  participant B as Agent B (console or run)
  B->>D: hook status → agent-status.jsonl (exists)
  A->>M: tool agents_list / agent_state(B)
  M->>D: read /api/sessions + run snapshots
  D-->>M: B: waiting, "AskUserQuestion: …"
  M-->>A: state + detail (marked as untrusted text)
```

Cost: small. All the data exists; only the MCP door is new.

### 8.2 One party notifies a running agent

```mermaid
sequenceDiagram
  participant S as Sender (agent, human, CI)
  participant D as Daemon mailbox
  participant L as AgentLink
  participant B as Agent B
  S->>D: message(to=B, from, priority, dedupe key)
  D->>D: append to B's mailbox file
  alt B has a hook delivery point (Claude)
    B->>L: next hook fires (PostToolUse / Stop)
    L-->>B: hook reply carries the message as added context [memory: verify]
  else B can only pull
    B->>D: MCP tool inbox_read at its own pace
  else B has ACP / native protocol (later)
    L->>B: prompt or steer mid-turn
  else B is a console TUI
    L->>B: type into the PTY (needs the writer-slot change of the console brainstorm)
  end
  D->>D: mark delivered / read; emit event
```

Rules [proposal]:

- A message is **untrusted data** for the receiver, wrapped with its sender,
  the same way Mastra wraps `<user name=… sentFrom=…>`. ADR-0072 trust zones
  apply: an agent's output is not an operator instruction.
- Every link has a **budget**: messages per agent per hour, and a hop count,
  so two agents cannot answer each other forever.
- Delivery at a turn boundary is the default. Mid-turn is a capability a link
  may have, never a promise.

### 8.3 An agent calls the human

```mermaid
sequenceDiagram
  participant B as Agent
  participant D as Daemon attention list
  participant N as Notifier
  participant H as Human
  B->>D: waiting (hook) or MCP tool ask_human(question, options)
  D->>D: open attention request (who, question, options, since)
  D->>N: raise
  N-->>H: workbench badge + browser notification + Telegram push
  H->>D: answer (workbench, later Telegram)
  D->>B: deliver the answer as a message (8.2)
  D->>D: close the request; emit event
```

Sources that open an attention request, all of which exist today as
separate facts [checked/reported]: agent state `waiting` (runs and
consoles), a `ready-for-human` blocker (ADR-0014), a triage escalate
(ADR-0018), a verify gate failure (ADR-0011), a usage-limit sleep. Today
each one reaches the human by a different path, or by none.

## 9. 80/20 roadmap

Ordered by value per cost. Each step stands alone and is useful without the
next.

**Step 0: close the gaps in what already exists (days, no new concept).**

1. Runs panel shows `phase.agent` (the state already reaches the snapshot).
2. A browser notification (Notification API) when an agent turns `waiting`
   and the tab is hidden. Opt-in.
3. `docs/events.md` lists the four emitted types it misses.
4. Agent state for more vendors: check which vendors have a hook or event
   for "turn ended" and "asks permission" (Gemini, Codex, OpenCode, Copilot,
   Kimi, Cursor) — research first, one adapter at a time.

**Step 1: one attention list (the human side).** One owner (the daemon)
gathers every "a human must decide" fact listed in 8.3 into one list, with
age and a link to where to answer. No new answer path yet: the link opens
the console, the issue, or the run. This is the piece that most changes the
operator's day.

**Step 2: `ralphy mcp`, read and ask only.** A stdio MCP server, a thin layer
over daemon routes (as the console brainstorm already proposed). Tools:
`agents_list`, `agent_state`, `attention_list`, `ask_human`. Read-only plus
one write (`ask_human` opens an attention request). Works for every vendor
whose MCP client Ralphy is allowed to configure.

**Step 3: mailbox and delivery at a turn boundary (agent → agent).** Tools
`message_send`, `inbox_read`. Delivery: Claude through hook replies (verify
first); every other vendor by pull. Answers from step 1 travel as messages.
Budget and hop limit from day one.

**Step 4: mid-turn links, one vendor at a time, only where step 3 showed a
real need.** ACP first (it covers several vendors with one client), then
Codex app-server. Each one is an adapter-level change (ADR-0002).

**Not now:**

- **A2A.** It solves agents across organizations and the network. The fleet
  already federates Ralphy's own hosts. Revisit when an outside agent must
  call Ralphy.
- **An own agent loop or a Mastra-style framework.** See 2.3.
- **Durable replay of agent runs.** Ralphy already recovers at the issue
  level (plan resume, cooperative stop, usage-limit sleep). Replaying vendor
  turns would repeat paid calls; Mastra's own docs warn about this.
- **A knowledge graph.** `KNOWLEDGE.md` and handoffs cover the current scale.

## 10. The factory, mapped to what exists [proposal, inferred from ADR titles and the reports]

| Mastra Factory stage | Ralphy today |
|---|---|
| Intake (GitHub, Linear) | the forge queue (GitHub only; the forge is not an extension point, ARCHITECTURE §6) |
| Triage | `triage-agent` role, evidence gate, escalate (ADR-0017, ADR-0018) |
| Planning + plan approval | planner; `stop-before`; plan review by a human is not a gate today |
| Building | executor, verify gate (ADR-0011) |
| Review in a separate agent | **missing**; ADR-0061 (review notes) is proposed, not built |
| Done after the task is checked again | close-on-green, acceptance ledger |
| Manual or auto per stage | queue labels and `ready-for-human` act as the manual switch per issue, not per stage |
| Trust rule (author has write access) | the label gate and the comment trust filter (ADR-0072 D5) |

The factory needs, beyond what exists: a review stage run by a different
agent, a plan approval gate, and stage rules. All three are easier once
steps 1–3 exist, because a plan approval is an attention request, and
"review found a problem" is a message to the executor.

## 11. Pushback (things this document recommends against)

- **"Talk to each vendor by its own protocol" as the first step.** Seven
  protocols, seven test harnesses, and every vendor changes its flags between
  releases (Mastra's ACP page says so). MCP plus turn-boundary delivery gives
  most of the value with one implementation.
- **Mid-turn steering as a requirement.** Mastra itself delivers at the loop
  boundary once content has started, and it only interrupts reasoning-only
  requests. A turn boundary is enough for almost every factory flow.
- **Moving the interop layer into core.** Core is batch and vendor-free by
  design (ADR-0002). The live layer belongs where live things are: the daemon.

## 12. Open questions (for a grill session)

1. Who may send a message to a run agent: only the operator, or other
   agents too? With which budget?
2. Should `ralphy mcp` be injected into run agents, or only into console
   agents at first? (Copilot and Cursor adapters disable extra MCP servers on
   purpose; this would need a security decision per ADR-0072.)
3. Attention list: does an unanswered request ever expire, or stay until a
   human closes it?
4. Is the answer to a waiting **run** agent in scope (needs a delivery point
   into a run), or only for console agents first?
5. Does the attention list span the fleet (all hosts), or one host first?
6. Is Telegram allowed to become two-way (ADR-0007 decided read-only)?

## 12a. Grill decisions (2026-10-11)

| # | Question | Decision |
|---|---|---|
| G1 | Who orchestrates the squad | **Hybrid.** Code (rules, queue, gates) decides the moves between phase roles. Inside one role, an agent may coordinate its vendor's own subagents, but its result moves on only through a gate in code. |
| G2 | Roles | A closed set of four **phase roles**: triager, planner, executor, **reviewer** (the only new one). One-shot tasks are not roles. Each role maps to adapter + model + effort in settings. A reviewer may be the same vendor and model as the executor, but always in a new process with no executor history. CONTEXT.md updated. |
| G3 | Where the reviewer acts | **Inside the run**, after the verify gate and before close-on-green (the **review gate**). It can only veto. "Changes requested" goes back to the executor **once**; after that the issue stays open for a human. Opt-in by setting. It can move to a PR trigger later, when push and PR exist. CONTEXT.md updated. |
| G4 | How roles talk | **Only through role artifacts** (files in `.ralphy/` and an issue comment for the audit trail), at points the runner controls. A live message is only "the world → one running agent" (human, operator, outside event). It is never role → role. An agent that operates another agent's console stays a console feature, outside the factory's critical path. This shrinks the mailbox to delivery to one agent, with no routing between agents. CONTEXT.md updated (Role artifact). **Scope corrected by G6: unattended mode only.** |
| G5 | Scale | Now: more repos and hosts (today's model). Next: one checkout per issue, several at once (ADR-0058), only after the review gate exists; the number per repo is a setting (default 1); quota is checked before a new one starts. Overlapping roles in one repo (planner and executor on two issues at the same time): never. Reason: the limits are vendor quota and human review, not orchestration. |
| G6 | The desk | **Two operating modes on the same foundations.** *Unattended* (run): G1–G5 hold. *Supervised* (desk): the operator orchestrates; a role is what the operator tells an agent (an open set, not the enum); **the fence is the squad**: agents in one fence may send live messages to each other; every message shows on the stage; a budget per fence (messages per hour, hop limit); a console outside every fence gets messages only from the operator; the scope works across hosts because the local daemon routes. Gives the fence a behaviour, not only a meaning, so ADR-0051 needs an amendment. Convergence: run ≈ Mastra Factory/workflows; desk ≈ Mastra AgentController/Sessions/Signals. CONTEXT.md updated (Fence, Operating mode). |
| G7 | How a message reaches a console agent | **Doorbell + letter.** The letter (the content) stays in the daemon mailbox and is read only through MCP (`inbox_read`), wrapped as `<message from="fence / console" kind="agent">`. The doorbell is a fixed text written by Ralphy, never the letter's content. It goes in through a hook reply when the agent is in a turn, or is typed into the PTY when the agent is `done` and the operator has not typed for N seconds. Another agent's content is never typed into a PTY automatically. A vendor with no MCP client does not join the squad, and the console says so. **Parked for later:** a daemon tool (MCP or other) that sends content to any PTY and reads its result, for agent CLIs, shells and non-agent TUIs (htop). It is a console operation tool with its own authority question (who may call it). See the console MCP brainstorm of 2026-09-30. |
| G8 | Agent states and "both wait, nobody notices" | Three levels: **agent** (hooks: working / waiting / done / unknown), **wait** (from the mailbox: "A waits for B since T"), **fence** (derived: active / needs operator / **stalled** / idle / can't tell). Every letter with `expects_reply` has a deadline (default 10 min, one global setting); a late one becomes `overdue` and the sender gets a doorbell. `stalled` (nobody working, nobody waiting for the operator, an open wait or unread letter older than the deadline) opens an attention request that names the cycle. `can't tell` (an `unknown` agent) never shows as idle. The graph stays free; no tree is forced. Mastra avoids the problem by shape (delegation is a tool call, so a tree) and gives every wait a state and a timeout; it has no stall detector between peers (docs). |
| G9 | Duties | A **duty** per agent console: a *charter* (text given at start) + a *description* (one line other agents see in `agents_list`). Duties are operator-written files (an open set) with a few seeds. Stored on the console, like the console name. At most one **lead** per fence; the lead is the first to get the stall letter, and after the deadline it goes to the operator. No permissions per duty in v1: the charter asks, the existing guard enforces. Mastra analog: modes + subagent description. CONTEXT.md updated (Duty). |
| G10a | Owner of fence membership | **The daemon.** The rule `fenceMembership` (the fence that holds the window's centre; fences never overlap) moves from `wb-geometry.ts` to Rust in the desk; the browser reads membership and does not compute it. One fact, one owner (ARCHITECTURE §7). Needs a row in the §7 fact index. Open side finding: Claude consoles can already message each other through the vendor's own `SendMessage` by display name, outside the fence; decide later whether to accept it or turn it off for squad consoles. |
| G10 | Identity and authority of `ralphy mcp` | **A console token**, anchored on `DeskRecord.id`, put in the agent's env next to `RALPHY_STATUS_FILE` by `spec_with_status`, with the MCP config written next to `<id>.settings.json`. It opens only the squad tools (`agents_list` of its fence, `message_send` inside the fence, `inbox_read`, `ask_human`): no workbench verb, no daemon token, no PTY operation tool. The daemon sets the `from` of every letter. Scope is checked against the daemon's membership (G10a) on every desk change; the token dies with the session. Squad routes always need the console token, even on loopback (identity, not only access). Only consoles inside a fence get `ralphy mcp`; runs do not in v1. ADR-0072 amendment: a new zone (an agent in a squad: its letters are untrusted for the receiver) and a D7 exception (a console-scoped token is given to the agent by design). |
| G11 | The attention list | **Derived in the daemon**, not stored. An item lives while its source says so; only an `ask_human` item is kept until it is answered or its console closes. v1 sources: a console agent `waiting`, a run agent `waiting`, a fence `stalled`, an `ask_human`. Forge sources (human blockers, escalate, verify failed) come in v2. Surfaces: an area in the workbench, plus a browser notification (opt-in) when the tab is hidden. Telegram unchanged. **The run (unattended) side does not change:** no answer path into a run, no new guard rule. Answering is a squad (supervised) feature only. |
| G12 | How the human answers a squad agent | `ask_human` items carry the answer (option buttons + free text); the answer becomes a letter with a doorbell (G7); it works from a phone or tablet with no terminal. A vendor-native wait (permission menu, a TUI question) gets a "go to the console" button that focuses the console and asks for the writer slot; no remote typing into a TUI menu in v1. The base squad charter tells every agent to ask through `ask_human`, not on the screen. Answers from outside the workbench (Telegram): v2. The parked PTY operation tool is what could later answer native menus from the item. |
| G13 | Vendors in the squad | **v1: Claude only, end to end** (two Claudes in a fence: duties, letters, `ask_human`, `stalled`), because only Claude has state hooks, which make the doorbell safe and `stalled` reliable. Every next vendor is one issue with a squad checklist: **S1** MCP config at launch, **S2** duty charter at launch, **S3** state hooks, **S4** doorbell point; the vendor list stays in the ADR-0040 inventory. A vendor with S1+S2 but not S3/S4 may join as an explicit **letter-only member** (no doorbell, state `can't tell`). Order after Claude: Codex, then Gemini, each after a research spike that answers S1–S4 with evidence. **Revised by the operator: v1 = Claude, Codex and OpenCode** (see 12e). |
| G14 | Claude's own `SendMessage` | **Turned off only for consoles inside a fence**, by adding it to the deny list of the per-console settings file the daemon already writes. Outside any fence nothing changes. Reason: inside a squad there is one way to talk, or the fence scope, the budget, the timeline and `stalled` stop being true. Not chosen: an allowlist (Mastra's way; it would take tools from the operator in an interactive console) and a bridge that turns `SendMessage` into letters (depends on vendor internals). Known risk, written down: two agents can still talk through files. To verify: `SendMessage` can be denied by settings on the installed Claude version. **Revised after 12c/12e (operator agreed): one channel, Ralphy's letters, for all three v1 vendors.** Inside a fence a PreToolUse hook denies Claude's `SendMessage` when the target is another session ("use message_send"); messages to Claude's own subagents still work. If the spike shows the tool input cannot tell a session from a subagent, deny all `SendMessage` inside a fence. Reason: Codex and OpenCode have no native peer messaging, so the Ralphy channel exists anyway; two channels would split the timeline and blind `stalled`. CONTEXT.md updated (Letter, Doorbell, Attention request). |
| G16 | Delivery order | **S0** fact spikes (report only) → **ADR "Squads"** (G6–G15, member lifecycle, back doors stated, amendments to ADR-0051 and ADR-0072, ARCHITECTURE §7 and §9 rows) → **E1** daemon owns fence membership → **E2** attention list v1 + opt-in browser notification → **E3** state fold with two source kinds (hook file, event stream) + Codex hooks + OpenCode SSE + Claude `Notification` → **E4** duty and charter at launch → **E5** daemon MCP endpoint + console token + `agents_list` + Launching/Joined/Degraded → **E6** letters (JSONL) + doorbell per vendor + `SendMessage` denied between sessions in a fence (first visible milestone) → **E7** `ask_human` → **E8** deadlines, `stalled`, lead first → **E9** PTY tool for free consoles → **E10** resume the vendor session on relaunch. Inside E3, E5, E6: **Claude → OpenCode → Codex**. Each step brings its fake agent (Rust helper binary); real vendors get a live checklist. Parallel track: review gate (G3), then checkout per run (G5). Out of v1: answering native permission prompts from the item, cost per squad, two-way Telegram. |
| G15 | The console operation tool (send to a PTY, read it) | **Only for free consoles** (shell, command, htop, a REPL) **in the same fence**, with the console token. Agent consoles are reached only by letters (G7). Visible (live typing, an "operated by …" mark, take back control), waits if the operator typed in the last N seconds, recorded in the audit log apart from operator input. Reading: v1 `read_output` (plain text from the existing 256 KiB ring); later `read_screen` (needs a terminal emulator in the daemon). Comes after the squad core. |

## 12b. What exists today and can be reused [checked 2026-10-11]

| Need | What exists | Where | Gap |
|---|---|---|---|
| A stable identity for a console | `DeskRecord.id`, stable across daemon restarts (the `session_id` is volatile and never the key) | `crates/ralphy-daemon/src/desk.rs` `DeskRecord` | the agent process does not know its record id |
| Where a duty is stored | `DeskRecord.console_name` (ADR-0066) is the pattern: one optional field on the record, capped | `desk.rs` | add `duty` next to it |
| Fence membership | `fenceMembership` in the browser: a window belongs to the fence that holds its **centre**; fences cannot overlap (`fenceFits`), so the answer is unique | `assets/ui/wb-geometry.ts:138` | **the rule lives only in the browser**. The daemon has every rect (desk layout is daemon state, ADR-0050) but does not compute membership. To enforce squad scope, the daemon must own this rule (ARCHITECTURE §7: one owner per fact) |
| Give a console agent per-console files and env | `StatusFiles::for_session` (`<id>.settings.json`, `<id>.agent-status.jsonl`) and the `env` list in `spec_with_status` (`RALPHY_STATUS_FILE`) | `agent_state.rs:203`, `session/spec.rs:194-221` | Claude only; add a console token and the MCP config the same way |
| A hook as a delivery point | the per-console Claude settings file already registers `hook status` on 7 events | `agent_state.rs:236-256`, `ralphy-cli/src/hook.rs` | the hook prints `{}`; it would return the doorbell text instead |
| Agent state | the fold `working/waiting/done/unknown`, shared by run and console (one fixture) | `agent_state.rs`, `ralphy-agent-claude/src/status.rs` | Claude only |
| Push to the browser | the presence socket and `sessions.dirty` | `routes/presence.rs` | add fence state and letters |
| Auth of a caller | `AuthPolicy` = `Localhost` / `Bearer` / `Session` (cookie + TOTP) | `auth/policy.rs` | there is no caller identity finer than "the operator"; a console token is new |
| Local transport for `ralphy mcp` | loopback HTTP; `daemon.sock` (Unix only, `0600`, per account) | `serve/socket.rs` | on Windows only loopback TCP |
| Cross-host | `/api/peer/command`, the session relay frame by frame, `/api/sessions` merges peers | `peer/`, `routes/api_sessions.rs` | letters are not relayed yet |
| Record of who did what | the audit log records console launches and takeovers (ADR-0074) | `audit.rs` | add letters and doorbells |
| Show a short message to the operator | operator message: confirm, notice, toast, flash (ADR-0073) | `wb-*` | no OS or browser notification |
| Push to a phone | Telegram pushes each `waiting` agent state (one-way) | `ralphy-cli/src/telegram/notifier.rs` | run-side only; consoles do not reach Telegram |
| Type into a console / watch it | the writer slot; `?watch=1` read-only attach; a 256 KiB scrollback ring | `routes/ws_session`, `session/manager.rs` | the daemon itself never types into a PTY today |
| Enforce what an agent may not do | the guard hook (`git push`, `gh pr` writes…) | `ralphy-cli/src/guard.rs` | unchanged |
| A charter file written by the operator | ADR-0062 "prompt file" (proposed): a file inside the repo or `~/.ralphy/prompts/`, an absolute path checked at install | `docs/adr/0062` | the duty charter can take the same confinement rule |
| Claude's own agent-to-agent | Claude sessions address each other by display name (`SendMessage`); Ralphy sets `--name wb-…` | `session/spec.rs:140` | not mediated by Ralphy; outside the fence rules |

## 12c. Critical review (2026-10-11, after G15)

### Facts checked (were memory before)

| Claim | Result | Source |
|---|---|---|
| Claude has `--mcp-config`, `--append-system-prompt[-file]`, `--disallowed-tools`, `--name` | **true** | `claude --help`, Claude Code 2.1.296 installed |
| Claude MCP over HTTP with headers | **true** (`--transport http`, `-H "Authorization: Bearer …"`) | `claude mcp add --help` |
| Hooks return `additionalContext` | **true** on SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, PostToolBatch, Stop, SubagentStop… (wrapped in a system reminder) | code.claude.com/docs/en/hooks |
| A Stop hook can keep Claude going | **true** (`decision: "block"` + `reason`, or exit 2) | hooks docs |
| A background hook can wake an idle Claude | **true**: `asyncRewake: true`; on exit 2 its stderr is shown to Claude as a system reminder; no timeout enforced on async hooks | hooks docs |
| An idle/needs-input signal | `Notification` matchers `idle_prompt`, `permission_prompt`, `agent_needs_input`, `elicitation_dialog` (ADR-0059 does not register `Notification`) | hooks docs |
| `SendMessage` can be denied | **true**: permission deny rules `SendMessage` and `ListAgents`; side effect: it also removes messaging to subagents and agent-team teammates | docs/en/cross-session-messaging |
| Console Claude runs in a permission-prompting mode | **true**: the console spec passes no skip-permissions flag | `session/spec.rs` |
| A relaunched console resumes the Claude conversation | **false**: no `--resume` in the console spec | `session/spec.rs`, grep |
| SQLite is already in the daemon binary | **true**: `rusqlite` (bundled) through `ralphy-usage-scan` (read-only use today) | `Cargo.lock`, `crates/*/Cargo.toml` |
| The CLI may not use tokio | **true** (fitness function `cli_manifest_pins_ureq_excludes_reqwest_tokio`) | ARCHITECTURE §5 |

### New finding 1: Claude already has mature session-to-session messaging

Claude Code (2.1.224+, native Windows 2.1.234+) has **cross-session
messaging** (`ListAgents`, `SendMessage`) on by default:

- Delivery between tool calls during a turn (a running tool is never
  interrupted); an **idle session starts a new turn** with the message.
- The receiver is told the message came from another session, not the user:
  it **cannot approve anything, cannot change configuration, commands do not
  run**, permission prompts still fire.
- Inbound control per session: `crossSessionInbound` = accept / hold /
  refuse; hold shows an approval dialog; at most 100 held.
- Loops throttled: per-sender rate limit, identical repeats dropped, at most
  50 queued; size cap about 1M characters.
- `notify_when_idle`: one notice when another session goes idle or exits,
  12 h expiry.
- Transport: a per-session Unix socket or Windows named pipe; path and a
  per-session token are exported to hooks and Bash
  (`CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`). A message
  from the session's own child process is delivered.
- Limits: **no per-sender allowlist** (any session of the same OS user on
  the machine); **WSL 2 and native Windows sessions cannot reach each
  other**; containers and host cannot either.

Consequence: G7 and G14 were designed as if Ralphy had to build the
transport for Claude. It does not. Claude already solves delivery at turn
boundaries, waking an idle session, authority labelling, and loop limits —
the hardest parts of G7. What Claude does not give: fence scope, Ralphy's
view (timeline, stalled, audit), other vendors, WSL↔Windows.

### New finding 2: Claude Channels exist, but not for Ralphy now

A channel is a stdio MCP server that pushes `notifications/claude/channel`
events into a running session (shown as `<channel source=…>`), with a reply
tool and **permission relay** (approve tool prompts from elsewhere). It is a
research preview: a custom channel needs `--dangerously-load-development-channels`
and a full-screen confirmation at every start, and does not load under `-p`.
Not usable for consoles Ralphy launches today. Worth watching: permission
relay is exactly "answer a native permission prompt from the attention item"
(G12).

### Gaps found

1. **Storage for the squad** was never decided (see 12d).
2. **`ralphy mcp` cannot be an async MCP server in the CLI** (no tokio). Either
   a small hand-written sync stdio JSON-RPC server, or an **HTTP MCP endpoint
   in the daemon** with the console token as a header (Claude supports it).
3. **Daemon restart**: consoles die with the daemon and a relaunch does not
   resume the Claude conversation. The squad's timeline would survive, the
   agents' memory would not. Claude has `--resume`; Ralphy does not use it in
   consoles.
4. **Cost per squad**: the usage scan attributes interactive usage by path,
   not by console or fence. A squad's spend is not visible.
5. **Testing**: the squad needs a fake agent (a Rust helper binary, by
   docs/TESTING.md) that speaks MCP and the hook protocol; real Claude cannot
   run in CI.
6. **State precision**: ADR-0059 does not use `Notification` (`idle_prompt`,
   `permission_prompt`), which would make `done` and `waiting` more exact.
7. **Deadline vs Claude's own**: Claude drops a held message after
   `dialogExpiry` (5 min default) and a `notify_when_idle` after 12 h; G8's
   10 min default must not fight these.
8. **Version floor**: messaging needs Claude 2.1.234+ on native Windows; the
   roster must check the version, not only presence on PATH.

## 12d. Storage and the message stack

| Need | Volume | Writer | Reader |
|---|---|---|---|
| Fence timeline (letters seen, state changes) | low: bounded by the budget per fence | the daemon only | the browser (push), `agents_list`, the stall rule |
| Open waits, attention items | tiny | the daemon | the daemon (in memory) |
| `ask_human` answers | tiny | the daemon | the daemon |

Options:

| Option | For | Against |
|---|---|---|
| **JSONL files + state rebuilt in memory at boot** | the pattern every daemon store already uses (`daemon-audit.jsonl`, `agent-status.jsonl`, desk JSON); one writer; no new code path | queries are scans; retention needs rotation |
| **SQLite via `rusqlite`** | already compiled into the daemon (bundled); real queries; WAL | first writable database in Ralphy: schema, migrations, a new kind of test |
| **libSQL** (Turso) | async crate, replicas, sync to a server | a large new dependency; its extra value (sync, replicas) serves a multi-instance server, which Ralphy is not; the planned control plane would be the reason, and it does not exist |

Recommendation: **JSONL now**, with rotation and caps (ADR-0072 D11). SQLite
through the existing `rusqlite` is the recorded upgrade when a real query need
appears (search across timelines, the control plane). Not libSQL. Mastra uses
libSQL/Postgres because it is a library for multi-instance servers with
threads, semantic recall and a notification inbox; Ralphy's daemon is one
process per OS account.

Stack, recommended:

- **MCP for agents: an HTTP MCP endpoint in the daemon** (async stack already
  there), authenticated by the console token in a header written into the
  per-console MCP config file (owner-only). No new process, one owner, and
  the CLI tokio ratchet holds. It is a new client class, so it needs a row in
  ARCHITECTURE §9 (trust boundaries).
- **Push to the browser:** the existing presence socket (`sessions.dirty`
  pattern).
- **Into Claude:** native cross-session messaging for Claude↔Claude;
  `asyncRewake` / Stop hook for Ralphy-authored notices (stall, an answer to
  `ask_human`); no PTY typing for Claude.
- **Into other vendors (later):** MCP pull + the PTY doorbell of G7.

## 12e. v1 with Claude, Codex and OpenCode (operator decision, 2026-10-11)

Installed: Claude Code 2.1.296, codex-cli 0.160.0, opencode 1.18.32 (Windows;
opencode also reachable from WSL through the Windows npm path).

### Squad capabilities per vendor (S1–S4 of G13)

| | Claude | Codex | OpenCode |
|---|---|---|---|
| **S1 MCP at launch** | `--mcp-config <file>`, HTTP + header [checked: help] | `-c mcp_servers.<n>.url=…` + `bearer_token_env_var` (the `mcp add` flags `--url`, `--bearer-token-env-var` exist) [checked: help; the `-c` form is inferred] | `mcp` in config, `type: "remote"`; delivered by `OPENCODE_CONFIG_CONTENT`, which the run adapter already uses [checked: docs + `ralphy-agent-opencode/src/command.rs:53`]; header field not shown on the config page [to verify] |
| **S2 charter at launch** | `--append-system-prompt[-file]` [checked] | not found in help [to verify: a `-c` instructions key]; fallback: the first doorbell carries the charter path | `instructions: [path]` in config [checked: docs] |
| **S3 state** | hooks, already wired (ADR-0059) | **hooks, stable** (`features list`: `hooks stable true`): SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest, PostToolUse, Stop, Interrupt, SessionEnd… Payload carries `session_id`. Hooks come only from config layers (`~/.codex`, `<repo>/.codex`, inline `[hooks]` in `config.toml`); a per-launch file is not documented; non-managed hooks need trust, or `--dangerously-bypass-hook-trust` for one invocation [checked: docs + help] | **event stream**: the TUI is a client of a local server; `--port` on the TUI; SSE `GET /event` with `session.idle`, `session.status`, `permission.asked`, `permission.replied`; basic auth with `OPENCODE_SERVER_PASSWORD` [checked: docs + help] |
| **S4 nudge (doorbell)** | `asyncRewake` hook or Stop `block` (documented); native cross-session messaging | **`codex queue --thread <id|exact name> --message <text>`**: an idle session starts a turn, a busy one gets it as the next user turn; inherits the session's permissions [checked: help; behaviour from a third-party post] | `POST /session/:id/prompt_async` (204), or `/tui/append-prompt` + `/tui/submit-prompt` [checked: docs] |
| **Native agent-to-agent** | cross-session `SendMessage` (mature) | none between sessions (`agent_message_board` is "under development"; `multi_agent` = subagents only) | none between sessions |
| **Answer a native permission prompt from outside** | Channels permission relay (preview, not usable) | `PermissionRequest` hook can allow/deny | `POST /session/:id/permissions/:permissionID` [checked: docs] |

Readings:

- **No PTY typing is needed for any v1 vendor.** Each has a documented way
  to put a fixed doorbell text into a session: a Claude hook, `codex queue`,
  OpenCode `prompt_async`. G7's PTY doorbell stays only for later vendors.
- **A nudge arrives as a user turn** in Codex and OpenCode (and as a system
  reminder in a Claude hook). So the G7 rule matters more, not less: only the
  fixed doorbell text goes through the nudge; the letter's content goes only
  through the MCP tool result, marked as from another agent.
- **State has two kinds of source**: a hook file (Claude, Codex) and an event
  stream (OpenCode). ADR-0059's fold must take vendor-neutral raw events from
  both, so the rule stays one function.
- **OpenCode gives the most control** (server API + SSE + permission reply)
  with documented, stable surfaces. **Codex's weak point is config
  injection**: hooks and the charter have no documented per-launch path, and
  a per-console `CODEX_HOME` would move the auth file. A spike must settle
  it.
- **Every vendor has a back door**: Claude's native messaging, `codex queue`
  from any local process, OpenCode's server for anyone with the password. The
  fence scope is a guarantee of **Ralphy's** channel, not an OS sandbox. The
  ADR must say so.

### How Mastra handles communication and lifecycle with these agents [docs]

- **Shape: hub and spokes.** An outside agent (ACP: OpenCode, Gemini, Cline;
  SDK: Claude Agent SDK, Cursor, OpenAI) is a **child of a Mastra parent**,
  called as a subagent or a tool: prompt in, stream out. Outside agents never
  talk to each other; every exchange goes through the parent. Codex is not
  covered (no package; not in Mastra's ACP list).
- **Lifecycle:** lazy spawn on first use; `persistSession: true` (default)
  keeps the process and its context across calls, `false` = one process per
  prompt; SDK agents resume by the vendor's own id (`sessionId`,
  `previousResponseId`). Stop, crash, restart, timeouts and concurrent calls
  are **not documented**.
- **Permissions:** by default `@mastra/acp` **picks the first option the agent
  offers** (that is, it usually approves); a callback can override.
- **Human:** outside agents are headless; the human sees them through
  Mastra's own surfaces (Studio, AgentController sessions), never in the
  vendor's TUI.
- **Mastra as an ACP server** (`mastracode --acp`): `session/new`, prompt,
  `session/cancel`; no session load; close = close stdin; modes and models
  through `session/set_config_option`.

Comparison:

| | Mastra | Ralphy desk (squad) |
|---|---|---|
| Topology | tree: LLM parent → vendor children | mesh of peers inside a fence, under the operator |
| Who owns the process | the parent, lazily | the daemon (PTY), started by the operator |
| Human view | Mastra UI; vendor TUI hidden | the vendor's own TUI, live |
| Message path | prompt and stream through the parent | letters through the daemon; nudges per vendor |
| Lifecycle docs | spawn and persist only | must define: launch, join, leave, relaunch, end |
| Permissions | auto-pick first option by default | the vendor's own prompt, seen by the operator |

What to take from Mastra: **a session that persists across prompts** and
**resume by the vendor's id** (Ralphy lacks console resume, gap 3 of 12c).
What not to take: auto-approval by default, and the parent-only topology (G6
chose a mesh).

### Lifecycle of a squad member [proposal]

```mermaid
stateDiagram-v2
  [*] --> Launching: operator opens an agent console in a fence
  Launching --> Joined: wiring confirmed (MCP connected, state source live)
  Launching --> Degraded: wiring failed (hook untrusted, MCP refused, server down)
  Joined --> Joined: agent state working / waiting / done
  Joined --> Left: console moved out of the fence (scope revoked)
  Left --> Joined: moved back in
  Degraded --> Joined: wiring repaired
  Joined --> Ended: console closed / agent exited
  Degraded --> Ended
  Left --> Ended
  Ended --> Launching: relaunch (resume by the vendor's session id, when supported)
```

- **Wiring is confirmed, not assumed.** The Copilot adapter already guards on
  a receipt (`session.mcp_servers_loaded`); a squad member is `Joined` only
  after the daemon sees its MCP call or its first state event. Until then it
  is `Degraded`, and the fence shows `can't tell`.
- **The vendor session id** (hook payload `session_id` for Claude and Codex;
  the OpenCode session id from its server) is recorded at join. It is the
  address for `codex queue` and `prompt_async`, and the key for a later
  resume.

## 13. Facts to verify before any ADR

Added 2026-10-11 for the three-vendor v1 (spikes, each on the installed
version, Windows and WSL):

- Codex: can hooks and an instructions key be passed per launch with `-c`
  (inline `[hooks]` tables), and does `--dangerously-bypass-hook-trust` make
  them run without a prompt? Does `codex queue` reach a TUI session on
  native Windows (is the shared app-server daemon running by default,
  `daemon_auto_start`)?
- OpenCode: remote MCP with an `Authorization` header in
  `OPENCODE_CONFIG_CONTENT`; the TUI's `--port` server exposes `/event` and
  `prompt_async` for the session the TUI shows; the exact payloads of
  `session.status` and `permission.asked`.
- Claude: the `tool_input` fields of `SendMessage` (to tell a session target
  from a subagent target) and of `ListAgents`.

Older list:

- Claude Code: `--input-format stream-json`; hook replies with
  `additionalContext` on `PostToolUse` / `UserPromptSubmit`; a `Stop` hook
  that returns `decision: block` with a reason. [memory]
- Which of the other six vendors have hooks or a structured "waiting for
  permission" event.
- MCP client configuration per vendor, and whether the Copilot/Cursor guards
  can allow one Ralphy-owned server without opening the rest.
- Codex app-server method names, from `codex app-server
  generate-json-schema` on the installed version.
- ACP: which installed vendor versions answer `initialize` over stdio.

## 14. S0 spike results (2026-10-11)

Run on Windows 11 in a scratch repo (`C:\Dev\spike-s0`, outside the Ralphy
tree), with Claude Code 2.1.296, codex-cli 0.160.0 and opencode 1.18.32.
Helpers: a Python HTTP "spy" that logs each request and answers MCP
`initialize`, `tools/list` and `tools/call` with one tool (`inbox_read`,
answer "no letters"), and a Python hook that appends its stdin to a file.
WSL was not run; the WSL rows stay open.

### Claude

| Check | Result |
|---|---|
| MCP over HTTP with `--mcp-config` and a header | **pass**: every request carried `Authorization: Bearer console-token-claude`; the client first tried MCP `2026-07-28` (`server/discover`), then fell back to `2025-11-25` |
| Charter by `--append-system-prompt` | **pass**: the model reported the charter keyword |
| PreToolUse hook denies a tool | **pass** on `ListAgents` (payload: `session_id`, `transcript_path`, `cwd`, `permission_mode`, `tool_name`, `tool_input: {}`, `tool_use_id`); the agent sees the deny reason |
| `SendMessage` fields | **not observed**: in a `-p` session the tool is not offered (`ToolSearch select:SendMessage` returns only `ListAgents`) |
| MCP tool in `-p` | **pass** with `--allowedTools mcp__spy__inbox_read`; note: `--allowedTools` is variadic, so the prompt must come on stdin |

Consequence for G14: Ralphy cannot rely on reading `SendMessage`'s input.
**Inside a fence, deny `SendMessage` by a permission deny rule in the
per-console settings** (documented, does not depend on the tool's fields).
Whether interactive consoles offer `SendMessage` at all goes to the live
checklist.

### OpenCode

| Check | Result |
|---|---|
| Basic auth on the server | **pass**: `401` without the password, `200` with `OPENCODE_SERVER_PASSWORD` |
| Remote MCP with a header through `OPENCODE_CONFIG_CONTENT` | **pass**: `initialize`, `notifications/initialized`, `tools/list` all carried `Authorization: Bearer console-token-opencode` |
| `POST /session/:id/prompt_async` | **pass**: `204` |
| State events on `GET /event` | **pass**: `session.status` with `{"type":"busy"}` then `{"type":"idle"}`, and `session.idle`, all with `sessionID` |
| Charter (`instructions`) and a tool call | first run **not verified**: the zai providers on this host answered `401 Authentication Failed` (credentials, not the protocol). **Second run: pass** with `opencode/big-pickle` (a free OpenCode model): one `prompt_async` doorbell → `session.status` busy ×5 → idle → `session.idle`; the agent called the MCP tool (`spy_inbox_read`, status `completed`, output "no letters") and reported the charter keyword from `instructions`. The spy saw `initialize`, `notifications/initialized`, `tools/list`, `tools/call` and the GET stream, each with `Authorization: Bearer console-token-opencode`. OpenCode names an MCP tool `<server>_<tool>` |
| API surface in 1.18.32 (from `/doc`) | `question.asked` / `question.replied` events and `/api/session/{id}/question/{requestID}/reply` (a native ask-user that can be answered from outside); `/api/session/{id}/permission/{requestID}/reply`; `/api/session/{id}/interrupt`; the `/tui/append-prompt` and `/tui/submit-prompt` routes of the docs are **not** in this version's spec (only `/tui/control/*`, `/tui/publish`) |
| Process lifetime | the `opencode.exe` server **outlived** the shell that started it (npm shim → native binary), in both runs. A daemon that starts it must end the whole process tree |

### Codex

| Check | Result |
|---|---|
| Hooks per launch | **pass**: inline `-c "hooks.<Event>=[{hooks=[{type=\"command\",command=\"…\"}]}]"` with `--dangerously-bypass-hook-trust` runs SessionStart, UserPromptSubmit, PreToolUse, Stop; payloads carry `session_id` and `turn_id`; the flag prints a warning each run |
| Stop hook output | a Stop hook that prints `{}` is reported **"Stop Failed"**; one that prints nothing **completes**. The shared `hook status` must print nothing for Codex |
| MCP per launch | **pass**: `-c mcp_servers.<n>.url=…` and `-c mcp_servers.<n>.bearer_token_env_var=…` sent the bearer token |
| MCP tool without a prompt | needs `-c mcp_servers.<n>.default_tools_approval_mode="approve"`; without it `exec` fails the call ("requires approval, but approval policy is never") |
| Charter | **pass**: `-c developer_instructions="…"` |
| `codex queue` | **stores** the message (`Queued message … for thread …`). With no app-server daemon running (the control socket refused the connection) it does **not** start a turn; the message was delivered **first** on the next activation of the thread (`codex exec resume`). Live wake of a TUI session needs the shared app-server daemon: live checklist |

### What changes in the design

- **G14:** deny `SendMessage` by settings in a fence (not by reading its
  input).
- **G7 / doorbell for Codex:** `codex queue` is a durable mailbox, not a
  doorbell, unless the TUI runs on the shared app-server daemon. Two options
  for the ADR: (a) launch the console TUI with `--remote` on a daemon that
  Ralphy starts, so `codex queue` wakes it; (b) a Stop hook returns the
  doorbell with `decision: "block"` while the turn is ending, and an idle
  Codex is reached only on its next turn. A live test decides.
- **The fold of agent state** must allow a vendor whose Stop hook prints
  nothing; the hook protocol differs per vendor even when the events match.
- **OpenCode** offers a native question flow that the attention list can
  answer (`question.asked` → reply route) — a candidate for G12 later.
- **Hook trust for Codex** uses `--dangerously-bypass-hook-trust` on hooks
  Ralphy writes itself. That is an ADR-0072 point (D6: the guard is Ralphy's
  own; the bypass applies only to that invocation).
- **The daemon must end the process tree** of a vendor server it starts.

### Live checklist (needs a real console, after E5)

1. Claude interactive console: is `SendMessage` offered; does the settings
   deny remove it; does `asyncRewake` wake an idle console.
2. Codex: TUI with `--remote` on a Ralphy-started app-server daemon; does
   `codex queue` start a turn in it; Windows and WSL.
3. OpenCode: `opencode attach` to a Ralphy-started server shows a session
   that `prompt_async` reaches (the server side passed in S0; the TUI side
   needs a real console).
4. All three under WSL.
