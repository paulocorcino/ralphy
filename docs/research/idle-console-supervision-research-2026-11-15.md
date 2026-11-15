# Idle consoles: process supervision, safe freeze, timeout theory (2026-11-15)

Web research for [ADR-0083](../adr/0083-an-idle-console-is-suspended-then-hibernated.md). It
builds on the earlier note `.ralphy/docs/console-idle-research-web.md` (2026-10-24, local and
ignored by git), which covered the Windows suspend calls, the cgroup v2 freezer against
`SIGSTOP`, the Android freezer, the browser tab rules, CRIU and the agent CLI resume flags. This
note does not repeat that material. It answers five questions the ADR left open: how to track
every process a console starts, how to freeze a tree safely, where the idle timeouts come from,
which production lifecycle models compare, and how a remote controller learns the state of a
node.

Labels on each claim:

| Label | Meaning |
| --- | --- |
| **[docs]** | From the owner's documentation or source code. |
| **[paper]** | From a peer-reviewed paper. |
| **[reported]** | From a third party: a reverse-engineering reference, a forum, a fork. |
| **[inferred]** | My own reasoning from the facts here. Not checked. |
| **[measured]** | A read-only check run on this machine on 2026-11-15. |

No process was stopped, signalled or suspended for this note. The local checks only read
`/proc`, `/sys/fs/cgroup` and `systemctl show` inside WSL, and the source of `portable-pty` in
the local cargo registry.

---

## Findings that change the design

1. **Do not track births. Let the kernel contain them.** A Windows job object and a Linux
   cgroup v2 both put every child into the container at creation, with no race and with no
   event that the supervisor must catch. The supervisor only reads the membership list of one
   console, which is not a system-wide snapshot. Mature supervisors work this way: systemd
   tracks a service by its cgroup, and node-pty, VS Code's PTY library, asks the console
   itself which processes are attached. ADR-0083 D4 and D6 instead rebuild the tree from a
   system-wide process table once a minute, and match PIDs by creation time. That model is
   the weakest of the options. [docs, inferred] (sources in Q1)
2. **On Windows, one job object per console gives the registry, the CPU and I/O totals, the
   kill, and the cleanup after a crash.** `JOB_OBJECT_MSG_NEW_PROCESS` and `EXIT_PROCESS` on an
   I/O completion port give the events. `JobObjectBasicProcessIdList` gives the resync list.
   `JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION` gives the tree's CPU time and I/O counts,
   with the processes that already ended included. `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` ends
   the whole tree when the daemon dies. That last point does on its own what D6 does on
   start ("an orphan has no terminal left; end its tree"). Microsoft says the completion port
   messages are not guaranteed, so a resync is still needed. [docs]
3. **The job freeze is safe against a daemon crash when the job has
   `KILL_ON_JOB_CLOSE`.** The old research and ADR-0083 rejected the job freeze because "if
   the last handle closes, the tree stays frozen". With kill-on-close, the close of the last
   handle ends the tree instead. D6 already ends orphans at start, so the end state is the
   same as with the state-change object, and the job freeze has no race and also freezes
   threads and processes born later. Both calls are undocumented `ntdll` calls. This needs a
   spike before it replaces the state-change object. [docs for kill-on-close; reported for the
   freeze; inferred for the combination]
