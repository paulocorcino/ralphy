# Process checkpoint and restore for idle consoles (2026-11-15)

Web research and local experiments for
[ADR-0083](../adr/0083-an-idle-console-is-suspended-then-hibernated.md). The question: can
Ralphy save an idle console's process tree to disk and restore it later, for any agent and
without a session id, as a third path beside *Suspended* (freeze and trim) and *Hibernated*
(end, then relaunch with the agent's resume flag)? The earlier notes
(`.ralphy/docs/console-idle-research-web.md` §2.4, and
[the supervision research](idle-console-supervision-research-2026-11-15.md)) gave CRIU a few
lines. This note covers the subject in full.

Labels on each claim:

| Label | Meaning |
| --- | --- |
| **[docs]** | From the owner's documentation or source code. |
| **[paper]** | From a peer-reviewed paper. |
| **[reported]** | From a third party: an issue, a forum, a blog, a talk. |
| **[measured]** | Checked on this machine on 2026-11-15 (details below). |
| **[inferred]** | My own reasoning from the facts here. Not checked. |

The machine: Windows 11 Pro 26300, 32 GB RAM, WSL 3.0.5 with kernel
6.18.58.1-microsoft-standard-WSL2, Ubuntu 22.04, systemd 249. criu.org refused connections
from here during this work, so CRIU facts come from the CRIU man page and the CRIU GitHub
repository instead.

The experiments acted only on processes that they started: a `sleep`, Python and Node.js
loops in WSL, one Node.js process on Windows, and one transient systemd user scope. No
process of the owner's daemon or agents was stopped, signalled, frozen or checkpointed. The
only reads of other processes were `/proc/<pid>/status` and `/proc/<pid>/fd` of the WSL
peer's own processes. Nothing was installed system-wide: DMTCP 4.2.0 was built from source
under `/tmp/ckpt-research`, and the Ubuntu CRIU package was unpacked there with
`apt-get download` and `dpkg -x`. The scripts are in `target/ckpt-research/` (not in git).

---

## Findings that change the design

1. **On Linux, the daemon can page out a frozen console without any privilege.** In a
   transient user scope (`systemd-run --user --scope`), a plain user froze a Python process
   that held 600 MB, wrote `500M` to the scope's `memory.reclaim`, and saw 500 MB move to
   swap in 0.5 s. RSS stayed at 101 MB after the thaw. The file is owned by the user in
   every cgroup under `user@1000.service`, including the WSL peer's own unit. The ADR's
   sentence "An unprivileged process cannot page out another one" is true for
   `process_madvise` (it needs `CAP_SYS_NICE`), and false for a cgroup the daemon owns.
   With swap, this gives Linux most of what "save the process to disk" would give.
   [measured; docs] (Q1.2)
2. **On Windows, Suspended plus trim already frees the RAM, but never the commit.** A
   suspended Node.js process with 822 MB private: `EmptyWorkingSet` took its working set
   from 839 MB to 0 in 65 ms. Its commit stayed at 822 MB until the process ended. The
   trimmed pages went into the memory compression store: almost free for a regular heap
   (+2 MB), and the full size for random data (+680 MB). This machine already uses 59 GB of
   a 76 GB commit limit. Only ending the process, which Hibernated does, gives back commit.
   [measured; docs] (Q1.1)
3. **There is no way to save and restore a plain Win32 process.** `PssCaptureSnapshot` and
   minidumps are diagnostic: they read a process, and nothing writes one back. Only research
   systems (a library OS such as Drawbridge) and old commercial products (known today from
   their patents) did it. On Windows, only a whole VM can be saved. [docs; paper; reported]
   (Q3)
4. **CRIU cannot dump Ralphy's consoles today, in WSL or on most desktops.** Three separate
   facts block it.
   - Node.js 22.23 and 24.13 (libuv 1.51) map an `io_uring` ring at start, even with
     `UV_USE_IO_URING=0`, and CRIU has no `io_uring` support (open issue).
   - Dumping a task that has a seccomp filter needs `CAP_SYS_ADMIN`, and the dumper must
     have no seccomp filter itself. In this WSL, every process has a seccomp filter, PID 1
     too. That is the WSL failure reported to CRIU.
   - The "unprivileged" mode still needs a capability that root grants once
     (`setcap cap_checkpoint_restore`), `CAP_NET_ADMIN` for TCP repair, and
     `ptrace_scope` 0 or `CAP_SYS_PTRACE`. This machine has `ptrace_scope` 1.

   [measured; docs; reported] (Q2.1)
5. **DMTCP needs no root and works, but it fails on Node.js and Bun here.** DMTCP 4.2.0
   (June 2026) built and ran as a plain user. It saved a Python process with 844 MB
   resident to an 843 MB image in 1.1 s and restored it in 2.0 s; the program went on
   counting. With its default gzip, the same save took 18.2 s. It also restored a program
   under a PTY, but it could not give the restored program its terminal back. It aborted
   at start for Node.js 22, Node.js 24 and Claude Code 2.1.323 (a Bun binary). DMTCP must
   also start the program itself (`dmtcp_launch`). [measured; docs] (Q2.2)
6. **A restored agent meets a world that has moved on, and its resume-by-id was built for
   that world.** Its TCP and TLS connections to the model API and to remote MCP servers are
   gone (CRIU must also hold TCP repair rights; gVisor returns `ECONNRESET`). Its PTY, PIDs,
   files and tokens changed, and its random generators repeat if one image is restored twice
   (reported for Node.js 24 under CRIU). A resume by id reads a conversation from disk,
   with new connections and fresh state. [docs; reported; inferred] (Q7)
7. **A VM snapshot works for a whole guest, not for one console.** Firecracker needs Linux
   and KVM. Hyper-V `Save-VM` needs Pro and admin rights. WSL has no save or pause verb.
   Apple's Virtualization framework can save a VM on Apple silicon with macOS 14 or later.
   Each console would need its own VM, with the agent, its tools and the repository inside.
   That does not fit a desktop tool that runs agents on the operator's own checkouts.
   [docs; measured; inferred] (Q5)
8. **No JavaScript runtime saves a running program.** Node.js startup snapshots are taken
   once, at build time, from one entry script. Heap snapshots of V8 and of JavaScriptCore
   are JSON or inspector files for analysis. Bun's bytecode cache holds code, not state.
   [docs] (Q6)

**Net result:** a third "checkpointed" state is not realistic on any of the three platforms
with what exists in 2026. Two cheaper changes take most of its value: on Linux, add
`memory.reclaim` after the freeze (with swap present); on Windows, keep the trim, and show
commit as the cost that only Hibernated removes.

---

## Q1. What Suspended plus trim really frees

### Q1.1 Windows

How a trim works, from Microsoft's documentation:

- `EmptyWorkingSet`, or `SetProcessWorkingSetSizeEx` with both sizes `(SIZE_T)-1`,
  "removes as many pages as possible from the working set". The handle needs
  `PROCESS_SET_QUOTA`. [docs]
  <https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-setprocessworkingsetsizeex>
- A page removed from every working set becomes a transition page. It stays in RAM until it
  is touched again (a soft fault) or reused. A dirty page "must be written to its backing
  store before it can be repurposed", and the system may write dirty pages at once. For a
  private page, the backing store is the page file. [docs]
  <https://learn.microsoft.com/en-us/windows/win32/memory/working-set>
- Since Windows 10, the memory manager compresses unused pages "instead of writing them to
  disk", into a store that first lived in the System process. [docs]
  <https://blogs.windows.com/windows-insider/2015/08/18/announcing-windows-10-insider-preview-build-10525/>
  On this machine the store is the "Memory Compression" process, with a 3.9 to 4.7 GB
  working set during the tests. [measured] Secondary sources quote about 40% of the original
  size as typical. [reported] (same search; no primary figure found)
- The commit charge is the memory "promised" to processes. The commit limit is RAM plus all
  page files. When the charge reaches the limit, "the system and processes might not get
  committed memory", which can cause "freezing, crashing, and other malfunctions". A
  system-managed page file grows up to three times RAM or 4 GB, at most one-eighth of the
  volume, when the charge passes 90% of the limit. [docs]
  <https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/introduction-to-the-page-file>

Measured on this machine (`target/ckpt-research/win-trim.ps1`, one scratch `node` 22.22.2
process holding 800 `Float64Array`s of 1 MB). [measured]

| Step | Working set | Private (commit) | Compression store |
| --- | --- | --- | --- |
| Running | 839 MB | 822 MB | 4 047 MB |
| Suspended (`NtSuspendProcess`) | 839 MB | 822 MB | 4 047 MB |
| Trimmed, 0 s (`EmptyWorkingSet`: 61 to 75 ms) | 0 | 822 MB | 4 047 MB |
| Trimmed, 20 s, regular data | 0 | 822 MB | 4 048 MB (+1) |
| Trimmed, 20 s, random data | 0 | 823 MB | 4 726 MB (+679) |
| Process ended | — | — | — |

- The commit of the process did not move while it was suspended and trimmed. It went back
  to the system only when the process ended: the system commit fell by about 850 MB.
  [measured]
- With a regular heap, the store took almost nothing, so the 822 MB became free RAM. With
  random data, the store grew by the full size, so the RAM did not become free until the
  pages went on to the page file. A real agent heap lies between the two. The system's
  "available" number moved by a few hundred MB either way, but the owner's machine was busy,
  so that number is noise here. [measured; inferred]
- The page file on this machine is system-managed: 45 GB allocated, 7.6 GB in use, peak
  12.9 GB. The commit limit is about 76 GB, with 59 GB in use. [measured] So commit is a
  real limit on this machine: every idle console still charges its full private size.
  [inferred]

What Suspended plus trim gives on Windows: the RAM, at once for a regular heap, after
compression or a page-file write otherwise. What it does not give: commit, page-file space,
the compression store's own RAM for poorly compressible data, kernel objects (handles,
threads, the console host), and survival across a daemon restart or a reboot. [inferred]

