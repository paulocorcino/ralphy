# An idle console is suspended, then hibernated

Status: proposed
Kind: structural
Protects: responsiveness, recoverability; constraints portability, security

Amended by ADR-0085 (proposed: a `waiting` agent blocks hibernation, not suspension).
Amended by ADR-0088 (proposed: the process registry, one container per console, replaces D4
and the process part of D6). Amendment of 2026-11-15 below: hibernation first, as a policy over it.
Amendment of 2026-11-16 below: a state that is not known blocks; hibernation is an operation with steps.

Amends ADR-0051 (a **workbench session** has a state besides live and gone),
ADR-0050 (the page no longer relaunches a free console when it loads; the
daemon starts consoles, through a queue) and ADR-0072 D13 (undocumented OS
calls).

## Context

An operator keeps many **workbench sessions** open, and most of them sit idle
for hours. Each one holds memory: an OpenCode console measured 659 MB of
working set and 1 188 MB of private memory (Windows 11, 2026-10-24). High
memory in use is the operator's problem, not CPU.

Measured on that console the same day, with nobody using it: a working-set
trim of the live process (`EmptyWorkingSet`) took the working set from 659 MB
to 0, then it came back to 307 MB after 10 minutes and to 728 MB after 30
minutes, and private memory grew to 1 453 MB. A trim of a running process does
not hold, because the runtime's own timers and garbage collector touch the
heap again. Android uses the same order for the same reason: collect, freeze,
then compact.

What the platforms offer (web research of 2026-10-24, sources in the track's
notes):

- Windows has no documented call that suspends a process. `NtSuspendProcess`
  and the job-object freeze leave the processes suspended for good if the
  process that suspended them dies. A process state-change object
  (`NtCreateProcessStateChange`, Windows 11) resumes the process when its
  handle closes. A spike on 2026-10-24 (Windows 11 26300, a scratch child
  process) confirmed it: the child stopped while suspended, ran again when the
  handle was closed, and ran again when the process holding the handle ended
  without closing it. The call failed with `STATUS_ACCESS_DENIED` on a handle
  with `PROCESS_SUSPEND_RESUME | PROCESS_QUERY_INFORMATION` and succeeded with
  full access; the minimum right is not measured yet.
- Linux has the cgroup v2 freezer (`cgroup.freeze`), which no process can see,
  but only with a cgroup the daemon owns. `SIGSTOP` is always there, but a
  shell that leads the terminal sees its job stop and takes the terminal back.
  An unprivileged process cannot page out another one, and with no swap the
  heap cannot leave memory at all.
- macOS offers `SIGSTOP` only; `task_suspend` needs the task port. The memory
  compressor handles idle pages by itself.
- On WSL, memory freed inside the VM goes back to Windows only with
  `autoMemoryReclaim`, which is off by default.

Agent CLIs keep their conversation on disk and can open it again. What each
interactive CLI offers, read from its `--help` on 2026-10-24 (Windows 11):

| Agent | Version | Choose the id at launch | Resume one session by id |
| --- | --- | --- | --- |
| Claude Code | installed | `--session-id <uuid>` | `--resume <id>` |
| Copilot CLI | 1.0.91 | `--session-id <uuid>` (sets it for a new session) | the same `--session-id`, or `--resume <id>` |
| Gemini CLI | 0.63.0 | `--session-id <uuid>` | `--resume` takes `latest` or a list index; a UUID is not documented |
| OpenCode | 1.19.0 | no | `-s, --session <id>` |
| Codex CLI | 0.160.0 | no | `codex resume <uuid or session name>` |
| Kimi CLI | 0.29.0 | no | `-S, --session <id>` |
| Cursor agent | 2026.08.31 | `create-chat` returns a new id before launch (the headless adapter already uses it) | `--resume <chatId>` |

So Ralphy knows the id before launch for Claude, Copilot and Cursor. Gemini can take a chosen
id, but resuming it by that id needs a live check: a list index changes when
sessions are added or deleted. OpenCode, Codex and Kimi need the id to be found
after launch, by listing sessions.

The daemon today has no state on a session: it is in the map or gone
(`session/manager.rs`). It keeps no time of the last input. Every write and
resize goes through `ManagedSession::write` and `resize`
(`session/manager.rs:121`, `:135`). Sessions do not survive the daemon. The
page, not the daemon, relaunches a console (`wb-desk-folds.ts`
`reconcileDesk`). The screen emulator already produces the answer to a
cursor-position query and drops it (`session/screen.rs:221`).