4. **The minimum right for `NtCreateProcessStateChange` is most likely
   `PROCESS_SET_INFORMATION`, plus `PROCESS_SUSPEND_RESUME` for `NtChangeProcessState`.** The
   phnt-based reference says so. The ADR's spike failed with
   `PROCESS_SUSPEND_RESUME | PROCESS_QUERY_INFORMATION`, which lacks
   `PROCESS_SET_INFORMATION`, so the failure agrees with it. [reported; consistent with the
   ADR's measurement] <https://ntdoc.m417z.com/ntcreateprocessstatechange>
5. **`portable-pty` 0.9 cannot put the child into a job at creation.** It calls
   `CreateProcessW` with only `EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT` and one
   attribute (`PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`), and it closes the main thread handle. So
   neither `PROC_THREAD_ATTRIBUTE_JOB_LIST` nor `CREATE_SUSPENDED` plus a late resume is
   possible without a patch. A job assigned after the spawn misses any child the agent starts
   in its first milliseconds. [measured: `portable-pty-0.9.0/src/win/psuedocon.rs`] The same
   file passes `PSEUDOCONSOLE_INHERIT_CURSOR`, which is why a new child waits for a
   cursor-position answer (ADR-0083 D9). [measured]
6. **The WSL peer already runs in a cgroup that the user owns, but systemd has not delegated
   it.** The peer daemon and all its `claude` children are in
   `user@1000.service/app.slice/ralphy-daemon.service`; `cgroup.freeze` there is owned by the
   user; the unit has `Delegate=no` and `KillMode=control-group`. `user@1000.service` itself
   has `Delegate=yes` with `memory pids`. systemd is version 249, so `DelegateSubgroup=`
   (version 254) is not there. A process started by `wsl.exe -e` lands in `/non-systemd`,
   which root owns. So `Delegate=yes` on Ralphy's own unit, with one sub-cgroup per console,
   gives the cgroup freezer on this machine with no new privilege. [measured, docs]
7. **The idle timeout has a known theory, and it gives a formula, not a constant.** A console
   pays a memory cost per hour while it is kept, and a one-time reactivation cost when the
   operator returns. That is the rent-or-buy problem of Karlin et al. A fixed timeout equal to
   the break-even time (reactivation cost divided by cost per hour) costs at most twice the
   offline optimum; a randomized timeout costs at most e/(e-1), about 1.58 times. When the
   distribution of idle periods is known, the best timeout is the time at which the chance per
   hour that the operator returns falls below the cost ratio. The ADR's 2 h and 24 h have no
   such basis. For Suspended, under memory pressure, the break-even time is minutes, not
   hours (Q3.2). [paper, inferred]
8. **The token cost of a reload is mostly not caused by hibernation.** The model provider's
   prompt cache lives 5 minutes by default (1 hour as a paid option) and is refreshed on use.
   After 2 hours idle, the next turn re-sends the conversation uncached whether the process
   lived or not. The reactivation cost of Hibernated is the operator's wait, the start of the
   agent and its MCP servers, the text not sent, and the risk that resume fails. [docs for the
   cache; inferred for the conclusion]
   <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
9. **The peer fact needs a push and a resync, not a 30 s poll alone.** Today peers do not push
   their sessions; the page's daemon reads them again every 30 s (docs/ARCHITECTURE.md §7).
   Kubernetes keeps the same split after years of work: events are "an optimization, not a
   source of truth", and the periodic relist stays on. A peer should push a change with a
   revision number and keep a slow full resync, and a peer that does not answer must show as
   unknown, not as "no consoles". [docs, inferred]

---

## Q1. Process supervision by operating-system events

### Q1.1 Snapshots against containment

A snapshot (Toolhelp32 on Windows, `/proc` on Linux, `sysctl` on macOS) gives PIDs and parent
PIDs at one instant. Three problems follow. [inferred]

- **A process born and ended between two reads is never seen.** With one read a minute, a
  test run of 30 seconds is missed. D3 counts "a process created or ended in the tree" as
  activity, so a snapshot loses activity.
- **The parent link breaks.** When a middle process exits, its child is re-parented (to init or
  a subreaper on Linux; Windows keeps a stale parent PID that may now belong to another
  process). A tree rebuilt from parent PIDs loses the grandchild or adopts a stranger.
- **PIDs are reused.** Matching a PID plus creation time plus executable (D6) narrows the
  error, but each read is still a guess about identity.

A container solves all three. The kernel puts each new child into the parent's container at
creation, and only a privileged act takes it out. The supervisor reads the member list of one
container when it needs it. Identity comes from a handle or a file descriptor held on the
process, which cannot be reused while it is open. [inferred from the sources below]

### Q1.2 Windows

- **Job objects contain the tree.** "After a process is associated with a job, by default any
  child processes it creates using CreateProcess are also associated with the job." The
  association "cannot be broken". Since Windows 8 a process can be in several nested jobs.
  [docs] <https://learn.microsoft.com/windows/win32/procthread/job-objects>
- **Breakaway.** A child leaves the job only if the job has `JOB_OBJECT_LIMIT_BREAKAWAY_OK` and
  the child is created with `CREATE_BREAKAWAY_FROM_JOB`, or if the job has
  `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK`. With neither, "the tool can monitor the entire process
  tree". A child that tries to assign itself to another job then fails, unless it nests. Note:
  `Win32_Process.Create` (WMI) children are not in the job. [docs] same page.
- **Events on a completion port.** The messages are `NEW_PROCESS` (the PID that was added;
  also sent for processes already in the job when the port is attached), `EXIT_PROCESS`,
  `ABNORMAL_EXIT_PROCESS`, `ACTIVE_PROCESS_ZERO` (the count fell to 0), and limit messages.
  Two warnings: "messages are intended only as notifications and their delivery to the
  completion port is not guaranteed", and a PID in a message may be recycled "unless you
  maintain an open handle to the process". Microsoft advises attaching the port while the job
  is inactive, to miss nothing. [docs]
  <https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-jobobject_associate_completion_port>
- **Resync list and totals.** `JobObjectBasicProcessIdList` returns the PIDs of the job and of
  its child jobs. `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION` gives `TotalUserTime` and
  `TotalKernelTime` "for all active processes ... as well as all terminated processes",
  `TotalProcesses` over the job's life, and `ActiveProcesses`. The `_AND_IO_` variant adds I/O
  counts. So the CPU and I/O evidence of D3 is one call per console, and a short-lived child
  still moves `TotalProcesses`. [docs]
  <https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-jobobject_basic_process_id_list>,
  <https://learn.microsoft.com/windows/win32/api/winnt/ns-winnt-jobobject_basic_accounting_information>
- **Kill and cleanup.** `TerminateJobObject` ends every member. With
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, "closing the last job object handle terminates all
  associated processes". [docs] job-objects page above.
- **The race at creation.** A process assigned after `CreateProcess` can start children before
  the assignment, and those children stay outside. The documented fixes are
  `PROC_THREAD_ATTRIBUTE_JOB_LIST` ("a list of job handles to be assigned to the child
  process", Windows 10 and later) or `CREATE_SUSPENDED`, then `AssignProcessToJobObject`, then
  `ResumeThread`. Microsoft names `CREATE_SUSPENDED` for one security limit; the general use
  is common practice. `AssignProcessToJobObject` needs `PROCESS_SET_QUOTA | PROCESS_TERMINATE`.
  [docs]
  <https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute>,
  <https://learn.microsoft.com/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject>
- **Ralphy today.** `portable-pty` 0.9 supports neither fix without a patch (finding 5). The
  daemon has no job object for consoles; it uses `CREATE_BREAKAWAY_FROM_JOB` only when it
  restarts itself (`crates/ralphy-cli/src/daemon/restart.rs`). [measured]
- **PID reuse, defeated.** Open a handle as soon as `NEW_PROCESS` arrives, then check
  `IsProcessInJob(handle, job)`. If the PID was reused in between, the new process is not in
  the job (unless it was born there, which makes it a member anyway). [inferred from the docs
  above]
- **Where conhost sits.** `CreatePseudoConsole` starts the console host before the child
  exists, from the caller, so conhost (or OpenConsole) is a child of the daemon, not of the
  shell. A console job filled through `JOB_LIST` at the child's creation does not contain
  conhost; freezing the job does not freeze conhost. Closing the pseudoconsole terminates "any
  related attached processes in the tree", and the host must keep draining the output pipe.
  [docs for close and drain; inferred for the parent of conhost]
  <https://learn.microsoft.com/windows/console/creating-a-pseudoconsole-session>
- **Console membership as a second list.** node-pty kills a ConPTY console by asking a helper
  process, attached to the console, for `GetConsoleProcessList`, then killing each PID; on a
  5 s timeout it falls back to the shell PID alone. It uses no job object. Microsoft marks
  `GetConsoleProcessList` "not recommended", with no virtual-terminal equivalent. Useful only
  as a check; a process that detaches from the console is not listed. [docs for node-pty
  source and the API]
  <https://raw.githubusercontent.com/microsoft/node-pty/main/src/windowsPtyAgent.ts>,
  <https://learn.microsoft.com/windows/console/getconsoleprocesslist>
- **ETW is not an option without rights.** Only administrators, members of Performance Log
  Users, and some service accounts can control trace sessions; only administrators and
  LocalSystem can control the NT Kernel Logger. Microsoft also warns that cross-process trace
  sessions are "a limited system resource" (64 by default). [docs]
  <https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-starttracew>
- **Chromium uses one job per renderer** with "one active process limit (disallows creating
  child processes)" and UI limits, as a security boundary. It is the main production example
  of a job per child on Windows; it is not used for tree tracking. [docs]
  <https://chromium.googlesource.com/chromium/src/+/main/docs/design/sandbox.md>
- **Service Control Manager.** I found no primary text that SCM tracks a service's process
  tree. Not used here. [gap]

### Q1.3 Linux

- **cgroup v2 contains the tree.** A child is born in its parent's cgroup. Moving a process out
  needs write access to `cgroup.procs` of the source, the target and their common ancestor, so
  a console cannot leave a delegated subtree. [docs]
  <https://docs.kernel.org/admin-guide/cgroup-v2.html>
- **Events without polling.** `cgroup.events` holds `populated` (1 while the cgroup or a
  descendant has a live process) and `frozen`. On change, "file modified events will be
  generated", so inotify or poll on that file gives the "tree is empty" event and the "freeze
  is done" event. `cgroup.procs` lists the members. `cpu.stat` (`usage_usec`, `user_usec`,
  `system_usec`) "exists whether the controller is enabled or not". `memory.current` gives the
  memory of the whole console, which is the real measure of what Suspended or Hibernated
  saves. `cgroup.kill` ends the tree, safe against concurrent forks. [docs] same page.
