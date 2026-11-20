# The daemon keeps a process registry, one container per console

Status: proposed
Kind: structural
Protects: observability and cost, recoverability; constraints portability, security

Amends ADR-0083 (proposed): its D4 and the process part of its D6 move here,
and its suspend means act on the container this ADR creates.
Amendment of 2026-11-16 below: what each container mode can tell.

## Context

An operator keeps about 50 agent consoles in the workbench, on a machine with
little RAM. To decide which console to put away, and to let the daemon do it
(ADR-0083), the operator and the daemon must both know which processes each
console holds and what they cost. Today nobody knows. The daemon keeps only
the root PID of each PTY (`crates/ralphy-daemon/src/session.rs`, `close`). It
finds the rest of the tree only when it ends it, from a system-wide Toolhelp32
snapshot on Windows or the process group on Unix
(`crates/ralphy-proc-util/src/lib.rs`, `kill_tree_by_pid`). An agent CLI starts
children that Ralphy never launched: shells, test runners, MCP servers, dev
servers.

Research of 2026-11-15, with sources:
[supervision](../research/idle-console-supervision-research-2026-11-15.md),
[Chromium](../research/chromium-process-management-research-2026-11-15.md),
[checkpoint](../research/process-checkpoint-restore-research-2026-11-15.md).
What it found:

- Mature supervisors do not rebuild a tree from periodic snapshots. They put
  the tree in a kernel container that holds every child from its creation:
  systemd tracks a unit by its cgroup. A Windows job object does the same, and
  it gives process events on an I/O completion port, a member list, and CPU and
  I/O totals that include processes that already ended. Microsoft says the
  completion port messages are not guaranteed, so a resync is still needed.
- `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` ends the tree when the last job handle
  closes, so the death of the daemon ends its consoles with no record.
- `portable-pty` 0.9 cannot put the child in a job at creation: it passes
  only the pseudoconsole attribute and no `CREATE_SUSPENDED`. A job assigned
  after the spawn misses the children of the first milliseconds.
- On the WSL peer (measured), the daemon's systemd user unit owns its cgroup
  files. The unit has no `Delegate=yes`, so the daemon cannot make child
  cgroups. systemd is 249, so `DelegateSubgroup=` is not there.
- macOS has no process container for a plain user. kqueue has no
  `NOTE_TRACK`.
- Chromium keys a process by its own id and keeps the PID as data, "because
  PIDs are reused aggressively". It samples metrics only while a viewer asks
  for them. One model of processes serves both its policies (freeze, discard)
  and its Task Manager window.
- Kubernetes keeps a periodic relist beside its process events: events are an
  optimization, not the source of truth (KEP-3386).

## Decision

**D1. Each daemon owns one process registry.** It is the only owner of the
fact "which processes belong to which console, and what they cost". The page,
the CLI and the ADR-0083 policy read it; none of them lists processes by
itself.

**D2. Each console has one container, and its kind is a fact.** The
**container mode** of a console is one of:

- `job`: on Windows, a job object made for the console, with
  `KILL_ON_JOB_CLOSE`, a completion port, and breakaway only on request
  (`JOB_OBJECT_LIMIT_BREAKAWAY_OK`, never the silent form). `ralphy daemon
  restart` asks for breakaway (`spawn_outside_job` in
  `crates/ralphy-cli/src/daemon/restart.rs`); a job that refused it would
  start the new daemon inside the console's job, and closing the old daemon
  would end it.
- `cgroup`: on Linux, when the daemon runs in a systemd unit with
  `Delegate=yes`. The daemon moves itself into the leaf `daemon` of its unit
  cgroup and makes one leaf `console-<id>` per console.
- `process-group`: everywhere else. That is macOS, and Linux when the daemon
  is not in a delegated unit (started from a terminal, WSL without systemd,
  `wsl.exe -e`). The console is a process group, and the daemon sets
  `PR_SET_CHILD_SUBREAPER` on Linux so an orphaned child comes back to it.

The Linux unit that `ralphy daemon install` writes gains `Delegate=yes`. The
container mode shows in `ralphy doctor` and in the workbench.

**D3. A console's first process enters the container before it can start a
child.** The daemon does not launch the agent itself. It launches the
`ralphy` binary in an internal launcher mode with the console id and the agent
command line:

- On Windows, the launcher assigns itself to the console's job, then starts
  the agent, which inherits the job. It waits for the agent and exits with its
  exit code.
- On Linux in `cgroup` mode, it writes its own PID to the console's
  `cgroup.procs`, then `exec`s the agent.
- In `process-group` mode, it `exec`s the agent.