### Q1.2 Linux

- **Without swap or zram,** anonymous memory (the V8 or JavaScriptCore heap) cannot leave
  RAM. Only file-backed pages can be dropped: the executable, shared libraries and mapped
  files. `MADV_PAGEOUT` says "If a page is anonymous, it will be swapped out"; with no swap
  there is nowhere to put it. [docs] <https://man7.org/linux/man-pages/man2/madvise.2.html>
  [inferred for the no-swap case]
- **With zram,** the pages are compressed in RAM, as on Windows and macOS. **With swap,**
  they go to disk. [inferred]
- **WSL2** adds swap by default: "25% of memory size on Windows rounded up to the nearest
  GB", in `%Temp%\swap.vhdx`. The VM gets 50% of Windows memory by default. [docs]
  <https://learn.microsoft.com/en-us/windows/wsl/wsl-config> This machine sets
  `memory=16GB` and `swap=8GB`: an 8 GB swap partition, unused. zswap is not built in; zram
  is a module. [measured]
- **Memory freed in the WSL VM goes back to Windows** through free page reporting, a kernel
  patch with Hyper-V support. Page cache is freed later, by the kernel or by
  `autoMemoryReclaim`. [docs]
  <https://devblogs.microsoft.com/commandline/memory-reclaim-in-the-windows-subsystem-for-linux-2/>
  This machine has `autoMemoryReclaim=gradual`. [measured] So a page swapped out inside WSL
  should also shrink the VM's memory on Windows. [inferred; not measured]