- **No birth event per process inside a cgroup.** There is none, and none is needed: the
  container is the registry. Read `cgroup.procs` on each `populated` change, on each state
  change, and on a slow timer. [inferred]
- **Delegation.** systemd: "each cgroup only has a single writer"; never write to a cgroup
  systemd created for you unless the unit has `Delegate=`; a delegated daemon must move itself
  into a sub-cgroup first, because a cgroup with children "may not contain processes".
  `DelegateSubgroup=` (systemd 254) does that move. [docs]
  <https://systemd.io/CGROUP_DELEGATION/>,
  <https://www.freedesktop.org/software/systemd/man/latest/systemd.resource-control.html>
- **Transient scopes.** `systemd-run --user --scope` makes a scope unit whose process is a
  child of `systemd-run`; over D-Bus, `StartTransientUnit` does the same. [docs]
  <https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html> A scope per
  console is a separate unit: it does not end when the daemon's unit stops. A sub-cgroup
  inside the daemon's own delegated unit does. For Ralphy, the second one matches D6.
  [inferred]
- **pidfd.** `pidfd_open` (Linux 5.3) returns a descriptor for one process; it polls readable
  when the process exits, and it keeps its identity while open. For a process the caller did
  not fork, the PID must be checked after the open; `clone3` with `CLONE_PIDFD` closes that
  gap at creation. [docs] <https://man7.org/linux/man-pages/man2/pidfd_open.2.html> Read a PID
  from `cgroup.procs`, open a pidfd, then re-read `/proc/<pid>/cgroup`; a reused PID is not in
  the console's cgroup. [inferred]
- **Subreaper.** `PR_SET_CHILD_SUBREAPER` (Linux 3.4) makes orphaned descendants re-parent to
  the daemon, so it can `wait` on them. It is not inherited by children. It keeps parent links
  true, but it is not a container: a process that double-forks is still found only by its
  parent link. [docs] <https://man7.org/linux/man-pages/man2/PR_SET_CHILD_SUBREAPER.2const.html>
- **Netlink proc connector.** It reports every fork, exec and exit on the system. It needed
  `CAP_NET_ADMIN`; a 2023 patch series allows non-root listeners for the process-event group.
  I could not confirm the first kernel release that has it. It is system-wide (every process
  of every user), so the daemon must filter, and netlink drops messages under load. [reported]
  <https://lore.kernel.org/all/20230310221547.3656194-5-anjali.k.kulkarni@oracle.com/>
- **WSL2.** With systemd on, the user manager delegates `memory pids` to `user@<uid>.service`,
  and a user unit's cgroup files are owned by the user. Without systemd, or for a process
  started by `wsl.exe -e`, the process is in `/non-systemd`, owned by root, and the daemon has
  no cgroup of its own. [measured on WSL kernel 6.18.58.1, Ubuntu 22.04, systemd 249]
  Swap is 8 GB there, so suspended pages can leave RAM. [measured]

### Q1.4 macOS

- **kqueue `EVFILT_PROC`.** `NOTE_EXIT`, `NOTE_FORK` and `NOTE_EXEC` are defined. XNU's
  header says: "NOTE_TRACK, NOTE_TRACKERR, and NOTE_CHILD are no longer supported as of 10.5".
  So a watcher learns that a watched PID forked, but not the child's PID; it must list children
  itself, which races with a fast child. [docs: XNU source]
  <https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/sys/event.h>