`portable-pty` is not forked or patched. If the measurement in "Open before
the build" fails the launcher, the fallback is Ralphy's own ConPTY spawn in
`ralphy-pty` with `PROC_THREAD_ATTRIBUTE_JOB_LIST`, on Windows only.

**D4. A console is keyed by its console id, never by a PID.** Across daemons
the key is `<daemon_id>/<console_id>`, as in `fleet.rs`. The registry holds an
open handle only on the container (the job, or the cgroup directory) and on
the console's root process. Every other process is a row of data: PID,
creation time, executable, parent PID, and the times it entered and left.

**D5. Every action is on the container.** To end a console, the daemon ends
its container: `TerminateJobObject`, `cgroup.kill`, or a signal to the process
group whose leader it holds. The ADR-0083 freeze acts on the container too.
The daemon never signals or terminates a PID that it read from a list.

**D6. The registry reads the operating system at three rhythms.**

1. Events, always on: a process enters or leaves (the job's completion port
   messages, the `populated` change of `cgroup.events`, kqueue `NOTE_EXIT` on
   the root).
2. A resync of each container's member list (`JobObjectBasicProcessIdList`,
   `cgroup.procs`, the process group) every 60 seconds, when a view opens, and
   just before any action. A difference from the events goes to the daemon
   log.
3. Sampling, only while an observer asks for it. The first observer starts
   it and the last one stops it. The interval is the smallest one asked for,
   and the metrics are the union of the ones asked for. There are two kinds of
   observer. One is the ADR-0083 policy, only while that feature is on: one
   reading of each container's totals (CPU, I/O, memory) every 60 seconds. The
   other is a process view that is open: per-process metrics every 2 seconds,
   only the columns it shows.

With the policy off and no view open, the registry costs its events and one
resync a minute.

**D7. A process is either part of the agent or a work process.** The agent
state (`agent_state.rs`) says when a turn starts and ends. A process born
outside a turn, such as an MCP server the agent starts at launch, is part of
the agent. A process born during a turn and still alive after the turn ended
is a **work process**: a dev server, a watcher, a job left in the background.
When the registry cannot tell, the process is a work process.

**D8. Each daemon sends its consoles' summary to the daemon the page talks
to.** The summary of a console is its state, container mode, memory, process
count, time of last activity, and its ADR-0083 idle blockers. It travels in
the console list (`/api/sessions`). A peer pushes a change on its existing
socket to that daemon, with a revision number per daemon. The 30-second read
of the peer stays, as the resync. A gap in the revisions forces a full read. A
peer that does not answer is named by `x-ralphy-unanswered`, as today, and the
workbench shows its consoles as unknown, with the time of the last good read.
The per-process detail of a peer's console is sent only while a view of it is
open: the daemon of the page registers a remote observer on the peer and
removes it when the view closes or the socket drops. Each daemon acts only on
its own containers. The new fields are `serde(default)`, and
`PEER_PROTOCOL_VERSION` goes up.