## Decision

**D1. The daemon owns the cycle.** The daemon decides that a console is idle,
suspends it, hibernates it and reactivates it. Each daemon acts only on the
processes it hosts: a peer acts on its own consoles with its own platform's
means. The page and the CLI only ask for a reactivation and show the state.

**D2. A console has two inactive states.** *Suspended*: its process tree is
stopped, then its working set is trimmed; it comes back in under a second.
*Hibernated*: its process tree is ended, its screen and its agent session id
are kept, and the daemon relaunches the agent with its resume flag when the
console is reactivated. There is no trim-only state: it does not hold, it does
not exist on macOS, and it needs swap on Linux.

**D3. A console is idle when its activity clock passes the limit and no
blocker holds.** These reset the clock: input written to the PTY; a process
created or ended in the tree; the tree's CPU over a limit in a minute; the
tree's I/O over a limit in a minute. These block while they are true: the
agent is in a turn (agent state); a listening socket of the tree accepted a
connection in the window; the console is **Always on**. Being shown on a
screen is not activity. A long-lived child, such as an MCP server, is not
activity; only a change of the tree is. Every doubt counts as activity. The
CPU and I/O limits are internal values, tuned by measurement.

**D4. The evidence costs Ralphy nothing.** The daemon reads the process
table, CPU times, I/O counters and sockets from the operating system once a
minute, for all consoles in one pass. The only fact it adds is the time of the
last input, stored on the write path that already runs. It does not compare
screens: output to the PTY already shows as CPU and as I/O.

**D5. Each platform suspends with its safest means, and where only an unsafe
means exists, there is no Suspended.** Windows: a process state-change object
for each process of the tree, its handles held by the daemon; conhost is not
in the tree. Windows without that call: no Suspended. Linux: `cgroup.freeze`
when the daemon has a delegated cgroup, otherwise `SIGSTOP` to the process
group, and only when the agent CLI leads the terminal session with no shell
in between. macOS: `SIGSTOP`, with the same rule. After it suspends, the
daemon reads the tree again; a new process means it resumes all of them and
gives up. Just before it suspends or hibernates, it checks the evidence again.
The daemon never suspends a process that is not in its custody record (D6).

**D6. The daemon keeps a custody record of every console it hosts.** The
record is a file under `~/.ralphy/`, written atomically (a temporary file,
then a rename), on events that already happen: spawn, change of state, end.
For each console it holds every process of the tree as PID, creation time and
executable; the state; the suspend means; and, when known, the agent session
id with the repo, the checkout and the agent. A Hibernated console's screen is
a file of its own beside it. The daemon has no database for this. When the
daemon ends normally (a restart, an update, a shutdown of the machine), it
first saves every console's screen and marks every console of an agent with
a chosen session id as Hibernated; then it ends the trees. On start, the
daemon reads the record: an entry whose process is gone is dropped; a
process that is alive and matches all three values has its tree ended,
because an orphan has no terminal left to attach. Then every console comes
back as D13 says. A recovery is always an end and a resume. After a crash,
no screen was saved for a console that was not already Hibernated, so it
comes back as a **placeholder**.

**D7. Only an agent whose session id Ralphy chooses at launch can be
Hibernated.** Whether an agent accepts a chosen id and resumes it is a
capability of the daemon's `session::Agent` (an entry of the ADR-0040
inventory). It is turned on for an agent only after a live check of that
agent's interactive CLI. An agent whose id must be found after launch stops at
Suspended, because a sibling console in the same repo could take the wrong
conversation. A **free console** is never Hibernated. Text typed in the prompt
and not sent is lost when a console is Hibernated.

**D8. A click in the console reactivates it.** An inactive console shows its
last screen under a grey, translucent veil, with no cursor, no button and no
fixed text; a short hint shows on hover. A click in the console body, or a key
from the writer, reactivates it. Moving or resizing the window does not. A
resize does not reactivate: the operating system keeps the event (in conhost
on Windows, as a pending `SIGWINCH` on Unix) and delivers it on resume. A
watcher's keys stay dropped. For a Suspended console the click also does its
normal job and the key is delivered after the resume. For a Hibernated
console the click and the key only start the relaunch, and the veil stays
until the agent draws. `ralphy console reactivate <name>` does the same from
the CLI. Each change of state writes one line to the daemon log: the
evidence, who asked, and from where.