- **No container.** macOS has no cgroup and no job object. The closest groups are the process
  group and the session; the daemon starts each console in its own session (the PTY), and a
  child that calls `setsid` leaves it. [inferred]
- **launchd** kills the remaining members of a job's process group when the job's main process
  dies, unless `AbandonProcessGroup` is true. Consoles live in their own sessions, so this does
  not reach them. [reported: launchd source patch, Apple forum]
  <https://lists.apple.com/archives/darwin-dev/2011/Nov/msg00007.html>
- **Endpoint Security** reports fork and exec for the whole system, but a client needs Apple's
  entitlement and must run as root (`ES_NEW_CLIENT_RESULT_ERR_NOT_ENTITLED`,
  `ERR_NOT_PRIVILEGED`), and user approval. Not usable by Ralphy. [docs]
  <https://developer.apple.com/documentation/endpointsecurity/client>
- So macOS stays with polling plus `NOTE_FORK`/`NOTE_EXIT` on known PIDs. [inferred]

### Q1.5 What real supervisors do

- **systemd**: the unit's cgroup; cgroup v2 gives "proper cgroup-empty notifications".
  [docs] systemd.io above.
- **Kubernetes kubelet**: asks the runtime (relist), plus optional runtime events. [docs] Q5.
- **Chromium on Windows**: a job per renderer that forbids children. [docs] sandbox.md.
- **node-pty and VS Code**: the console's attached-process list at kill time. [docs] node-pty
  source.
- **Docker and runc**: the container's cgroup; `docker pause` uses the freezer. [docs] Q2.
- **launchd**: the job's process group. [reported]
- **s6, runit**: the direct child only; a daemon must not fork away. [gap: skarnet.org refused
  the fetch, so this is unverified]

No supervisor I found rebuilds a tree from a periodic process-table snapshot as its main
model. [inferred from the table]

---

## Q2. Safe freeze of a tree, and the death of the supervisor

Each means, with three questions: does it freeze children born later; can the process see
it; what happens if the supervisor dies.

- **`NtSuspendProcess` per process**: no, it races with new threads; not visible; the process
  stays suspended.
- **Process state-change object (Windows 11)**: yes, for that one process; not visible; undone
  when the object's last reference goes.
- **Job freeze (Windows 8+)**: yes, for the whole job, with no race; not visible; stays frozen,
  or ended when the job has `KILL_ON_JOB_CLOSE`.
- **cgroup v2 `cgroup.freeze`**: yes, for the whole subtree; not visible; stays frozen until
  systemd stops the unit and kills its cgroup.
- **`SIGSTOP` to a process group**: no, a child outside the group runs; visible to the parent;
  an orphaned group gets `SIGHUP`, then `SIGCONT`.
- **Android cached apps freezer**: cgroup v2 based; not visible; no source for a crash.
- **`docker pause`**: the cgroup freezer; not visible; no source for a crash.

- **Windows sources.** The state-change object: created with a handle that has
  `PROCESS_SET_INFORMATION`; changed with `PROCESS_SUSPEND_RESUME`; "when the reference
  counter on the process state object drops to zero, the system automatically undoes the
  effect". Windows 11. [reported: ntdoc, phnt-based]
  <https://ntdoc.m417z.com/ntcreateprocessstatechange> The job freeze: "deep freezing ... not
  susceptible to race conditions and takes care of the new threads"; "closing the last handle
  to a frozen job makes it impossible to unfreeze". Both freeze future threads. [reported:
  diversenok] <https://github.com/diversenok/Suspending-Techniques> Kill-on-close ending the
  members on the last close is [docs]; whether it also ends frozen members at once is not in
  any source I found. [gap: spike]
- **The state-change object is per process.** To freeze a tree, the daemon enumerates it and
  creates one object per process. A process that is not frozen yet (an MCP server, a test
  runner) can start a child between the enumeration and its own freeze. The ADR handles this
  with "read the tree again; a new process means resume all and give up". With a job, the
  enumeration is the job list, and a job freeze removes the window. [inferred]
- **Windows PLM** suspends a packaged app a few seconds after it leaves the foreground, gives
  it a `Suspending` event with a deadline, and may terminate it later "if the system does not
  have the resources"; the app gets no notice of termination and must restore its state from
  `PreviousExecutionState == Terminated`. Network events are "not queued--they are simply
  missed" while suspended. Which kernel means PLM uses is not documented. [docs for the model;
  gap for the means] <https://learn.microsoft.com/windows/uwp/launch-resume/app-lifecycle>
- **cgroup freezer.** Async: write `1` to `cgroup.freeze`, wait for `frozen 1` in
  `cgroup.events`. A process moved into a frozen cgroup stops. A frozen process can still be
  killed by a fatal signal. `docker pause` uses it, and "the process is unaware, and unable to
  capture, that it is being suspended". [docs]
  <https://docs.docker.com/reference/cli/docker/container/pause/> systemd exposes the same as
  `systemctl freeze`/`thaw` (version 246, unified hierarchy only) and as `Freeze()`/`Thaw()`
  with a `FreezerState` property on D-Bus. [docs]
  <https://www.freedesktop.org/software/systemd/man/latest/systemctl.html>
- **If the Linux daemon dies.** The unit has `KillMode=control-group` and
  `Restart=on-failure` (measured). A frozen sub-cgroup inside the unit stays frozen until
  systemd stops the unit. A fatal signal works on a frozen task, so the tree ends; a `SIGTERM`
  waits until thaw, so the stop may wait for the stop timeout before `SIGKILL`. Whether
  systemd thaws before it stops is [reported] in the old note; not re-checked. [docs for the
  signal rule; inferred for the sequence]