**D9. Agents and scripts read the registry through the CLI.** `ralphy console
list`, `ralphy console show <console>` and `ralphy console processes
<console>` print the registry, with `--format json`, and change nothing. An
agent in a console calls them as it calls `git`. It never calls the HTTP API
with the daemon's token (ADR-0072 D7). The verbs that act (`ralphy console
end`, and the ADR-0083 verbs) are for the operator: they ask for
confirmation or take `--yes`, and the daemon logs who asked and from where.
The launcher sets `RALPHY_CONSOLE_ID` in each console, and the CLI refuses
the verbs that act when it is set. This guards against an agent's mistake,
not against an agent that wants harm: an agent runs as the same user and can
end processes by itself (ADR-0072). This ADR adds no MCP server.

**D10. The workbench shows a process view of every console.** It covers every
console, free consoles too. It shows memory, CPU, the processes with their
kind (D7), the time of last activity and the container mode. Its End action
ends the whole tree, and its text says so.

**D11. The registry adds no crate.** It uses `windows-sys` (new features of
the same version) and `libc`, which the workspace already has. The calls that
the operating system makes in a container's name live in `ralphy-proc-util`.
The registry and its observers live in `ralphy-daemon`, and the launcher mode
lives in `ralphy-cli`. A new crate needs a measured reason in an amendment.

## Consequences

- The ADR-0083 custody record no longer needs PIDs for safety in `job` and
  `cgroup` modes. The death of the daemon ends every console there: by
  kill-on-close on Windows, and by `KillMode=control-group` on Linux. In
  `process-group` mode, the record still keeps the root's PID, creation time
  and executable, and the daemon ends a matching orphan group when it starts.
- Each console on Windows has one more small process, the launcher.
- An installed Linux unit without `Delegate=yes` runs in `process-group`
  mode until the unit is written again (`ralphy daemon install`,
  `scripts/update-ralphy.ps1`).
- A process that asks for breakaway leaves the console's job and the
  registry. Today only `ralphy daemon restart` asks for it.
- A shared daemon (a build cache, a language server) that one console starts
  lives in that console's container. It counts as a work process and keeps
  that console from being idle, and ending the console ends it for the other
  consoles that use it. How often this happens is measured before the build.
- On macOS and in Linux `process-group` mode, a child that leaves the
  process group by calling `setsid` leaves the registry. The registry says so
  through the container mode; it does not claim a full tree there.

## Considered options

- **One read of the system process table a minute (ADR-0083 D4).** It misses
  a child that is born and ends between two reads, and a PID can be reused
  between them. A container sees both.
- **Track each birth from system-wide events.** ETW kernel process events
  need administrator rights. The Linux proc connector needs `CAP_NET_ADMIN`.
  kqueue without `NOTE_TRACK` races the fork. A container needs no event to be
  complete.
- **Fork or patch `portable-pty`.** It is a vendored library to maintain
  through every upstream release (ADR-0072). The launcher needs no change to
  it.
- **The `sysinfo` crate.** It reads the whole process table and matches by
  PID, which is the model this ADR replaces. On Windows it brings the
  `windows` and `ntapi` crates, which the workspace does not have. The few
  per-process calls fit in `windows-sys` and `libc`.
- **A job with no breakaway at all.** `ralphy daemon restart` run inside a
  console would start a daemon that dies with the old one, a failure the
  restart code already records.
- **A handle or pidfd on every process.** A test runner can start hundreds of
  processes a minute. Actions are on the container, so a handle per process
  buys nothing.
- **Push from peers with no resync.** Events get lost. Kubernetes kept its
  relist for this reason.
- **An MCP server for agents.** The CLI already reaches the daemon, and an
  MCP server can later wrap the same verbs.

## Compliance

- D1: not checked by code: manual, reviewed in the PR.
- D2: not checked by code: needs each platform; the unit text with
  `Delegate=yes` is pinned by the autostart tests.
- D3: not checked by code: needs a live ConPTY; reviewed in the PR.
- D4: not checked by code: manual, reviewed in the PR.
- D5: not checked by code: pinned by a test of the end and freeze entry
  points, which take a container and no PID.
- D6: not checked by code: behavior, pinned by the tests of the observer
  count.
- D7: not checked by code: behavior, pinned by the tests of the process kind.
- D8: not checked by code: behavior, pinned by the peer protocol tests.
- D9: not checked by code: manual, reviewed in the PR.
- D10: not checked by code: manual, reviewed in the PR.
- D11: checked by `cargo-machete` and the lockfile review for unused or new
  crates; the crate placement is reviewed in the PR.

Open before the build, each measured on Windows 11 and WSL:

- whether a process inside a ConPTY can assign itself to a job, and whether
  Ctrl+C, resize and the cursor answer of ADR-0083 D9 still work through the
  launcher (decides D3 or its fallback);
- how many completion port messages a job loses under load, with 1 000 short
  children (decides the resync period of D6);
- whether the WSL daemon can move itself into a leaf of its delegated unit
  under systemd 249 (decides D2 there);
- the share of MCP servers that each agent starts during a turn, from each
  agent's tree at rest (tells how often D7 keeps a console from being idle);
- `ralphy daemon restart` run inside a console, with the job of D2 (proves the
  breakaway);
- a shared daemon (sccache, Gradle, tsserver) started by one console and used
  by another (tells how often it blocks a console or is ended under another).

## Amendment 2026-11-16 — what each container mode can tell, and the job handle

Changes D2, D3, D6, D7 and D8, corrects one sentence of the Context, and adds
D12. It comes from two independent reviews of 2026-11-16. The rules not named
here stay.

**Context, corrected.** Chromium has two models of processes, not one. Its
Performance Manager serves the freeze and discard policies, and its Task
Manager is a separate, older system (the Chromium research, finding 5). The
comparison supports one thing only: the fact is kept apart from the policy.

**D2, changed.**

- `job`: the job handle lives only in the daemon. After the agent starts, no
  other process holds one, because `KILL_ON_JOB_CLOSE` acts when the *last*
  handle closes. A console whose process could not be put in the job is not
  started.
- `cgroup`: the order is fixed. The daemon makes the leaf `daemon`, moves its
  own PID there, writes `+memory +pids` to `cgroup.subtree_control` of the
  unit cgroup, and then makes the `console-<id>` leaves. Without that write a
  console leaf has no `memory.*` file. A controller that the user manager
  does not delegate is not there: on the WSL peer (measured) it delegates
  `memory` and `pids`, so a console leaf has no `io.stat`. If a step fails,
  the daemon runs in `process-group` mode and says so.
- `KillMode=control-group` ends the consoles when the unit stops. It is a
  rule of the unit, so it is checked on the installed unit, not assumed.
- `PR_SET_CHILD_SUBREAPER` brings an orphan back to the daemon. It does not
  hold a child that calls `setsid`.

**D3, changed: on Windows the means is chosen by a spike that builds both.**
The two are the launcher, and Ralphy's own ConPTY spawn with
`PROC_THREAD_ATTRIBUTE_JOB_LIST`, which needs no launcher process. D3 did not
say how the launcher gets the job handle; a handle that a second console
inherits breaks D2. The one that is built must show all of these:

- no process but the daemon holds the job handle after the agent starts;
- a failed assignment ends the launch;
- Ctrl+C reaches the agent and does not end the console;
- the refusal of an unsafe batch command line (`batch_refusal` in
  `ralphy-pty`) still runs on the agent's command line;
- the processes and the memory it adds per console, measured.

On Linux the launcher stays: it `exec`s the agent and leaves no process.

**D6, changed: only a job reports each process.**

| Container mode | Event | A child born and ended between two resyncs |
| --- | --- | --- |
| `job` | each process that enters or leaves | has a row |
| `cgroup` | `populated`: the container became empty or not empty | has no row; its CPU is in the total |
| `process-group` | the root ended | has no row |

So in `cgroup` and `process-group` modes the member list comes from the
resync. A resync of one console also runs when its agent's turn starts and
when it ends. In `cgroup` mode the CPU total comes from `cpu.stat`, which
needs no controller, and the I/O is the sum of `/proc/<pid>/io` over the
members alive at the sample; the I/O of a process that already ended is not
counted there. A fact that could not be read is unknown, never zero. CPU
needs two samples; after the first one it is unknown. The summary says how
old each fact is.

**D7, changed: the kind comes from the creation time.** The registry compares
the creation time the operating system gives for a process with the turns the
agent state recorded. This needs no event per process. A process whose turn
is not known is a work process. An agent's own helper that starts on first
use (a language server, an MCP server) is born in a turn and is a work
process by this rule. No rule exempts it yet: how often it happens, per
agent, is measured first.

**D8, changed.**

- The revision travels with the id of the daemon's run. A new run forces a
  full read, because a daemon that starts again counts its revisions from
  the start.
- A field that a peer does not send means unknown. It never means that a
  console can be deactivated.
- A remote observer has a lease. The 30-second read renews it. A peer stops
  the 2-second sampling by itself when the lease ends, so a socket that is
  dead but not closed does not keep it running.

**D12. The registry says what it can tell for each console.** Beside the
container mode, the summary holds: whether the tree is complete, whether
each process is reported, and which totals exist (CPU, I/O, memory). The
ADR-0083 policy reads these, and treats a missing one as unknown.

**Left open.** Ending one work process and not the whole console. D5 forbids
it. A handle or a pidfd, opened and then checked against the container and
the creation time, would make it safe. It is decided when the process view
is built.

**Considered options, added.**

- **Exempt known helpers from the work-process rule now.** A list of names,
  or "no listening socket and no CPU", is a guess about agents Ralphy does
  not control. The survey gives the numbers first.
- **Decide for the own ConPTY spawn without a spike.** It removes the
  launcher's three problems on Windows, but it rewrites the spawn that
  `portable-pty` does today, in `unsafe` FFI (ADR-0072 D13). Its cost is not
  measured.

**Compliance, added.**

- D2, 2026-11-16: not checked by code: needs each platform; the order of the
  cgroup steps is pinned by a test of the function that makes a leaf.
- D3, 2026-11-16: not checked by code: needs a live ConPTY.
- D6, D7, 2026-11-16: not checked by code: behavior, pinned by the tests of
  the registry rows and of the process kind.
- D8, 2026-11-16: not checked by code: behavior, pinned by the peer protocol
  tests.
- D12: not checked by code: behavior, pinned by a test that a missing fact
  reads as unknown.

**Open before the build, added.**

- the two Windows means of D3, against its five points;
- the files a `console-<id>` leaf has on the WSL peer after the steps of D2;
- the share of consoles, per agent, whose helper is a work process by D7.