- **`process_madvise` from another process** exists since Linux 5.10. For another process it
  takes only `MADV_COLD`, `MADV_PAGEOUT`, `MADV_WILLNEED` or `MADV_COLLAPSE`. The caller
  needs a `PTRACE_MODE_READ_FSCREDS` check on the target and `CAP_SYS_NICE`. [docs]
  <https://man7.org/linux/man-pages/man2/process_madvise.2.html> The WSL peer daemon has no
  capabilities (`CapEff` 0). [measured] So this way is closed to it.
- **`memory.reclaim` in cgroup v2** is a write-only file in every cgroup: write an amount,
  and the kernel reclaims about that much from the cgroup. A nested key `swappiness=` takes
  0 to 200, or `max` for anonymous memory only. [docs]
  <https://docs.kernel.org/admin-guide/cgroup-v2.html> The kernel versions are not verified:
  `memory.reclaim` is from about 5.19, and the `swappiness=` patch series was posted from
  late 2023 to early 2024. [reported; unverified]
  <https://lkml.iu.edu/hypermail/linux/kernel/2401.0/01941.html>
- **Who may write it.** A delegated cgroup's own resource files are not for the delegatee
  to write, since they control the parent's split. [docs] (same page) But in the
  sub-cgroups that the user's systemd creates, every file belongs to the user.
  `memory.reclaim` is owned by `corcino` in `app.slice`, in `ralphy-daemon.service`, and in
  a transient scope. [measured]
- **The experiment** (`target/ckpt-research/reclaim-try.sh`): `systemd-run --user --scope`
  started a Python process holding 600 MB of random bytes. The script froze the scope with
  `cgroup.freeze`, wrote `500M` to `memory.reclaim` (exit 0, 506 ms), and read the result.
  `memory.current` went from 605 to 105 MB, `memory.swap.current` from 0 to 502 MB, and
  RssAnon from 604 to 101 MB. Two seconds after the thaw, the numbers were the same, because
  the idle program did not touch its heap. [measured]