- **`SIGSTOP` under a PTY.** The old note covers the shell and orphaned-group traps. One
  addition: a TUI usually handles `SIGCONT` by re-entering raw mode and drawing again; a
  frozen cgroup sends no signal, so nothing is drawn again on thaw. [inferred]
- **Android.** Freeze 10 s after a process becomes cached (Android 14+); a sync binder call to
  a frozen process kills it; async calls are buffered and kill it on overflow; when all of an
  app's processes are frozen its TCP sockets are closed; GC, then freeze, then compaction.
  [docs] <https://source.android.com/docs/core/perf/cached-apps-freezer> The page says nothing
  about a crash of `system_server`, the process that freezes. [gap]
- **CRIU** stays a poor fit (old note, §2.4).

### Q2.1 What a long freeze does to network connections and timers

- **TCP keepalive is answered by the kernel**, so a frozen process keeps its sockets open at
  the TCP level. Keepalives are sent only with `SO_KEEPALIVE`, and Linux waits 7200 s idle
  before the first probe by default. `TCP_USER_TIMEOUT` closes a connection whose sent data
  stays unacknowledged. [docs] <https://man7.org/linux/man-pages/man7/tcp.7.html>
- **HTTP/2 PING is answered by the application.** "Receivers of a PING frame ... MUST send a
  PING frame with the ACK flag set"; PING exists for "determining whether an idle connection
  is still functional". A frozen HTTP/2 client cannot answer, so a server that pings an idle
  connection closes it. [docs] <https://www.rfc-editor.org/rfc/rfc9113.html#section-6.7>
- **What a resumed agent sees** [inferred]: the kernel may have queued a FIN or RST; the first
  read or write returns an end of stream, `ECONNRESET` or `EPIPE`. An HTTP client that retries
  on a new connection hides it; a stream that was open mid-turn is lost. So D3's "the agent is
  in a turn" blocker is a correctness rule, not a preference. TLS adds no timer of its own;
  OAuth tokens may have expired and are refreshed on the next call.
- **Timers.** `CLOCK_MONOTONIC` keeps running while one process is stopped. On resume, every
  timer that came due fires once (Node re-arms from the callback time); fixed-rate schedulers
  burst, which Android documents. [inferred; docs for Android]
- **Chrome and PLM give the page a chance first.** Chrome fires `freeze` before it freezes a
  page and advises it to close WebSockets and database connections, and fires `resume` after.
  PLM fires `Suspending` with a deadline and `Resuming` after, and says to refresh stale
  network connections on resume. An agent CLI gets no such event from Ralphy. [docs]
  <https://developer.chrome.com/docs/web-platform/page-lifecycle-api>

---

## Q3. Theory for idle timeouts

### Q3.1 Sources

- **Dynamic power management.** Benini, Bogliolo and De Micheli, "A survey of design techniques
  for system-level dynamic power management", IEEE TVLSI 8(3):299–316, 2000. The break-even
  time `T_BE` of an inactive state is "the minimum inactivity time required to compensate the
  cost of entering" it: the transition time, plus the extra time needed to pay back the
  transition energy when the transition costs more power than staying on. The fixed timeout is
  "the most common predictive PM policy"; it trades efficiency for safety: "large timeouts
  cause a large number of underpredictions ... and a sizeable amount of power is wasted waiting
  for the timeout to expire", and timeouts "always pay a performance penalty upon wakeup". The
  survey also covers predictive shutdown (from the history of idle and busy periods), predictive
  wakeup, and stochastic (Markov) policies. [paper]
  <https://si2.epfl.ch/demichel/publications/archive/2000/VLSISvol8iss3Jun00pg299.pdf>
- **Fixed timeout at `T_BE` is 2-competitive.** The survey states that Karlin et al. "proposed
  to use [a timeout equal to `T_BE`] and showed that this choice leads to an energy consumption
  which is at worse twice the energy consumed by an ideal policy". [paper] same PDF, §III-B.
  The original: Karlin, Manasse, Rudolph, Sleator, "Competitive snoopy caching", Algorithmica
  3:79–119, 1988 (FOCS 1986). [paper] <https://doi.org/10.1007/BF01762111>
- **Randomized: e/(e-1).** Karlin, Manasse, McGeoch, Owicki, "Competitive randomized algorithms
  for nonuniform problems", Algorithmica 11(6):542–571, 1994 (SODA 1990): randomized algorithms
  with ratio approaching e/(e-1), about 1.58, against an oblivious adversary; no online
  algorithm beats 2 against an adaptive one; and a deterministic algorithm that adapts to the
  observed distribution approaches the same factor. [paper; abstract only read]
  <https://doi.org/10.1007/BF01189993>
- **Learning-augmented ski rental.** Purohit, Svitkina, Kumar, "Improving Online Algorithms via
  ML Predictions", NeurIPS 2018. With a prediction `y` of the number of days and buy cost `b`:
  "if y ≥ b then buy on the start of day ⌈λb⌉, else buy on the start of day ⌈b/λ⌉", with
  `λ ∈ (0, 1)`. Theorem 2.2: this is `(1 + λ)`-consistent (good prediction) and
  `(1 + 1/λ)`-robust (any prediction). A randomized version is `λ/(1 − e^(−λ))`-consistent.
  [paper] <https://papers.nips.cc/paper/2018/hash/73a427badebe0e32caa2e1fc7530b7f3-Abstract.html>

### Q3.2 The mapping to a console

Keeping a console alive is renting; suspending or hibernating it is buying. [inferred from the
sources above]

- `c` = cost per hour of keeping the console in its current state, instead of the next one.
  It is the memory the next state frees, `ΔM` (GB), times the value of one GB-hour, `v`.