**D9. When no page is attached, the daemon answers the terminal's queries of
a relaunched agent.** On Windows a new child waits for the answer to a
cursor-position query. The daemon forwards its screen emulator's answer to
the PTY only while no client holds the **writer slot**, so the agent never
gets two answers.

**D10. The operator opts out per console, and the whole feature is opt-in.**
Each console has a small gear in its title bar that opens a menu: the home of
per-console settings. It holds **Always on** and the Restart action, which
leaves the title bar. Always on means "this console must always run": it is
never suspended nor hibernated, and the daemon starts it by itself when the
daemon starts (D13). It is off by default for every console. It is stored on
the desk record, like `locked`, and travels to a peer with the launch and
with each change. The daemon settings section "Consoles" holds: suspend idle
consoles (off by default), suspend after (2 hours), hibernate after (24
hours), include free consoles (off: free consoles are not suspended unless
this is on). These settings are per daemon.

**D11. A WSL free console is never suspended or hibernated.** It is a
`wsl.exe` process on the Windows host, and stopping it does not stop the
Linux processes inside. Its menu shows Always on checked and disabled, with a
hint. A peer with no swap says that suspending saves little memory there, and
the Windows daemon says so when `.wslconfig` has no `autoMemoryReclaim`.
Ralphy never changes the WSL configuration.

**D12. Every console start goes through one queue in the daemon.** A
relaunch, the reactivation of a Hibernated console, a click on a
placeholder, and an Always on console at daemon start all wait in one
queue. At most two consoles start at the same time; the number is an
internal value, tuned by measurement. The console the operator clicked goes
first, then the consoles shown on a screen, then the rest. A console that
waits shows its screen, or its placeholder, under the veil with a short
"Starting…" hint.

**D13. After the daemon starts, nothing starts without a click, except an
Always on console.** Every console comes back as its last saved screen under
the veil (a placeholder when no screen was saved). The hint says what a click
does. For an agent that resumes (D7), it continues the conversation the
screen shows. For any other console, it starts a new session, so the veil is
stronger and the hint says the session ended. The daemon starts each Always
on console by itself, through the queue, with no page open. The page no
longer starts a console when it loads, and the per-browser setting "Relaunch
agent consoles on load" is removed.

## Consequences

- A session gains a state beside live and gone. The pump's exit poll,
  `close`, the end reasons the page reads, and the session list sent to peers
  must all know it. New fields in the peer session list are `serde(default)`;
  the peer messages for a click and for Always on raise
  `PEER_PROTOCOL_VERSION`.
- The daemon, not the page, starts consoles after a daemon start. Today the
  page relaunches every free console when it loads, all at the same time,
  and every agent console too when "Relaunch agent consoles on load" is on
  (`wb-desk-folds.ts` `reconcileDesk`, `wb-desk-restore.ts`); each open
  browser does it on its own. On 2026-10-03 two pages that loaded 4 seconds
  apart started 8 shells for 4 records (ADR-0050, amendment of 2026-10-04).
  D12 and D13 end both: one owner, one queue, and a click for each console
  that is not Always on.
- A free console no longer comes back running by itself. An operator who
  wants a shell or a server back at once marks it Always on.
- The first click on a console after a daemon start takes a few seconds:
  the agent starts and loads its conversation. That is the price of not
  starting ten agents at once.
- D6 gives something beyond memory: after a crash, an update or a reboot, a
  console of an agent with a chosen session id comes back with its
  conversation.
- The Windows means is an undocumented `ntdll` call. A Windows release that
  removes it turns Suspended off; it cannot leave a process stopped.
- A spinner that burns CPU keeps its console active for good. That is the
  safe side of D3.

## Considered options

- **Trim only, with the process running.** Measured: the memory comes back
  within 30 minutes. Rejected (D2).
- **`NtSuspendProcess` or a job-object freeze on Windows.** If the daemon
  dies, the tree stays stopped and holds its memory with nobody to resume
  it. Rejected (D5).
- **Find the agent session id after launch, for every agent.** Two consoles
  in one repo can take each other's conversation, and the listing formats
  change between releases. Rejected for Hibernated (D7).
- **Compare screens to detect activity.** It costs work on every console and
  a spinner defeats it. CPU and I/O already show output (D4).