- So the per-console sub-cgroup that the supervision research proposes (`Delegate=yes` on
  Ralphy's unit, or a transient scope for each console) gives both the freezer and a
  targeted page-out, with no new privilege. A scope (`systemd-run --user --scope`) runs the
  command as a direct child, so the PTY is kept. [inferred from the systemd design; the PTY
  part is not measured]

What Suspended plus `memory.reclaim` gives on Linux with swap: nearly all of the RSS. What
it does not give: the swap space (on WSL, a file on the Windows disk), the kernel objects,
and survival across a daemon restart or a reboot. A cgroup also charges swap usage, which
`memory.swap.max` can limit. [inferred; docs for the swap files]

### Q1.3 macOS

- macOS writes pages that are not recently used to a backing store on the boot volume, and
  there is no API that lets one app page out another. [docs]
  <https://developer.apple.com/library/archive/documentation/Performance/Conceptual/ManagingMemory/Articles/AboutMemory.html>
- Since OS X 10.9, the compressor compresses the least recently used pages before it swaps.
  Apple's description is only quoted by secondary sources today. [reported]
  <https://appleinsider.com/articles/13/06/12/compressed-memory-in-os-x-109-mavericks-aims-to-free-ram-extend-battery-life>
- A process stopped with `SIGSTOP` touches no page, so its pages age, and the compressor and
  then swap take them, but only under memory pressure. Ralphy cannot ask for it. [inferred]

### Q1.4 How much of "hibernate the exe" does Suspended already give?

| Platform | RAM freed | Commit or swap freed | Survives a daemon restart |
| --- | --- | --- | --- |
| Windows | Yes: at once for compressible pages, later through the page file | No | No |
| Linux, swap or zram | Yes, with `memory.reclaim` | No: swap or zram holds it | No |
| Linux, no swap | Only file pages | — | No |
| macOS | Under pressure, by the OS | No | No |

A checkpoint to a file would add three things: commit (or swap) given back, the kernel
objects released, and a console that outlives the daemon. The first two have value only on
a machine near its commit or swap limit. The third is the real gain, and Q7 shows why a
restored process cannot use it well. [inferred]

---

## Q2. Checkpoint and restore of a process tree on Linux

### Q2.1 CRIU

**Unprivileged mode.**

- "criu can be used as non-root with either the CAP_SYS_ADMIN capability or with the
  CAP_CHECKPOINT_RESTORE capability introduced in Linux kernel 5.9. CAP_CHECKPOINT_RESTORE is
  the minimum that is required." It also needs `CAP_SYS_PTRACE` or
  `kernel.yama.ptrace_scope` 0, and the flag `--unprivileged` "to accept the limitations".
  [docs] <https://raw.githubusercontent.com/checkpoint-restore/criu/criu-dev/Documentation/criu.txt>
- `--unprivileged` first appears in the man page of CRIU 3.18; it is not in 3.16 or 3.17.
  [docs: `Documentation/criu.txt` at tags v3.16 to v3.19, read with `gh api`]
- `CAP_CHECKPOINT_RESTORE` lets a process write `/proc/sys/kernel/ns_last_pid`, use
  `clone3` `set_tid`, and read `/proc/<pid>/map_files` of other processes. [docs]
  <https://man7.org/linux/man-pages/man7/capabilities.7.html>
- The capability is granted to the `criu` binary by root once (a file capability). A
  kernel maintainer said in the review: "CAP_CHECKPOINT_RESTORE *is* a privileged process."
  [reported] <https://lkml.iu.edu/hypermail/linux/kernel/2006.1/04238.html>
- The Linux Plumbers 2022 talk on unprivileged CRIU says some workloads also need
  `CAP_NET_ADMIN`, `CAP_SYS_PTRACE`, `CAP_SYS_CHROOT`, `CAP_SYS_RESOURCE` or `CAP_SETUID`.
  [reported: search summary of the slides; the PDF could not be read here]
  <https://lpc.events/event/16/contributions/1242/attachments/1129/2194/Unprivileged%20CRIU%20-%20Linux%20Plumbers%202022.pdf>
- **Seccomp.** To inject its parasite code, CRIU suspends the target's seccomp filter
  (`PTRACE_O_SUSPEND_SECCOMP`). The kernel allows that only `if (capable(CAP_SYS_ADMIN))`
  and only when the caller's own `seccomp_mode` is `SECCOMP_MODE_DISABLED`. [docs]
  <https://raw.githubusercontent.com/torvalds/linux/master/kernel/ptrace.c> So unprivileged
  CRIU cannot dump any task with a seccomp filter. Agent sandboxes on Linux often use
  seccomp (bubblewrap). [inferred]

**WSL2.**

- The WSL kernel has `CONFIG_CHECKPOINT_RESTORE`, `CONFIG_MEM_SOFT_DIRTY`,
  `CONFIG_USERFAULTFD`, the socket diag options, `CONFIG_PID_NS` and `CONFIG_TIME_NS`.
  [measured: `/proc/config.gz`]
- Every process in this WSL has a seccomp filter: PID 1, the user's systemd and the Ralphy
  peer daemon (`Seccomp: 2`, `Seccomp_filters: 1`). [measured] WSL issue #9993 reported this
  in 2023 as "breaking CRIU"; it was fixed in WSL 1.2.1, and a maintainer warned that
  "seccomp filters may exist in a potential future". [reported]
  <https://github.com/microsoft/WSL/issues/9993> They exist again in WSL 3.0.5. [measured]
- CRIU issue #2391 shows `suspending seccomp failed: Operation not permitted` on WSL2. A
  maintainer answered that the check "requires CRIU to be executed without any seccomp
  filters", and another said CRIU on WSL "has not been well tested". [reported]
  <https://github.com/checkpoint-restore/criu/issues/2391>
- Ubuntu 22.04's CRIU 3.16.1 (unpacked without install) fails on this kernel before it
  dumps anything: `vdso: Unexpected rt vDSO area bounds`, then `Could not initialize kernel
  features detection`. It also does not know `--unprivileged`. That was inside
  `unshare -Urpf`, where it had every capability of its own namespace. [measured] A current
  CRIU (4.3.1) needs a build with protobuf headers, which was not possible without sudo.
  [measured: the dev packages are not installed]

**PTYs.**

- CRIU dumps "an external TTY, identified by st_rdev and st_dev" with `--external tty`, and
  on restore `--inherit-fd` "lets criu use an already opened file descriptor" in its place.
  `--shell-job` lets the restored task "inherit session and process group ID from the criu
  itself". [docs] (man page above)
- The earlier note recorded a 2023 patch that lets `--shell-job` work with
  `CAP_CHECKPOINT_RESTORE` instead of `CAP_SYS_ADMIN`. [reported]
  <https://lkml.iu.edu/hypermail/linux/kernel/2312.1/00626.html>
- For Ralphy, the pty master belongs to the daemon, which is outside the dumped tree. So the
  slave is external: on restore, the daemon opens a new pty pair and passes the new slave
  with `--inherit-fd`. The agent then runs on a new terminal with the old terminal modes,
  and draws nothing until it gets a `SIGWINCH` or input. [inferred]

**TCP and TLS.**

- `--tcp-established` checkpoints established connections; without it, CRIU refuses a
  task that has one. `--tcp-close` drops them instead. `--network-lock` sets how the
  network is locked during the dump. [docs] (man page above)
- The kernel feature behind it, TCP repair, needs `CAP_NET_ADMIN`. [reported: listed among
  the extra capabilities in the Plumbers talk; the exact kernel check is not verified here]
- A restore restores only Ralphy's side. Long before the restore, the model API's server or
  load balancer has timed out and dropped its side, so the first packet gets a reset. TLS
  lives in the process's memory (OpenSSL or BoringSSL), so it comes back exactly, but on a
  dead socket. [inferred] CRIU issue #454 shows "Connection reset by peer" after a restore
  with changed connection details. [reported]
  <https://github.com/checkpoint-restore/criu/issues/454>

**PIDs.**

- CRIU restores each task with its old PID, through `clone3` `set_tid` (Linux 5.5) or
  `ns_last_pid`. `set_tid` "requires CAP_SYS_ADMIN or (since Linux 5.9)
  CAP_CHECKPOINT_RESTORE" in each user namespace that owns one of the PID namespaces. If the
  PID "already exists", `clone3` fails with `EEXIST`. [docs]
  <https://man7.org/linux/man-pages/man2/clone.2.html>
- Outside a PID namespace, a restore fails when any old PID of the tree is in use. With
  `pid_max` at 4 194 304 here, a clash is rare but possible, and more likely after a reboot,
  when PIDs start low again. [measured: `pid_max`; inferred]
- A PID namespace per console avoids the clash. Unprivileged, that means a user namespace
  too (`unshare -Urp` works here). [measured] But the seccomp check and the `map_files`
  read are made against the initial namespace, so the namespace does not lift them.
  [docs for `capable()`; inferred for `map_files`]
- `/proc/sys/kernel/ns_last_pid` is mode 0666 here, but the write is checked against the
  capability. [measured: file mode; docs: capabilities(7)]

**`io_uring`.**

- CRIU fails with `Unknown shit 600 (anon_inode:[io_uring])` on any task with an
  `io_uring` mapping. Issue #2131 ("io_uring support in CRIU") is open; an earlier pull
  request stopped. The workaround in the thread is `sysctl kernel.io_uring_disabled=2`, a
  root, system-wide setting. [reported]
  <https://github.com/checkpoint-restore/criu/issues/2131>,
  <https://github.com/checkpoint-restore/criu/issues/2673>
- Node.js 22.23.5 and 24.13.0 (libuv 1.51.0) map an `io_uring` ring here, idle with no
  file operation, and also with `UV_USE_IO_URING=0`: three `io_uring` descriptors per
  process. [measured] libuv 1.50.0 (2025-01) "always use[s] io_uring for epoll batching".
  [docs] <https://github.com/libuv/libuv/blob/v1.x/ChangeLog> So OpenCode and other Node
  agents cannot be dumped by CRIU on kernels where `io_uring` is on. [inferred]
- Claude Code 2.1.323 (Bun) on the WSL peer has no `io_uring` descriptor. [measured] Six of
  its processes there have 268 to 274 MB RSS each. [measured]

**Image size and time.**

- An image holds the resident anonymous memory plus small state files. DMTCP measured it
  here: 843 MB for 844 MB RSS (Q2.2). [measured, for DMTCP]
- A 2024 study found CRIU checkpoint time "linearly proportional to the size of the memory
  allocation of the container", with restore much faster. [paper]
  <https://journals.riverpublishers.com/index.php/JWE/article/view/23635>
- `--pre-dump` and `--track-mem` copy memory in rounds while the task runs; `--lazy-pages`
  restores with memory loaded on demand. [docs] (man page above) These shorten the stop;
  they do not shrink the image. [inferred]
- For ten idle consoles of about 1 GB, the images need about 10 GB of disk. On WSL that
  lives in the distro's ext4 VHDX on the Windows disk. [inferred]

**cgroup v2.** `--manage-cgroups` saves and restores the cgroup tree; `--cgroup-yard` and
`--cgroup-root` work with cgroups prepared by someone else. [docs] (man page above)

### Q2.2 DMTCP

- DMTCP is maintained: 4.2.0 is from 2026-06-26, and the repository had pushes on
  2026-11-05. [docs] <https://github.com/dmtcp/dmtcp/releases> It runs as a plain user,
  with no kernel module, by wrapping libc. A program must be started by `dmtcp_launch` and
  joined to a coordinator. [docs; measured]
- Built from source here as a plain user, with gcc and make only. [measured]
- **Results** (`target/ckpt-research/dmtcp-*.sh`, private coordinators). [measured]

  - `sleep 300`: a 3.6 MB image in about 0.4 s.
  - Python with 844 MB RSS of random bytes, default gzip: save and end 18.2 s; image
    808 MB; restart to the first tick 3.5 s.
  - The same with `--no-gzip`: save and end 1.1 s; image 843 MB; restart 2.0 s; the counter
    went on from where it stopped.
  - Python with 225 MB under `script(1)` (a PTY), gzip: save 4.9 s; restart 1.2 s; it went
    on, with the warning "Controlling terminal not bound to a terminal device".
  - Node.js 22.23.5: aborts at start, `JASSERT` in `procselfmaps.cpp`, called from V8's
    `GetStackStart`.
  - Node.js 24.13.0: the same abort.
  - Claude Code 2.1.323 (Bun), `--version` only: aborts at start, `JASSERT` in
    `threadwrappers.cpp` `pthread_create`.

- The restored Python saw the same PID (40000) before and after, because DMTCP gives every
  process a virtual PID. The real PIDs differed. [measured]
- `dmtcp_command -bc` (blocking checkpoint) did not block in 4.2.0: the coordinator logged
  `unhandled user command; (cmd = b)` and checkpointed in the background. `-kc` (checkpoint,
  then kill) worked. [measured]
- PTYs: DMTCP saves and restores PTYs it created. The restored program warned that its
  controlling terminal was not bound, because the restart had no terminal. A daemon would
  have to start `dmtcp_restart` on a new PTY. [measured; inferred]
- Fit for Ralphy: every console would start under `dmtcp_launch`. Its `LD_PRELOAD` wrapper
  must be correct for every agent runtime, and today it is not for V8 or Bun. [inferred]

### Q2.3 Containers and sandboxes

- **`docker checkpoint`** is "experimental", needs CRIU, and fails for containers started
  with `-t`. "`seccomp` is only supported by CRIU in very up-to-date kernels." [docs]
  <https://docs.docker.com/reference/cli/docker/checkpoint/>
- **`podman container checkpoint`** freezes the container's cgroup during the dump, and
  needs `--tcp-established` to restore established TCP connections. Its page does not say
  whether rootless containers can be checkpointed. [docs; rootless status unverified]
  <https://docs.podman.io/en/latest/markdown/podman-container-checkpoint.1.html>
- **gVisor** (`runsc checkpoint`) saves its own user-space kernel, without CRIU. After a
  restore, "Sockets that were connected at checkpoint time return `ECONNRESET`". The restore
  host must have every CPU feature of the source. `CLOCK_REALTIME` jumps by the time spent
  checkpointed. [docs] <https://gvisor.dev/docs/user_guide/checkpoint_restore/>
- All three are Linux only, and each moves the agent into a container that must see the
  operator's checkout, credentials and tools. [inferred]

### Q2.4 Real reports of checkpointing Node.js or V8

- **Success (CRIU):** Node.js issue #66880 (2026-10) measured random generators after
  restores. "14 of 14" blocks were identical between two restores of one CRIU image of
  Node.js 24.18.0 (Fedora 44, Linux 7.2.11). So CRIU restored that Node.js twice, but the
  report does not say how `io_uring` was handled. Restored copies repeat `Math.random`,
  `crypto.randomBytes` and `crypto.randomUUID`. [reported]
  <https://github.com/nodejs/node/issues/66880>
- **Failure (CRIU):** dumps of Redis and vLLM containers fail on the `io_uring` mapping.
  [reported] (#2673, #2131 above)
- **Failure (DMTCP):** Node.js 22 and 24 and the Bun-based Claude Code abort at launch here.
  [measured]
- **Research:** "Prebaking Functions to Warm the Serverless Cold Start" (ACM Middleware
  2020) restores function processes from CRIU images. The runtimes it covers are not
  verified here. [paper] <https://5ujinkang.github.io/prebaking-serverless-warmstart/>
- An older Node.js issue, "Checkpoint and Restore" (#17103), is closed. [reported]
  <https://github.com/nodejs/node/issues/17103>

---

## Q3. Windows

- **`PssCaptureSnapshot`** "Captures a snapshot of a target process". Microsoft says the data
  "is typically used for diagnostic purposes": threads, handles, the address space, and
  performance data. A snapshot is read with `PssQuerySnapshot` and `PssWalkSnapshot`.
  Nothing in the API makes a process from a snapshot. [docs]
  <https://learn.microsoft.com/en-us/windows/win32/api/processsnapshot/nf-processsnapshot-psscapturesnapshot>,
  <https://learn.microsoft.com/en-us/previous-versions/windows/desktop/proc_snap/overview-of-process-snapshotting>
- **Minidumps:** `MiniDumpWriteDump` "Writes user-mode minidump information to the specified
  file" for a debugger. The handle needs `PROCESS_QUERY_INFORMATION` and `PROCESS_VM_READ`.
  No API loads a dump back into a live process. [docs]
  <https://learn.microsoft.com/en-us/windows/win32/api/minidumpapiset/nf-minidumpapiset-minidumpwritedump>
- **Application Recovery and Restart** lets an application save its own data before it
  crashes or hangs, and be restarted. It needs the application's cooperation. [docs]
  <https://learn.microsoft.com/en-us/windows/win32/recovery/application-recovery-and-restart-portal>
- **Windows Sandbox** is "a disposable virtual machine": "closing it deletes all software,
  files, and state". Since 22H2, data persists only through restarts inside the sandbox.
  One instance at a time; Pro, Enterprise and Education. It has no save or checkpoint.
  [docs]
  <https://learn.microsoft.com/en-us/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-overview>
- **Hyper-V:** a standard checkpoint "takes a snapshot of the virtual machine and virtual
  machine memory state"; a production checkpoint takes no memory. `Save-VM` is "similar to
  hibernating a physical computer". [docs]
  <https://learn.microsoft.com/en-us/virtualization/hyper-v-on-windows/user-guide/checkpoints>,
  <https://learn.microsoft.com/en-us/powershell/module/hyper-v/save-vm> These act on a whole
  VM, through the Hyper-V module, which needs admin rights or Hyper-V Administrators.
  [inferred from the module; not verified]
- **WSL2:** `wsl.exe` has `--terminate`, `--shutdown`, `--export` (a tar or VHD of the file
  system) and `--manage`, but no suspend, pause or save. [measured: `wsl --help`]
  `.wslconfig` has `vmIdleTimeout` and `instanceIdleTimeout`, which shut down, and no key
  that saves state. [docs] (wsl-config page above) A search of the WSL issues for "suspend
  VM save state" and "hibernate wsl vm" found nothing. [measured: `gh search issues`] So a
  WSL distro cannot be saved with its processes. [inferred]
- **Research and products:**
  - A checkpoint library for Windows NT appeared at the 2nd USENIX Windows NT Symposium
    (1998), for long-running multi-threaded programs. [paper]
    <https://www.usenix.org/conference/2nd-usenix-windows-nt-symposium/transparent-checkpoint-facility-nt>
  - Drawbridge (Microsoft Research) ran Windows applications on a library OS in a
    "picoprocess", with migration as a goal. Bascule (EuroSys 2013) added checkpointing
    as an extension. The project "has reached its conclusion". [paper; docs]
    <https://www.microsoft.com/research/project/drawbridge/>,
    <https://www.cs.purdue.edu/homes/pfonseca/papers/eurosys2013-bascule.pdf>
  - A family of US patents describes the checkpoint, migration and "checkpoint-terminate"
    of groups of Windows applications. The search did not confirm the owner. Products of
    that time (for example Librato or Evergrid) are not verified. [reported]
    <https://patents.google.com/patent/US9201737>

**Verdict for Windows:** no supported or practical way exists to save a plain Win32 process
and restore it later. Only a whole VM can be saved. [docs; inferred]

---

## Q4. macOS

- **`task_suspend`** raises the task's suspend count, and each call needs a `task_resume`.
  It needs the task port. On a current macOS, `task_for_pid` on another process needs root
  or a debugger entitlement. [docs for the call; inferred for the entitlement, as in the
  earlier note]
  <https://opensource.apple.com/source/xnu/xnu-344/osfmk/man/task_suspend.html.auto.html>
  It suspends a process. It does not save one. [docs]
- **App Nap** lowers priority, timer rates and I/O for an app that "isn't the foreground
  app" and has not "recently updated content in the visible portion of a window". Those
  terms are about windowed apps; the page does not mention command-line processes. It
  saves nothing to disk. [docs]
  <https://developer.apple.com/library/archive/documentation/Performance/Conceptual/power_efficiency_guidelines_osx/AppNap.html>
- **No process checkpoint tool exists for macOS.** CRIU, DMTCP and gVisor are Linux only.
  [inferred: no primary source names one]
- **VM level:** the Virtualization framework has `saveMachineStateTo` and
  `restoreMachineStateFrom` for a paused VM, and `validateSaveRestoreSupport()` from
  macOS 14. Developers report that it compiles only for Apple silicon, and that restores
  fail when the MAC address changes. [docs; reported]
  <https://developer.apple.com/documentation/virtualization/vzvirtualmachineconfiguration/validatesaverestoresupport()>,
  <https://developer.apple.com/forums/thread/745168>

---

## Q5. VM or sandbox level

The options, by host (sources in the bullets below; the host list for QEMU is [inferred]):

| Option | Host | What it saves |
| --- | --- | --- |
| Firecracker | Linux with KVM | A full or diff snapshot: device state and guest memory |
| QEMU | Linux, Windows (WHPX), macOS (HVF) | `savevm`, or a migration to a file |
| Hyper-V | Windows Pro and up, admin rights | A standard checkpoint, `Save-VM` |
| WSL2 VM | Windows | Nothing: there is no save verb |
| Apple Virtualization | macOS 14+, Apple silicon | `saveMachineStateTo` |

Each option costs a full guest per console: its kernel, its page cache, and the agent with
its tools. [inferred]

- Firecracker loads the memory file `MAP_PRIVATE`, on demand. "Guest network connectivity
  is not guaranteed to be preserved after resume." vsock connections close. A snapshot must
  resume on identical software and hardware. Resuming one state twice is unsafe unless
  uniqueness is restored (VMGenID reseeds the guest kernel, not the applications). [docs]
  <https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md>
  The VMM's own memory is "less than 5 MiB" per instance. [reported] (search summary,
  fly.io and others)
- QEMU saves VM snapshots inside qcow2 images. [docs]
  <https://www.qemu.org/docs/master/system/images.html>
- Nested KVM in WSL is off on this machine (`nestedVirtualization=false`). [measured]
- **Realism for a desktop tool** [inferred]:
  - Memory: every console carries its own guest kernel and page cache, and the agent's own
    heap is unchanged. While a console runs, the VM costs more than the process.
  - Start: the first start needs an image with the agent, git, a shell and the agent's
    tools. The repository must be shared into the guest (virtiofs or 9p), which is slow on
    Windows hosts and changes file-watch and lock behavior.
  - Disk: one memory file per saved console, the size of its guest memory.
  - Fit: the agent would run in another OS than the operator's checkout. That changes
    paths, credentials, the agent's own config, and the security model (ADR-0072). It is a
    new product shape, not an idle state.

---

## Q6. Runtime level

- **Node.js startup snapshots:** `--build-snapshot` "Generates a snapshot blob when the
  process exits", from "a single entrypoint", which "can load built-in modules, but not
  additional user-land modules". Unserializable resources must be released in
  `addSerializeCallback` and made again in `addDeserializeCallback`. "The application
  deserialized from a user-land snapshot cannot be snapshotted again." [docs]
  <https://nodejs.org/api/cli.html>, <https://nodejs.org/api/v8.html>
- **V8:** custom startup snapshots serialize a context after running a script, at build
  time. [docs] <https://v8.dev/blog/custom-startup-snapshots>
- **Heap snapshots:** `v8.writeHeapSnapshot` writes JSON for "tools such as Chrome
  DevTools"; "The JSON schema is undocumented ... and may change". It needs about twice the
  heap, and it blocks the event loop. It cannot be loaded back. [docs]
  <https://nodejs.org/api/v8.html>
- **Bun and JavaScriptCore:** `Bun.generateHeapSnapshot` is for the inspector. `--bytecode`
  with `--compile` caches parsed code, which is tied to one JavaScriptCore version. No
  document describes saving a running program's state. [docs]
  <https://bun.com/docs/bundler/bytecode>,
  <https://bun.com/reference/bun/generateHeapSnapshot>
- **Without the agent's cooperation:** a startup snapshot is made by the program's own
  build. Ralphy cannot inject one into a released agent CLI, and a running program with
  sockets, timers, child processes and native handles cannot be serialized by the runtime.
  The only state an agent CLI offers to restore is its session on disk, through its resume
  flag. [inferred]

---

## Q7. What a restored process would face

Each item gives the restored process first, then the agent's resume by id.

- **Terminal.** Restored: Windows has no tool at all. On Linux, a new pty slave comes in by
  `--inherit-fd`, with the old terminal modes, and nothing is drawn until a `SIGWINCH` or
  input. Resume: a new PTY, and the agent draws at start (ADR-0083 D9 answers its cursor
  query).
- **Model API connection.** Restored: the socket is reset, and an HTTP/2 stream that was
  open at the dump is lost. TCP repair also needs `CAP_NET_ADMIN`. Resume: a new
  connection.
- **Local MCP servers.** Restored: in the image only if they are in the tree, and their own
  sockets and children have the same problems. Resume: the agent starts them again.
- **Remote MCP servers.** Restored: the connection is reset, and a server-side session may
  be gone. Resume: a new session.
- **Child processes.** Restored: every one must be dumpable (no `io_uring`; no seccomp for
  unprivileged CRIU). Resume: not needed.
- **Files and locks.** Restored: files may have changed. A file offset or a view of the
  repository may be stale, and a lock may now be held by someone else. CRIU can restore
  locks (`--file-locks`), which may then conflict. Resume: the agent reads the files again.
- **Auth tokens.** Restored: tokens in memory may have expired, and a refresh may work or
  not. Resume: read from the agent's store at start.
- **Random state.** Restored: it repeats if one image is restored twice. Resume: fresh.
- **PIDs.** Restored: they must be free, or each console needs its own PID namespace.
  Resume: any PID.
- **Versions.** Restored: the image pins the old binary, so an agent update on disk is
  ignored until the next start. Resume: the current version.
- **Daemon restart or reboot.** Restored: possible only if the image is kept, and an image
  taken under an older kernel may fail. Resume: works, by design.

Sources: CRIU man page for `--inherit-fd`, `--tcp-established`, `--file-locks`; gVisor for
`ECONNRESET`; Node.js issue #66880 for random state; RFC 9113 §6.7 for HTTP/2 PING (in the
supervision research). The items are [docs] where a source is named and [inferred] for the
rest.

A frozen process (Suspended) faces the same network, file and token changes after a long
freeze. ADR-0083 accepts them because the process lives and can retry, and because the
blocker "the agent is in a turn" keeps a stream from being cut. A restored process has more
to lose, and it gains only that it outlives the daemon. [inferred]

---

## A third inactive state?

Short cells; the details and labels are in Q1 to Q7. "Disk = RSS" means an image about the
size of the resident memory.

### Windows

| Means | Privilege | Cost | Risk | Verdict |
| --- | --- | --- | --- | --- |
| Save a Win32 process | — | — | — | None exists |
| Process snapshot, minidump | `PROCESS_VM_READ` | Disk = private size | No restore | Diagnostics |
| Hyper-V VM per console | Admin; Pro | A guest per console | New product | Not realistic |
| Save the WSL VM | — | — | — | None exists |

### Linux (native and WSL)

| Means | Privilege | Cost | Risk | Verdict |
| --- | --- | --- | --- | --- |
| CRIU as root | Root | Disk = RSS; ~1 s/GB | Seccomp, `io_uring`, TCP, PIDs | Not for a daemon |
| CRIU `--unprivileged` | `setcap` by root | Disk = RSS | Same; no TCP repair | Blocked here |
| DMTCP | None | Disk = RSS; 1.1 s/GB | Fails on Node.js, Bun | Not usable today |
| Freeze + `memory.reclaim` | None (own cgroup) | Swap = RSS | Needs swap | **Use in Suspended** |

- CRIU "~1 s/GB" is the DMTCP figure measured here, used as a stand-in; CRIU itself
  did not run. [measured for DMTCP; inferred for CRIU]
- "Blocked here": every WSL process has a seccomp filter, and Node.js 22 and 24 map an
  `io_uring` ring. The last row is not a checkpoint; it is the page-out of Q1.2.

### macOS

| Means | Privilege | Cost | Risk | Verdict |
| --- | --- | --- | --- | --- |
| Save a process | — | — | — | None exists |
| `SIGSTOP`, compressor | None | Managed by the OS | Job-control traps (D5) | Already Suspended |
| Virtualization VM | Entitlement; macOS 14+ | A guest per console | New product | Not realistic |

---

## Mapping to ADR-0083

- **Context.** The sentence "An unprivileged process cannot page out another one, and with
  no swap the heap cannot leave memory at all" needs a correction. An unprivileged daemon
  can page out a cgroup it owns, with `memory.reclaim`. Without swap or zram, it still
  cannot. [measured]
- **D2 (two inactive states).** Keep two states; there is no third, checkpointed state.
  Suspended on Linux should write `memory.reclaim` after `cgroup.freeze` when swap exists.
  Then "it needs swap on Linux" describes Suspended's memory gain, not a trim-only state.
  On Windows, Suspended frees RAM but keeps commit; the settings text and the log line
  could say so. [measured; inferred]
- **D5.** The per-console sub-cgroup already proposed for the freezer is the same object
  that `memory.reclaim` needs. A transient user scope per console is a second way to get
  it, besides `Delegate=yes` on Ralphy's unit. [measured; inferred]
- **D7 (only a known session id can be Hibernated).** A checkpoint does not remove this
  limit for OpenCode, Codex and Kimi. CRIU cannot dump Node.js with `io_uring`, DMTCP
  cannot start Node.js here, and Windows has no means at all. These agents stay at
  Suspended. The open question stays as it was: a launch gate per repository and agent.
  [measured; docs]
- **D13 (after a daemon start, nothing starts without a click).** A checkpoint image does
  not make a console come back live after a daemon restart: its PTY, sockets and kernel
  objects are gone, and on Windows no image exists. D13 stays as written. [inferred]
- **Considered options.** Add one entry, for example:
  "**Save the process tree to disk (CRIU, DMTCP, a VM snapshot) as a third state.**
  Windows has no means for a Win32 process. On Linux, CRIU needs root or a capability
  granted by root, cannot dump a task under seccomp (every WSL process) or with `io_uring`
  (Node.js 22 and 24), and does not keep TCP connections. DMTCP fails to start Node.js and
  Bun. A VM per console is a new product. A restored agent meets reset connections, a new
  terminal and old tokens, while its resume flag reads the conversation again. Rejected
  (research of 2026-11-15)." [inferred from this note]

---

## What to measure

1. **`memory.reclaim` on a real idle agent** in the WSL peer, in its own scope:
   RSS and swap before and after, the time to the first frame after thaw, and whether the
   agent's timers pull pages back while it is frozen (they should not) and after the thaw.
2. **Whether WSL hands the swapped-out memory back to Windows:** the `vmmem` (or
   `VmmemWSL`) working set before and after a reclaim, with `autoMemoryReclaim=gradual`.
3. **The compression ratio of real agent heaps on Windows:** the growth of the Memory
   Compression process per GB trimmed, for OpenCode, Claude Code and Codex. The two
   scratch cases here were 0% and 100%.
4. **Commit pressure:** the share of the system commit that idle consoles hold on the
   owner's machine, and whether the system page file grows because of them. This is the
   number that would justify Hibernated for agents that resume.
5. **A current CRIU (4.3) on a native Linux machine** with a Node.js agent, with
   `kernel.io_uring_disabled=2` in a VM, to see whether the `io_uring` block is the only one.
   Only on a lab machine, never on the owner's.
6. **Whether a PTY is kept** when a console starts through `systemd-run --user --scope`.

---

## Gaps

- criu.org was not reachable, so the CRIU wiki pages on TTYs, TCP and unprivileged mode were
  not read. The man page, the source and the issues were used instead. [gap]
- The Plumbers 2022 slides could not be read here; their content comes from a search
  summary. [gap]
- The kernel versions for `memory.reclaim` (about 5.19) and its `swappiness=` key are not
  verified. [gap]
- Node.js issue #66880 restored Node.js 24 under CRIU, but it does not say how the
  `io_uring` ring was handled. Fedora's default for `io_uring_disabled` is not checked.
  [gap]
- A current CRIU could not be built without sudo, so no CRIU dump ran here. The WSL
  failure is inferred from the seccomp state, the kernel source and issue #2391. [gap]
- The right that the Hyper-V cmdlets need, and whether Podman can checkpoint rootless
  containers, are not verified. [gap]
- No primary Microsoft figure for the compression ratio of the memory compression store was
  found. [gap]
- Commercial Windows checkpoint products before 2010 are known here only from patents.
  [gap]
- macOS was not available for a check. [gap]