- `B` = one-time cost when the operator comes back to a console in the next state. For
  Suspended: the resume time (under 1 s by the ADR), plus the chance of a broken connection.
  For Hibernated: the wait from click to a usable prompt, plus the start of MCP servers, plus
  the value of the text not sent, plus the chance that resume fails, times the cost of a lost
  conversation.
- `T_BE = B / c = B / (ΔM · v)`.

Rules, from the cheapest to the most informed:

1. **Fixed timeout at `T_BE`**: never worse than 2 times the best offline choice, for any
   operator.
2. **Learned distribution.** With `S(t)` the chance an idle period lasts longer than `t`, and
   `f(t)` its density, the expected cost of a timeout `τ` is
   `E(τ) = c · ∫₀^τ S(t) dt + B · S(τ)`.
   Its derivative is `S(τ) · (c − B · h(τ))`, with the return hazard `h = f / S`. So the best
   `τ` is where `h(τ) = c / B`: **move the console to the next state when the chance per hour
   that the operator comes back falls below `c / B`**. When the hazard falls over time (an idle
   period that has lasted long is likely to last longer, as for nights and weekends), there is
   one crossing; if the hazard starts below `c / B`, act at once.
3. **Per-console prediction** (Purohit et al.): predict `y` from the console's own history (for
   example, "idle past 19:00 on a weekday lasts until the next morning"); if `y ≥ T_BE`, act at
   `λ · T_BE`, else at `T_BE / λ`. With `λ = 0.5`: at most 1.5 times the best cost when the
   prediction is right, 3 times when it is wrong.

What `v` is: memory has a value only when something else needs it. That is why Firefox unloads
tabs only on low memory and Android's lmkd kills by pressure (Q4). So `v` is near 0 while free
memory is high and large under pressure. Windows signals this with
`CreateMemoryResourceNotification(LowMemoryResourceNotification)`, a system-wide waitable
object [docs]
<https://learn.microsoft.com/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification>;
Linux with PSI (`memory.pressure`, which lmkd uses on Android 10+) [docs]
<https://source.android.com/docs/core/perf/lmkd>. A practical form is two values of `v`, so two
values of `T_BE`: a calm one and a pressure one. [inferred]

A worked example with guessed numbers (each must be measured; see the spike section):
Suspended frees `ΔM = 1 GB` and costs `B ≈ 2 s` of the operator's time. When memory is tight
and one GB-hour is worth one minute of the operator's time, `c = 60 s per hour` and
`T_BE = 2 s / (60 s per hour) = 2 minutes`. When memory is calm and one GB-hour is worth
2 seconds, `c = 2 s per hour` and `T_BE = 1 hour`. The result moves with `v` by more than one
order of magnitude, so a constant 2 h fits only one value of `v`. For
Hibernated, `B` is tens of seconds plus risk, and `ΔM` is only what Suspended did not already
free (commit charge, compressed pages), so its `T_BE` is much longer. [inferred]

### Q3.3 What the theory does not cover

- **Safety is a separate rule.** The competitive ratio counts only rent and buy. A suspension
  during background work (a test run, a stream mid-turn) has a cost that is not `B`: lost work.
  D3's blockers handle it, and they stay. [inferred]
- **Abandoned consoles.** An idle period that never ends (the operator closes the console
  later) makes any early move free. The learned distribution captures this; the fixed rule
  does not. [inferred]

---

## Q4. Lifecycle state models in production

- **Chrome Page Lifecycle**: active, passive, hidden, frozen, discarded, terminated. Events:
  `visibilitychange`, `freeze`, `resume`, `pagehide`, `pageshow`; on reload,
  `document.wasDiscarded`. [docs] developer.chrome.com, Q2.1.
- **Android**: foreground, visible, service, cached; a cached process is frozen after 10 s.
  Component callbacks, none at the freeze. [docs]
- **Android lmkd**: kills by `oom_adj` level under PSI memory pressure. No event. [docs]
  <https://source.android.com/docs/core/perf/lmkd>
- **Windows PLM (packaged apps)**: not running, running in foreground or background, suspended,
  terminated. Events: `EnteredBackground`, `Suspending` with a deadline, `Resuming`; on launch,
  `PreviousExecutionState`. [docs] Q2.

Android sources: <https://developer.android.com/guide/components/activities/process-lifecycle>.
A cached process "is not currently needed, so the system is free to kill it"; a long-running
service "might be demoted" to cached after about 30 minutes. [docs]

Mapping to Ralphy [inferred]:

| Ralphy | Chrome | Android | PLM |
| --- | --- | --- | --- |
| live, shown, written to | active | foreground | running in foreground |
| live, not shown | passive or hidden | visible or cached, not frozen | running in background |
| Suspended | frozen | cached and frozen | suspended |
| Hibernated | discarded | killed by lmkd | terminated |
| Placeholder | terminated | cold start | not running |

What Ralphy's model lacks [inferred from the table]:

1. **A notice before the freeze.** Chrome, PLM and Android's GC step all give the program a
   moment to drop connections and save. Ralphy freezes an agent with no notice. A hook to the
   agent is not possible for most vendors, so the daemon must assume a broken connection on
   resume (Q2.1).
2. **A pressure trigger.** Every production model but Edge's 2 h rule acts on memory pressure
   or on a short time after the program becomes hidden. Ralphy acts only on a long timer.
3. **A short first step.** Android freezes after 10 s and Chrome after 5 minutes hidden and
   silent; both are cheap to undo. Suspended is the same kind of step and could be minutes.
4. **A "was discarded" fact on return.** Chrome's `document.wasDiscarded` and PLM's
   `PreviousExecutionState` tell the program how it came back. Ralphy's equivalent is the veil
   and the hint (D8, D13); the relaunched agent itself is not told.