- **A SQLite store for the custody record.** Ralphy has no database of its
  own (`rusqlite` only reads the agents' databases), the record is tens of
  entries, and a database is a new kind of store. Reopen only if history and
  queries become a goal.
- **Keep "Relaunch agent consoles on load".** It is per browser, it turns
  every agent console on or off at once, and every open browser acts on it
  by itself. Always on is per console, kept by the daemon, and acted on
  once (D13).
- **A placeholder, not the old screen, for a console that cannot resume.**
  The screen shows where the operator was, which a placeholder does not. The
  stronger veil and the hint keep it from promising a conversation that is
  gone (D13).
- **Adopt the orphans after a daemon restart.** Their terminal ended with the
  daemon; there is nothing to attach to (D6).

## Compliance

- D1: not checked by code: manual, reviewed in the PR.
- D2: not checked by code: manual, reviewed in the PR.
- D3: not checked by code: behavior, pinned by the tests of the idle rule.
- D4: not checked by code: manual, reviewed in the PR.
- D5: not checked by code: needs each platform; the rule "never suspend a
  process outside the custody record" is pinned by a test of the suspend
  entry point.
- D6: not checked by code: behavior, pinned by the tests of the start-up
  pass.
- D7: not checked by code: needs a live agent CLI.
- D8: not checked by code: manual, reviewed in the PR.
- D9: not checked by code: manual, reviewed in the PR.
- D10: not checked by code: manual, reviewed in the PR.
- D11: not checked by code: manual, reviewed in the PR.
- D12: not checked by code: behavior, pinned by a test of the queue limit;
  every spawn site going through the queue is reviewed in the PR.
- D13: not checked by code: behavior, pinned by the tests of the page's
  restore fold (no start on load) and of the daemon start-up pass.

Open before the build: the minimum handle right for the Windows state-change
object; whether Gemini resumes a session by the id it was given; whether a launch gate per repo and agent
makes finding the id after launch safe enough for OpenCode, Codex and Kimi;
the CPU and I/O limits, from the observe-only slice.

## Amendment 2026-11-15 — hibernation first, as a policy over the process registry

Changes D2, D3, D4, D5, D6, D7, D8, D10, D11 and the list "Open before the
build"; adds D14 to D18. The rules not named here stay. The research behind
it:
[supervision](../research/idle-console-supervision-research-2026-11-15.md),
[Chromium](../research/chromium-process-management-research-2026-11-15.md),
[checkpoint](../research/process-checkpoint-restore-research-2026-11-15.md).
A review by the team on the same day moved Hibernated before Suspended.

**The use case.** An operator keeps about 50 agent consoles in the
workbench. They do not all start when the workbench opens; the operator
reactivates each one when needed. Over a week, the consoles in use pile up,
and many sit unused for days while they still hold memory. The machine has
little memory. Today the only way to get memory back is to close a console,
which loses it: the operator must create it again and find the conversation.
So the operator needs to see every console's state, memory and processes
(ADR-0088 D10); to make many consoles inactive at once, by hand, and to let
the daemon do it when memory is short, never while a console works; and to
reactivate one with a click, on the same conversation.

**What was measured on 2026-11-15** (Windows 11, WSL2 with systemd 249):

- On Windows, a trim of a frozen Node process took its working set from
  839 MB to 0 in 65 ms, and its 822 MB of commit stayed charged. This machine
  used 59 GB of a 76 GB commit limit. Only an ended process gives commit back.
- On Linux, writing to `memory.reclaim` of a frozen user-owned cgroup moved
  500 MB to swap in 0.5 s, with no privilege. The old sentence "an
  unprivileged process cannot page out another one" holds only for
  `process_madvise`.
- No tool can save a running agent CLI and restore it. Windows has none for a
  plain process. CRIU in WSL is stopped by the seccomp filter on every process
  and by the `io_uring` ring that Node opens. DMTCP aborts at start for Node
  22, Node 24 and Claude Code.

**D2, changed: Hibernated comes first.** Hibernated needs no undocumented
call, works the same on the three systems, gives back both memory and
commit, and survives a daemon restart. Suspended keeps commit, needs an
undocumented call on Windows and a delegated cgroup on Linux, and may break
the agent's connections (D14). The work is cut in three slices:

1. **Slice 1:** the process registry and its view (ADR-0088), and a manual
   **Deactivate** of one or many consoles (D8). Deactivate hibernates.
2. **Slice 2:** the memory trigger (D10). It hibernates.
3. **Slice 3:** Suspended, built only if the measurements of slices 1 and 2
   show that a fast return, or the agents that cannot hibernate, are worth its
   risk. Until then, D5 and D14 describe a state that does not exist.

There is no third state (D16).

**D3, changed.** A console is idle when its activity clock passes the limit
and no **idle blocker** holds. Activity is input written to the PTY, a
process that entered or left the container, or the container's CPU or I/O
over a limit in a minute (ADR-0088 D6). The blockers are one closed enum.
Each console has two sets of them, one for suspension (slice 3) and one for
hibernation:

| Blocker | Blocks Suspended | Blocks Hibernated |
| --- | --- | --- |
| `always_on` | yes | yes |
| `agent_working` (in a turn) | yes | yes |
| `agent_waiting` (ADR-0085 D17) | no | yes |
| `agent_state_unknown` (no turn source, D7) | yes | yes |
| `recent_input` | yes | yes |
| `tree_changed` | yes | yes |
| `cpu_or_io_active` | yes | yes |
| `socket_accepted` | yes | yes |
| `work_process_running` (ADR-0088 D7) | yes | yes |
| `no_container` (`process-group` mode) | yes | no |
| `no_resume` (no known session id, D7) | no | yes |
| `no_suspend_means` (the freeze call failed) | yes | no |
| `feature_off` | yes | yes |

The sets go to the console list, to `ralphy console show`, to the workbench
(a short hint in the gear menu, such as "Not deactivated: `npm run dev` is
still running"), and to the log line of each decision.

**D4, replaced** by ADR-0088 D6: the evidence comes from the container's
counters, read only while the feature is on.

**D5, changed, for slice 3 only.** If Suspended is built, it exists only in
`job` and `cgroup` modes:

- Windows: the job freeze (`JobObjectFreezeInformation`, an undocumented
  `ntdll` call) on the console's job, which has `KILL_ON_JOB_CLOSE`, so the
  tree ends if the daemon dies. This holds if closing the job ends its frozen
  members at once; if not, one process state-change object per process, with
  `PROCESS_SET_INFORMATION | PROCESS_SUSPEND_RESUME`. Then a trim, which frees
  memory but not commit.
- Linux: `cgroup.freeze`, a wait for `frozen 1`, then `memory.reclaim` when
  the system has swap.
- `process-group` mode: no Suspended.

If the call fails or is missing, that daemon turns Suspended off with
`no_suspend_means`. No path can leave a process stopped.

**D6, changed.** The custody record keeps, per console, the state, the agent
session id with the repo, the checkout and the agent, and the saved screen
beside it. PIDs are kept only in `process-group` mode (ADR-0088
Consequences). The rest of D6 stays.

**D7, changed.** Only a console whose agent session id Ralphy knows for
certain can be Hibernated. Ralphy knows it when it chose the id at launch,
or when the agent reported it from inside the console through Ralphy's hook
or plugin (the `session_id` of the status line, ADR-0085 D8). The automatic
trigger acts only on an agent with a turn source; without one, the console
has `agent_state_unknown`, and only a manual Deactivate acts on it. Each
capability is turned on only after a live check of the agent's interactive
CLI. As of 2026-11-15:

| Agent | Turn source | Session id | Hibernated |
| --- | --- | --- | --- |
| Claude Code | hooks | chosen (`--session-id`) | yes |
| Copilot CLI | none | chosen (`--session-id`) | by hand only |
| Cursor agent | none | chosen (`create-chat`) | by hand only |
| Antigravity CLI | hooks | reported (ADR-0086 D18) | yes |
| OpenCode | plugin | reported by the plugin | after a live check of `-s <id>` |
| Codex CLI | hooks | reported by the hook | after a live check of `codex resume <id>` |
| Kimi CLI | none | unknown | no |

A launch gate that finds the id by listing sessions is not used. The Gemini
CLI is gone (ADR-0086).

**D8, changed: the operator deactivates consoles by hand, one or many.** The
gear menu, the process view and `ralphy console deactivate` make an agent
console Hibernated when D7 allows it. The process view sorts by idle time and
by memory and allows a multiple selection with "Deactivate selected". The CLI
takes one console or `--idle-for <duration>`, shows what it will do, and acts
with `--yes`. A manual deactivate ignores the activity clock. For one
console, another blocker is named and asks for confirmation. For many, a
console with a blocker is skipped and listed. It is not offered on a free
console. The CLI refuses the verbs that act when it runs inside a console
(the launcher sets `RALPHY_CONSOLE_ID`, ADR-0088 D9). This guards against an
agent's mistake, not against an agent that wants harm: an agent runs as the
same user and can end processes by itself (ADR-0072). The rest of D8 stays.

**D10, changed.** The feature stays opt-in per daemon, and Always on stays.
The blockers always win.

1. **The memory trigger (slice 2).** The daemon watches the system's memory:
   on Windows both the physical-memory signal
   (`CreateMemoryResourceNotification`) and the commit charge against its
   limit; on Linux PSI `memory.pressure`. While a limit holds, it hibernates
   idle consoles that D7 allows, the largest first and then the oldest. The
   limits, and the amount the operator expects to get back, are set from the
   observe-only data (D17) before slice 2 is built.
2. **The idle-time trigger is off by default.** When the operator turns it on,
   its limit is a setting, and choosing it is a choice. The break-even time
   `T_BE = B / (ΔM · v)` (rent-or-buy, Karlin et al., 1988) explains why a
   limit near it is reasonable, but `v`, the value of a gigabyte-hour, cannot
   be measured, so it gives no default. `B` holds the operator's wait and the
   risks: a resume that fails, text not sent, a broken connection.

The setting "include free consoles" is removed.

**D11, withdrawn.** The policy applies only to agent consoles. A free console
is never Suspended or Hibernated. It still has a container and a place in the
process view (ADR-0088), and Always on for it means only that the daemon
starts it when the daemon starts (D13).

**D14. After a resume from Suspended, the first network call may fail once**
(slice 3). A frozen agent cannot answer an HTTP/2 PING, so its server may
close an idle connection. Ralphy does not repair the agent's connections. An
agent that needs a manual step after a freeze would skip Suspended.

**D15. The idle-time limit can later follow measured idle periods.** When the
observe-only data has weeks of idle periods per console, a later amendment
may use the rule "act when the chance per hour that the operator returns
falls below the cost ratio".

**D16. Process checkpoint is not a state.** Ralphy does not save a running
agent and restore it (see the measurements above).

**D17. Measure before building.** Before slice 1 is built, an observe-only
pass over the operator's consoles records: the mix of agents, the private
memory and commit of each console, the share of consoles with long-lived
children, and the idle periods. Slice 2 is planned from these numbers.

**D18. A process that asks for breakaway leaves its console.** The console's
job allows breakaway on request (ADR-0088 D2), so `ralphy daemon restart` run
inside a console starts a daemon that outlives that console. Such a process
is not in the registry and is not deactivated with the console.

**Considered options, added.**

- **Checkpoint and restore of the process (CRIU, DMTCP, a VM snapshot).**
  Rejected by the measurements above. A whole VM per console is a new kind of
  product, and a restored process comes back on a new terminal with dead
  connections, which the agent's own resume handles better.
- **Suspended first.** It holds all the technical risk (an undocumented call,
  cgroup delegation, broken connections), it does not give commit back, and
  its own gain is a fast return and three agents. Hibernated first gives most
  of the memory with none of that risk.
- **A launch gate per repo and agent.** It finds the id by listing sessions,
  whose format changes between releases, and a mistake gives a console
  another console's conversation.
- **Fixed limits of 2 hours and 24 hours, or a default derived from
  `T_BE`.** Both are choices; `v` cannot be measured. The idle-time trigger is
  off by default instead.
- **Free consoles under the policy.** The operator's case is long-idle agent
  consoles; a WSL free console cannot be frozen from Windows at all.

**Compliance, added.**

- D2, 2026-11-15: not checked by code: manual, reviewed in the PR.
- D3, 2026-11-15: not checked by code: behavior, pinned by the tests of the
  blocker sets; the enum is matched exhaustively.
- D5, 2026-11-15: not checked by code: the freeze entry point takes a
  container (ADR-0088 D5), pinned by its test; the FFI module is reviewed in
  the PR.
- D7, 2026-11-15: not checked by code: needs a live agent CLI.
- D8, 2026-11-15: not checked by code: behavior, pinned by the tests of the
  selection and the refusal inside a console.
- D10, 2026-11-15: not checked by code: behavior, pinned by the tests of the
  trigger order.
- D14 to D18: not checked by code: manual, reviewed in the PR.

**Open before the build, replaced.** Measured on Windows 11 and WSL, and on
macOS when a Mac is at hand:

- for slice 1: the observe-only data of D17; a live check that OpenCode
  resumes the `session_id` its plugin reports, and Codex the one its hook
  reports; `ralphy daemon restart` run inside a console (D18); a shared daemon
  (sccache, Gradle, tsserver) started by one console and used by another;
- for slice 2: how often the memory signals fire with the usual load; the
  memory and commit each hibernation gives back, per agent; the time from a
  click to a usable prompt, per agent, and the share of resumes that fail;
- for slice 3, only if it is planned: whether closing a frozen
  `KILL_ON_JOB_CLOSE` job ends its members at once; the time from a click to
  the first echoed key, with the pages already written to disk; what each
  agent does with its first call after 10 minutes, 1 hour and 4 hours frozen;
  the CPU and I/O limits of D3.

## Amendment 2026-11-16 — a state that is not known blocks, and hibernation is an operation with steps

Changes D3, D6, D7, D8, D10 and D18 and two sentences of the amendment of
2026-11-15; adds D19 and D20. It comes from two independent reviews of
2026-11-16 and from ADR-0088's amendment of the same day. The rules not
named here stay.

**Two sentences, corrected.**

- "Only an ended process gives commit back" is too strong. A process can
  give back its own commit. No supervisor can do that for another process,
  so ending the tree is the means the daemon has.
- "No tool can save a running agent CLI" is more than was measured. In WSL,
  CRIU was stopped before it made an image, and DMTCP aborted. The research
  also records a report of CRIU restoring Node.js 24 on Fedora. D16 stays:
  no means was shown that is supported and works for the agents and the
  systems tested.

**D3, changed.**

- Until slice 3 is planned, a console has one set of blockers, the one for
  hibernation. The second set, `no_container` and `no_suspend_means` come
  with slice 3.
- `socket_accepted` is removed. The registry reads no socket (ADR-0088). A
  server that an agent left running is a work process.
- `agent_state_unknown` holds whenever the daemon cannot say what the agent
  does: the console has no turn source, the source is silent, no state has
  arrived yet, or the state is `unknown` because a `working` state grew old
  (ADR-0085). It always blocks the trigger. So a Claude console that the
  operator interrupted is not deactivated by the trigger; the operator
  deactivates it by hand. The survey counts these consoles.
- A state counts only for the run of the console that reported it.
- Three blockers are conditions that a confirmation cannot pass.
  `no_resume`: ending a console that cannot resume is End, not Deactivate.
  `always_on`: the operator turns Always on off first. A state of
  Deactivating or Starting (D19).
- `feature_off` turns off the triggers only. A manual Deactivate does not
  need the feature.
- The CPU and I/O limits are needed by slice 2, so they are measured before
  it, not with slice 3.

**D6, changed.** The sentence "an entry whose process is gone is dropped"
is about the process, not the console. The record of a console stays until
the operator ends the console.

**D7, changed.**

- The session id that Ralphy keeps is the *current* one. An operator can
  change the conversation inside the agent (`/clear`, `/resume`), and the
  turn source then reports a new lead session. The daemon writes the lead
  session's id to the custody record each time it changes.
- With no turn source (Copilot, Cursor), Ralphy knows only the id it chose
  at launch. If the operator changed the conversation inside the agent, a
  reactivation opens the one of the launch. The hint of Deactivate says so.
- On a reactivation the daemon gives the agent state the id from the custody
  record. An agent may not report it again: `agy` sends no `SessionStart`
  for a resumed conversation.
- The capability belongs to the console, not to the agent. A Codex console
  in a project with its own hooks has no turn source.
- `agy` reports no wait for a permission. Its state stays `working` and then
  becomes `unknown`; both block, so the trigger does not end it while it
  waits.
- "Kimi: no" means that Ralphy does not learn Kimi's session id. Kimi can
  resume a session by id.
- Before OpenCode is turned on: whether its consoles share one server
  process, in the version installed. If they do, a console's tree is not the
  conversation's only process.

**D8, changed.** After a reactivation, the veil goes away when the agent has
drawn visible text, not at the first bytes from the PTY. If the agent ends
before that, D19 applies. The key that reactivated a Hibernated console is
not sent to the new agent.

**D10, changed: the memory trigger is a controller with limits.**

- It watches the resource that is short and orders the consoles by what
  each gives back of that resource: private commit when commit is short,
  memory in use when memory is short.
- On Linux the signal is the available memory and the free swap. PSI
  `memory.pressure` is a second signal only: it measures the time tasks
  wait, and it reads zero while idle consoles fill the memory.
- It deactivates one console, waits, and measures again. It starts above
  one limit and stops below a lower one. A console that the operator
  reactivated is not deactivated again for a time. The limits, the wait and
  that time come from the survey (D17).
- When no console can be deactivated, it does nothing and says so in the log
  and in the workbench.
- A WSL peer cannot see the memory of the Windows host. When the Windows
  daemon is over its limit, it asks its local WSL peer to run one round. The
  peer still decides and acts on its own consoles (D1). How much of that
  memory goes back to Windows is measured.
- macOS has no trigger yet. Deactivate is by hand there.
- The trigger first ships as a record only: it logs what it would
  deactivate, and the operator compares that with what happened.
- `T_BE` is no longer part of the decision. It was an analogy: its bound
  holds only when the costs have one unit, and here they do not. The
  idle-time trigger stays off by default, and its limit is a plain setting.

**D18, changed: a restart asked from inside a console is done from outside
it.** `ralphy daemon restart` stops the old daemon before it starts the new
one (`crates/ralphy-cli/src/daemon/restart.rs`). Inside a console, the
command ends with the old daemon, before it starts anything, so breakaway
alone does not help. The command first starts a coordinator outside the
console's container: with breakaway on Windows, in its own session in
`process-group` mode. The coordinator stops the old daemon and starts the
new one. Where systemd or launchd owns the daemon, it does the restart. The
command inside the console ends with its console, and the operator reads
the result from the daemon. A process that breaks away still leaves its
console.

**D19. Hibernation is an operation with steps, and it can fail.**

1. The daemon marks the console Deactivating in the custody record and
   takes no more input for it.
2. It reads the members again and checks the blockers again. A change stops
   the operation.
3. It saves the screen and the current session id.
4. It ends the container.
5. It waits until the container is empty. If the wait ends first, the
   console shows an error and is not Hibernated.
6. It marks the console Hibernated.

A daemon that starts and finds a console Deactivating finishes the
operation: Hibernated when the screen and the id were saved, a placeholder
when not. When a resume fails (the agent ends before it draws), the console
stays Hibernated and shows the error. The daemon never opens a new
conversation in its place.

**D20. Whether the agent is asked to exit before step 4 is measured, per
agent.** Ending the container is a kill. The measure is whether the
conversation resumes, with its last turn, after a kill. An agent that loses
data gets its own exit step, with a time limit, before the kill.

**Considered options, added.**

- **Let the trigger act on an old `unknown` state when CPU and I/O are
  zero.** It would free the consoles that the operator interrupted. It would
  also end an agent that waits for the network with no CPU. The survey
  counts the consoles first.
- **Make `agy` manual only.** The rule for `unknown` already keeps the
  trigger away while `agy` waits.
- **A grace period for every agent.** The agents' stores are made to
  survive a crash. D20 measures before it adds a wait to every Deactivate.

**Compliance, added.**

- D3, D7, 2026-11-16: not checked by code: behavior, pinned by the tests of
  the blocker set and of the session id in the custody record.
- D10, 2026-11-16: not checked by code: behavior, pinned by the tests of the
  controller's rounds.
- D18, 2026-11-16: not checked by code: needs a live daemon.
- D19: not checked by code: behavior, pinned by the tests of each step's
  failure and of the start-up pass.
- D20: not checked by code: needs a live agent CLI.

**Open before the build, added.**

- for slice 1: the conversation after a kill, per agent (D20); a restart
  from inside a console in each container mode (D18); a change of
  conversation inside each agent, and the id the turn source reports (D7);
  whether OpenCode consoles share a server; a reactivation while a hidden
  browser tab holds the writer slot (D9);
- for slice 2: the CPU and I/O limits of D3; the consoles in
  `agent_state_unknown`, per agent; the memory the Windows host gets back
  when the WSL peer deactivates a console.
