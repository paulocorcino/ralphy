# An idle console is suspended, then hibernated

Status: proposed
Kind: structural
Protects: responsiveness, recoverability; constraints portability, security

Amended by ADR-0085 (proposed: a `waiting` agent blocks hibernation, not suspension).

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
|---|---|---|---|
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