5. **An order of victims.** lmkd kills by level and by size; Chrome's discard order puts pinned
   and audible tabs last. Ralphy has Always on, but no order among the rest (largest first,
   oldest input first). [inferred]

---

## Q5. Federated or remote supervision

- **Kubernetes PLEG.** The kubelet's package documentation: "it is recommended to set the
  relist period short and have an auxiliary, longer periodic sync in kubelet as the safety
  net", and the health threshold must be "greater than the relisting period + the relisting
  time". [docs] <https://pkg.go.dev/k8s.io/kubernetes/pkg/kubelet/pleg> The values, a 1 s
  relist and a 3 min threshold, come from a Red Hat article that quotes the source. [reported]
  <https://developers.redhat.com/blog/2019/11/13/pod-lifecycle-event-generator-understanding-the-pleg-is-not-healthy-issue-in-kubernetes>
  KEP-3386 (evented PLEG) adds the CRI `GetContainerEvents` stream; in its current text, an
  event only asks for a relist of that pod, "CRI events are best effort and may be delayed,
  duplicated, reordered, or lost", there is "no replay or resume mechanism", and the generic
  relist keeps its normal period: the stream is "an optimization, not a source of truth". It is
  still alpha, disabled by default, after a revert from beta in v1.30. [docs]
  <https://github.com/kubernetes/enhancements/tree/master/keps/sig-node/3386-kubelet-evented-pleg>
- **List plus watch.** A client lists, then watches from the list's `resourceVersion`, so it
  can "fetch the current state and then subscribe to subsequent changes, without missing any
  events". On `410 Gone` it must clear its cache, list again and watch from the new version.
  Bookmarks advance the version with no change. [docs]
  <https://kubernetes.io/docs/reference/using-api/api-concepts/>
- **Unknown is a state.** Pod phase `Unknown`: "the state of the Pod could not be obtained ...
  typically due to an error in communicating with the node". [docs]
  <https://kubernetes.io/docs/concepts/workloads/pods/pod-lifecycle/>
- **systemd over D-Bus.** `PropertiesChanged`, `UnitNew`, `UnitRemoved` and job signals are
  sent only after a client calls `Subscribe()`. Not every property signals its change: each has
  an `EmitsChangedSignal` annotation (`const`, `false`, `invalidates`), and `invalidates` says
  that a value changed without sending it, so the client must read it again. [docs]
  <https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html>
  `systemd-run -H user@host` and `systemctl -H` reach a remote manager over SSH; `-M` reaches
  a local container. [docs] systemd-run page above.
- **The general pattern.** Edge-triggered events give speed; a level-triggered resync gives
  truth; a version number lets the receiver tell a stale message from a new one; a node that
  stops answering is shown as unknown, not as empty. [inferred from the sources above]

What applies to a peer that pushes its console list to a browser [inferred]:

1. The peer pushes `sessions.dirty` (or the changed entries) with a monotonic revision number,
   on each change of its process registry or of a console's state.
2. The receiving daemon keeps the 30 s read as the resync, and may slow it while pushes arrive.
3. A push whose revision is not the next one triggers a full read, as `410 Gone` does.
4. A peer that does not answer keeps its consoles listed, marked unknown, with the time of the
   last good read. The daemon already sends `x-ralphy-unanswered`; the page should not read
   that as "no consoles".
5. The fact that crosses the wire is the state (live, Suspended, Hibernated, starting) and the
   time of the last change, never a process list. Each peer owns its own processes (D1).

---

## Mapping to ADR-0083

- **D1, the daemon owns the cycle: confirms.** Every production model puts one owner on the
  node: PLM, lmkd, the kubelet.
- **D2, two inactive states: confirms and refines.** They match frozen and discarded. Suspended
  can be a short step, not a 2 h one.
- **D3, activity and blockers: refines.** The job counter `TotalProcesses` and the cgroup
  `cpu.stat` include processes that already ended, so a test run between two reads still
  counts. The mid-turn blocker is a correctness rule (Q2.1). The foreground process group of
  the PTY (`tcgetpgrp` on Unix) shows when the agent runs a program in the foreground; I found
  no primary source for its use as an idle signal. [docs for the counters; inferred]
- **D4, one OS read a minute: contradicts.** Containers give events and per-console totals; a
  global snapshot misses short children. The per-console reads are also cheaper than one global
  snapshot: one `QueryInformationJobObject` call, or three small file reads, per console.
- **D5, the safest means per platform: refines.** Two Windows candidates, both undocumented:
  per-process state-change objects (no stuck tree if the daemon dies; a race window during the
  enumeration) and the job freeze (no race; the tree ends if the daemon dies, with
  kill-on-close). Linux: `cgroup.freeze` in a sub-cgroup of the delegated daemon unit;
  `SIGSTOP` only where the daemon has no cgroup. macOS: `SIGSTOP`.
- **D6, the custody record: refines.** With a job per console (Windows) or a sub-cgroup per
  console (Linux under systemd), the daemon's death ends every console without a record. The
  record then holds what must survive: the agent session id, the repo, the state and the saved
  screen. PIDs in the record are no longer needed for safety there; on macOS and on Linux
  without systemd they still are.
- **D7, a chosen session id: confirms; out of date.** The table still lists Gemini, which
  Antigravity replaced.
- **D8, a click reactivates: confirms and refines.** The same as Chrome and PLM; add a
  pressure path that needs no click.
- **D9, the cursor answer: confirms.** `portable-pty` sets `PSEUDOCONSOLE_INHERIT_CURSOR`.
- **D10, opt-in with 2 h and 24 h: contradicts the constants.** Keep the settings, but default
  them from the break-even rule with measured inputs (Q3), and add the pressure trigger.
- **D11, a WSL free console: confirms and refines.** The WSL daemon can freeze its own consoles
  once its unit is delegated.
- **D12, one start queue: confirms.** No source gives the number 2.
- **D13, nothing starts without a click: confirms.** The same as Chrome: a discarded tab loads
  again only when it is opened.

## A proposed method

**Process registry, per daemon.** One module owns every process Ralphy starts. Each console is
one container: a job object on Windows (created with `KILL_ON_JOB_CLOSE`, no breakaway, a
completion port, the child put in through `PROC_THREAD_ATTRIBUTE_JOB_LIST`, which needs a patch
to `portable-pty`); a sub-cgroup of the daemon's delegated unit on Linux (`Delegate=yes`; the
daemon moves itself into a leaf, since systemd 249 has no `DelegateSubgroup=`); the PTY session
on macOS, with kqueue `NOTE_FORK`/`NOTE_EXIT`. Events update the registry: job messages, the
`populated` change of `cgroup.events`, kqueue. A slow resync reads each container's member list
and repairs the registry, because job messages are not guaranteed. An entry holds a process
handle or a pidfd, so its identity cannot be reused.

**Suspend means.** Windows 11: the job freeze or one state-change object per process; the spike
decides. Linux: `cgroup.freeze`, then wait for `frozen 1`; `SIGSTOP` to the session-leading
agent only without a cgroup. macOS: `SIGSTOP` with the same rule. Never freeze conhost or a
process outside the container. On resume, expect one broken connection.

**Timeout rule.** For each step, compute `T_BE = B / (ΔM · v)` from measured `B` and `ΔM`, with
a calm and a pressure value of `v`. Start with the fixed timeout at `T_BE` (2-competitive). When
the observe-only slice has enough idle periods, move to the hazard rule `h(τ) = c / B`. Add a
pressure trigger (`LowMemoryResourceNotification`, PSI) that suspends idle consoles in order of
size and age. The blockers of D3 always win.

**Peer fact.** Each peer pushes its console states with a revision number on each change; the
receiver keeps a slow full read as the resync; a gap in revisions forces a full read; a silent
peer shows its consoles as unknown with the time of the last good read.

---

## What to measure in a spike

No source gives these numbers. Each needs a local measurement on Windows 11, WSL, and a Linux
or macOS machine where possible.

1. **Job freeze plus kill-on-close.** Freeze a scratch job with `KILL_ON_JOB_CLOSE`, end the
   holder process, and time how long the members take to end. Same with the state-change
   object (already done: they resume).
2. **The minimum right** for `NtCreateProcessStateChange`: try
   `PROCESS_SET_INFORMATION | PROCESS_SUSPEND_RESUME` alone.
3. **Job messages under load.** Start and end 1 000 short children in a job and count
   `NEW_PROCESS` and `EXIT_PROCESS` messages against the real count, to size the resync period.
4. **Memory freed, per state and per agent.** For Suspended: private commit, working set,
   standby list and compressed store before and after, at 1 h. For Hibernated: the same, plus
   what the relaunched agent holds after its first turn. On Linux: `memory.current` and
   `memory.stat` of the console's cgroup. This gives `ΔM`.
5. **Reactivation cost.** Suspended: time from click to the first echoed key. Hibernated: time
   from click to the first draw and to a usable prompt, per agent, with and without MCP
   servers; the share of resumes that fail. This gives `B`.
6. **Connections after a freeze.** Freeze an idle agent for 10 minutes, 1 hour and 4 hours;
   resume; send one prompt; record whether the first call fails and whether the agent retries.
7. **Idle periods.** From the observe-only slice, the time between the last input and the next
   one, per console, over a few weeks. This gives `S(t)` and the hazard.
8. **Pressure.** How often `LowMemoryResourceNotification` (Windows) and `memory.pressure`
   (Linux) fire on the operator's machine with the usual load.
9. **A frozen cgroup and systemd.** Freeze a console sub-cgroup, crash the daemon in a scratch
   unit, and time how long systemd takes to end the frozen tree.

---

## Gaps

- No primary source for how PLM freezes packaged apps (job freeze or other).
- No source says whether `KILL_ON_JOB_CLOSE` ends frozen members at once.
- The first kernel release with non-root proc connector listeners is not confirmed.
- s6 and runit pages refused the fetch; their row in Q1.5 is unverified.
- No primary source on the Service Control Manager and process trees.
- Karlin et al. 1994 was read only through its abstract and the survey's summary.
- The kubelet relist period (1 s) and threshold (3 min) come from secondary readings of the
  source.
- Android does not document what happens to frozen apps when `system_server` crashes.
- Whether systemd thaws a frozen unit before it stops it was not re-checked.

## Corrections to the earlier note (2026-10-24)

- **Finding 4 and §1.3, "the job freeze turns a daemon crash into a stuck tree".** True only for
  a job without `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. With it, the last close ends the members
  (documented); the frozen case needs the spike above.
- **§1.3, "a frozen parent cannot spawn, so this is mostly fine".** A process that is not yet
  frozen during the per-process enumeration can still start a child; the window is real for a
  tree with MCP servers or test runners.
- **§2.1, "WSL with no systemd may have no writable delegated cgroup".** On this machine WSL runs
  systemd 249, `user@1000.service` has `Delegate=yes` (`memory pids`), and the peer's unit
  files are user-owned; the missing piece is `Delegate=yes` on Ralphy's unit. Processes started
  by `wsl.exe -e` are in a root-owned `/non-systemd` cgroup.
- **§5, "TCP keepalive probes are answered by the kernel".** True, but an HTTP/2 PING must be
  answered by the application, so a frozen HTTP/2 client can lose an idle connection long before
  a TCP keepalive would.
- **§1.3, the access rights of `NtCreateProcessStateChange`** were right; they also explain the
  ADR spike's `STATUS_ACCESS_DENIED`, which the ADR lists as open.
