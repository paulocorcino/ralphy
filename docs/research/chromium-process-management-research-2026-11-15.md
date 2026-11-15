# How Chromium manages its processes: a reference for Ralphy

Date read: 2026-11-15. Unless a line says otherwise, the Chromium source was read at
`main` on chromium.googlesource.com on that date. The code changes often, so a file can
move or change after this date.

Scope: how Chromium knows, measures, shows, freezes, discards and ends its own child
processes. General OS supervision (job objects in general, cgroups, kqueue), freeze safety,
the ski-rental timeout theory and Kubernetes PLEG are out of scope (another research note
covers them).

Labels: **[source]** Chromium code; **[docs]** an official doc or design doc;
**[reported]** a third party; **[inferred]** my own reasoning, with no URL.

Short names for URLs used many times:

- `GS` = `https://chromium.googlesource.com/chromium/src/+/main/`
- `CDP` = `https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/`

## Findings for Ralphy

1. **The registry is built from the launcher, not from OS events.** Chromium knows a child
   because the browser process launched it and holds a host object for it
   (`ChildProcessLauncher`, `BrowserChildProcessHost`, `RenderProcessHost`). Observers get
   "launched and connected", "disconnected", "crashed", "killed", "launch failed" and
   "exited normally" callbacks from those hosts. Nothing watches the OS process table.
   **[source]** `GS content/public/browser/browser_child_process_observer.h`.
   Ralphy can do the same for the console's root process, but not for the grandchildren
   an agent CLI starts: Chromium forbids those (see 3). **[inferred]**
2. **Every process has two ids, and the PID is not the identity.** The browser gives each
   child host a unique id from a counter (`ChildProcessHost::GenerateChildProcessUniqueId`),
   and the OS PID is a separate field. The Performance Manager says of the PID: "Refrain
   from using this as a unique identifier as on some platforms PIDs are reused
   aggressively." **[source]** `GS content/browser/child_process_host_impl.cc`,
   `GS components/performance_manager/public/graph/process_node.h`. The extension API
   shows both: `id` ("provided by the browser") and `osProcessId`. **[docs]**
   <https://developer.chrome.com/docs/extensions/reference/api/processes>
3. **Chromium's renderers cannot have children.** On Windows each renderer runs in its own
   job object with "One active process limit (disallows creating child processes)", and
   every sandbox job level sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. **[docs]**
   `GS docs/design/sandbox.md`; **[source]** `GS sandbox/win/src/job.cc`. This is the
   largest difference from Ralphy: an agent CLI spawns shells, MCP servers and tools that
   Ralphy never launched. Ralphy must discover them; Chromium never has to. **[inferred]**
4. **The Task Manager samples only while someone watches, and only what is shown.** The
   first observer starts updating and the last one stops it (`StartUpdating` /
   `StopUpdating`); the interval is the smallest interval any observer asks for, and the
   metrics are the bitwise OR of the flags the observers ask for. The Task Manager window
   asks for 2 seconds and turns a metric on only when its column is visible.
   **[source]** `GS chrome/browser/task_manager/task_manager_interface.cc`,
   `GS chrome/browser/ui/task_manager/task_manager_table_model.cc`.
5. **The Performance Manager is the analog of the module the owner wants.** It is one graph
   of Page, Frame, Process, Worker and System nodes on the browser's main thread, with
   observers per node type, "decorators" that attach measured data, "policies" that decide,
   and "mechanisms" that act. Freezing, discarding, Memory Saver and working-set trimming
   are all policies on this graph. **[docs]** `GS components/performance_manager/README.md`.
   The Task Manager is a separate, older system with its own providers; the two do not
   share a model today. **[source]** `GS chrome/browser/task_manager/sampling/task_manager_impl.cc`.
6. **Opt-outs are a closed, named list, and the browser decides.** Freezing has 23 named
   `CannotFreezeReason` values and discarding has about 30 `CannotDiscardReason` values
   (visible, audible, capturing, device connected, form interaction, pinned, DevTools
   open...). Each reason has a short text that `chrome://discards` shows. **[source]**
   `GS components/performance_manager/freezing/cannot_freeze_reason.cc`,
   `GS chrome/browser/performance_manager/policies/discard_eligibility_policy.cc`.
   This matches ADR-0083 D3 ("blockers"); Ralphy can copy the shape: an enum of reasons,
   each with a label the workbench shows. **[inferred]**
7. **Chromium also does "freeze, then trim, then discard if it grows".** A working-set
   trimmer policy empties a process's working set when all its frames are frozen
   (`OnAllFramesInProcessFrozen`), and the freezing policy discards a frozen page whose
   private memory footprint grew after the freeze (`FROZEN_WITH_GROWING_MEMORY`).
   **[source]** `GS chrome/browser/performance_manager/policies/working_set_trimmer_policy.h`,
   `GS components/performance_manager/freezing/freezing_policy.h`. Today the trimmer
   policy is built only on ChromeOS. **[source]** `.../working_set_trimmer_policy.cc`.
   This supports the order in ADR-0083 D2 (suspend = stop, then trim; hibernate = end).
8. **Memory is "private memory footprint", one formula per OS.** Windows: private bytes.
   Linux, ChromeOS, Android: anonymous resident + swap. macOS: `phys_footprint` minus shared
   resident. **[source]**
   `GS services/resource_coordinator/memory_instrumentation/queued_request_dispatcher.cc`.
9. **The ChromeOS Task Manager already shows processes from another PID namespace.**
   Tasks that run in a VM (ARC, Crostini) are kept in a separate map,
   `arc_vm_task_groups_by_proc_id_`, from host tasks. **[source]** `task_manager_impl.cc`.
   This is the closest Chromium analog of a Ralphy peer (WSL): a PID is only unique inside
   one machine or VM, so the key must include the peer. **[inferred]**
10. **Remote viewers read little.** The DevTools protocol gives `SystemInfo.getProcessInfo`
    (type, PID, cumulative CPU seconds per process, browser target only), the `Performance`
    domain (named metrics for one page), and memory dumps through `Tracing`. There is no
    remote "end process" and no cross-machine process view. **[docs]** `CDP SystemInfo.pdl`,
    `CDP Performance.pdl`, `CDP Tracing.pdl`.

## 1. How Chromium knows every child process

### The browser process is the only launcher

- `ChildProcessLauncher` "Launches a process asynchronously and notifies the client of the
  process handle when it's available", because creating a process "can take > 100 ms".
  Its client gets `OnProcessLaunched()` or `OnProcessLaunchFailed(int error_code)` on the
  thread that made the launcher. **[source]** `GS content/browser/child_process_launcher.h`
- The launcher keeps the process: `GetProcess()` returns a `const base::Process&`, and "If
  the process is still running by the time this object destructs, it will be terminated."
  **[source]** same file.
- On Windows the helper launches through `sandbox::policy::SandboxWin::StartSandboxedProcess`
  (or `base::LaunchProcess` for an elevated child), on the launcher thread
  (`LaunchProcessOnLauncherThread`). **[source]**
  `GS content/browser/child_process_launcher_helper_win.cc`
- Two host families sit on top. Renderers are `RenderProcessHost`; other children
  (GPU, network, utility, etc.) are `BrowserChildProcessHost`. Observers of renderers get
  `RenderProcessReady`, `RenderProcessExited(host, ChildProcessTerminationInfo)` and
  `RenderProcessHostDestroyed` ("guaranteed to be the last call made to the observer").
  **[source]** `GS content/public/browser/render_process_host_observer.h`
- Observers of other children get six callbacks: `BrowserChildProcessLaunchedAndConnected`
  (with a `base::Process` that "is guaranteed to be valid"),
  `BrowserChildProcessHostDisconnected`, `BrowserChildProcessCrashed`,
  `BrowserChildProcessKilled`, `BrowserChildProcessLaunchFailed` and
  `BrowserChildProcessExitedNormally`. **[source]**
  `GS content/public/browser/browser_child_process_observer.h`
- A consumer that starts late first walks the existing hosts, then subscribes. The Task
  Manager's child provider does exactly this: a loop over `BrowserChildProcessHostIterator`
  in `StartUpdating()`, then `BrowserChildProcessObserver::Add(this)`; all on the UI thread.
  **[source]** `GS chrome/browser/task_manager/providers/child_process_task_provider.cc`
- So Chromium tracks what it launched; it does not watch OS process events. **[source]**
  (the files above) / **[inferred]** (no OS-event watcher was found in these paths).

### How it learns that a child ended

- The primary signal is the IPC pipe. `ChildProcessHostImpl` sets a Mojo disconnect handler
  `OnDisconnectedFromChildProcess`; when a legacy IPC channel exists, the code says "we will
  use its connection status to monitor child process lifetime". **[source]**
  `GS content/browser/child_process_host_impl.cc`
- After the disconnect, the launcher reads the exit through
  `GetChildTerminationInfo(bool known_dead)`. On Linux "With |known_dead| set to false, a
  dead process could be seen as running" and "With |known_dead| set to true, the process
  will be killed if it was still running." **[source]** `child_process_launcher.h`
- On Windows, the info is `base::GetTerminationStatus(process.Handle(), &exit_code)`.
  **[source]** `child_process_launcher_helper_win.cc`
- On Linux, `base::GetKnownDeadTerminationStatus` sends SIGKILL before it waits, "to avoid
  the risk of hanging"; `ForceNormalProcessTerminationSync` terminates and then reaps the
  child ("On POSIX, we must additionally reap the child"), through the zygote when there is
  one. **[source]** `GS content/browser/child_process_launcher_helper_linux.cc`,
  `GS base/process/kill.h`
- `ChildProcessTerminationInfo` holds `status` (`base::TerminationStatus`) and `exit_code`
  on all platforms, `cpu_usage` (cumulative CPU) on non-Android, `last_error` on Windows,
  and many Android-only fields (binding state, "was killed intentionally by browser",
  spare-renderer data). **[source]** `GS content/public/browser/child_process_termination_info.h`

### PID reuse

- The launcher holds a `base::Process` for the whole life of the child. On Windows an open
  process handle keeps the process object, so its PID cannot be reused while the handle is
  open; on POSIX an unreaped child stays a zombie and keeps its PID until it is reaped.
  **[inferred]** (I did not read `base/process/process.h` to confirm the handle type.)
- The Performance Manager stores the PID but warns not to use it as an identity (finding 2).
  **[source]** `process_node.h`
- One weak point: the Task Manager's `Task::Kill()` does not use the held handle. It opens a
  new one by PID: `base::Process::Open(process_id())`, then `Terminate(RESULT_CODE_KILLED,
  false)`. It guards only against PID 0 ("This can result in calling "kill 0" which kills
  all processes in the process group") and against its own PID. **[source]**
  `GS chrome/browser/task_manager/providers/task.cc`. So "End process" relies on the task
  being removed when the child dies, before the PID is reused. **[inferred]**

### Windows job objects of the sandbox

- Each sandbox job level sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (the levels fall through
  to `kUnprotected`); `kLimitedUser` sets `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` with a limit of
  1; a memory limit (`JOB_OBJECT_LIMIT_PROCESS_MEMORY`) is set when nonzero. **[source]**
  `GS sandbox/win/src/job.cc`
- The sandbox doc says the job "could" limit CPU, memory and I/O but "currently does not",
  and that the broker "should always outlive all the target processes that it spawned".
  Policies of live processes are visible at `chrome://sandbox`. **[docs]**
  `GS docs/design/sandbox.md`
- The job object is a containment and cleanup tool here, not the way Chromium learns about
  exits (that is the IPC disconnect plus the handle). **[inferred]**

## 2. The Task Manager

### Model

- `TaskManagerInterface` is the model the window and other consumers observe. Methods
  include `GetTaskIdsList()` ("Tasks that share the same process id will always be
  consecutive"), `GetProcessHandle` / `GetProcessId`, `KillTask(TaskId)` ("Returns true if
  the process terminates") and `IsTaskKillable`. **[source]**
  `GS chrome/browser/task_manager/task_manager_interface.h`
- A `Task` has a `Type`: `UNKNOWN, BROWSER, GPU, ARC, CROSTINI, ZYGOTE, UTILITY, RENDERER,
  EXTENSION, GUEST, SANDBOX_HELPER, DEDICATED_WORKER, SHARED_WORKER, SERVICE_WORKER`, a
  `GetChildProcessUniqueID()` (the host id, not the PID), a process handle and PID,
  network counters, `GetParentTask`, `GetTabId`, and `Kill`/`IsKillable`. **[source]**
  `GS chrome/browser/task_manager/providers/task.h`
- Providers that create tasks: `BrowserProcessTaskProvider`, `ChildProcessTaskProvider`,
  and a `FallbackTaskProvider` over `SpareRenderProcessHostTaskProvider`,
  `WorkerTaskProvider` and `WebContentsTaskProvider`, with `RenderProcessHostTaskProvider`
  as fallback; on ChromeOS also `ArcProcessTaskProvider` and `VmProcessTaskProvider`.
  **[source]** `GS chrome/browser/task_manager/sampling/task_manager_impl.cc`
- Task to process: `TaskAdded()` looks up a `TaskGroup` by PID in `task_groups_by_proc_id_`
  and creates one if missing; VM tasks go into `arc_vm_task_groups_by_proc_id_`;
  `task_groups_by_task_id_` maps task to group; an empty group is erased. **[source]**
  same file. So the unit of measurement is the process (the group), and tasks are labels on
  it. **[inferred]**

### Sampling

- Each `TaskGroup` owns a `TaskGroupSampler` that runs on a sequenced thread-pool runner
  (`MayBlock`, `BEST_EFFORT`, `SKIP_ON_SHUTDOWN`) and reports back by callbacks to the UI
  thread. It refreshes CPU, idle wakeups, open FD count (Linux, ChromeOS, macOS), process
  priority and swapped memory. "The first reading is treated as invalid" for CPU.
  **[source]** `GS chrome/browser/task_manager/sampling/task_group_sampler.h`,
  `task_manager_impl.cc`
- Memory footprint comes from a global request,
  `memory_instrumentation::MemoryInstrumentation::RequestPrivateMemoryFootprint`, not from
  each sampler. **[source]** `task_manager_impl.cc`
- `base::ProcessMetrics` is the per-process reader: `GetPlatformIndependentCPUUsage` ("will
  return zero on the first call"), `GetCumulativeCPUUsage`, `GetIdleWakeupsPerSecond`,
  `GetMemoryInfo` (`resident_set_bytes`, plus `private_bytes` on Windows),
  `GetOpenFdCount` on POSIX. On macOS it needs a port provider for other processes.
  **[source]** `GS base/process/process_metrics.h`
- The metric flags are `REFRESH_TYPE_CPU, SWAPPED_MEM, GPU_MEMORY, V8_MEMORY,
  SQLITE_MEMORY, WEBCACHE_STATS, NETWORK_USAGE, IDLE_WAKEUPS, HANDLES, START_TIME, CPU_TIME,
  PRIORITY, FD_COUNT, KEEPALIVE_COUNT, MEMORY_FOOTPRINT, HARD_FAULTS, CPPGC_MEMORY`.
  Observers get `OnTaskAdded`, `OnTaskToBeRemoved`, `OnTasksRefreshed`,
  `OnTasksRefreshedWithBackgroundCalculations` and `OnTaskUnresponsive`. **[source]**
  `GS chrome/browser/task_manager/task_manager_observer.h`

### Does it gather data only while the window is open?

- Yes, more precisely: only while at least one observer exists. `AddObserver` calls
  `StartUpdating()` for the first observer ("This is the first observer to be added. Start
  updating"); `RemoveObserver` for the last one sets the flags to 0, stops the timer and
  calls `StopUpdating()`. **[source]** `GS chrome/browser/task_manager/task_manager_interface.cc`
- `StopUpdating` calls `ClearObserver()` on every provider and clears all task groups, so
  the task list itself is rebuilt from the hosts at the next start. **[source]**
  `task_manager_impl.cc`
- The window's model asks for `kRefreshTime = base::Seconds(2)` with `REFRESH_TYPE_NONE`,
  then adds a flag when a column becomes visible and removes it when the column is hidden
  (`UpdateRefreshTypes`). **[source]**
  `GS chrome/browser/ui/task_manager/task_manager_table_model.cc`
- The window is not the only possible observer: the `chrome.processes` extension API (Dev
  channel only) is built on the same data, and warns that `onUpdatedWithMemory` "incurs extra
  CPU usage and should only be listened for when needed". **[docs]**
  <https://developer.chrome.com/docs/extensions/reference/api/processes>

### End process

- `TaskManagerTableModel::KillTask(row)` calls `observed_task_manager()->KillTask(task)`,
  which finds the group and calls `Task::Kill()`, which opens the process by PID and
  terminates it with `content::RESULT_CODE_KILLED`. **[source]** the files above.
- The kill ends the whole process, so every task in that process ends with it.
  **[inferred]** (follows from the task-to-process grouping).
- Edge has the same window ("Browser task manager", under More tools). **[reported]**
  <https://www.makeuseof.com/how-to-sleep-tabs-in-microsoft-edge/> (search result; I did not
  find a Microsoft Learn page for it).

## 3. The Performance Manager

- Purpose: "data-driven, centralized resource management, prioritization and planning for
  the Chrome browser", meant to replace scattered heuristics. **[docs]**
  `GS components/performance_manager/README.md`
- Parts: the embedder notifies a registry when content objects are created or change; the
  graph is "a coarse, abstracted view"; decorators add measured data (CPU, memory);
  aggregators combine node data (for example frame memory into a page total); policies
  decide; mechanisms act, maybe in another process; content proxies give safe access back to
  the real objects and return null if the object was deleted. **[docs]** README.
- Nodes: Page (a `WebContents`), Frame, Process (a renderer or another process such as the
  browser or GPU), Worker, plus a System node. **[docs]** README;
  **[source]** `GS components/performance_manager/public/graph/system_node.h`
- `ProcessNode` exposes `GetProcessType`, `GetProcessId` (null before start; kept after
  exit), `GetProcess` (invalid before start and after exit), `GetLaunchTime`,
  `GetExitStatus` (empty until exit), `GetPrivateFootprint`, `GetResidentSet`,
  `GetPrivateSwap`, `GetPriority`, `GetFrameNodes`, `GetWorkerNodes`,
  `GetHostedContentTypes`, and proxies to the content host. Observer callbacks include
  `OnProcessNodeAdded`, `OnProcessLifetimeChange`, `OnProcessNodeRemoved`,
  `OnPriorityChanged`, and `OnAllFramesInProcessFrozen`. **[source]** `process_node.h`
- `PageNode` exposes the signals the policies read: visible (with last change time),
  audible, picture-in-picture, loading state, lifecycle state, holding a Web Lock or a
  blocking IndexedDB lock, uses WebRTC, notification permission, form interaction, user
  edits, and memory estimates. Its observer has one callback per signal plus
  `OnAboutToBeDiscarded`. **[source]**
  `GS components/performance_manager/public/graph/page_node.h`
- `SystemNodeObserver` has one callback, `OnProcessMemoryMetricsAvailable`. **[source]**
  `system_node.h`
- Thread: "the graph lives on the main thread", with all observers and mutators. **[docs]**
  README. Older headers describe a dedicated PM sequence, and a newer header marks the
  sequence access "DEPRECATED: Use objects living in the graph directly on the UI thread".
  **[reported]** (search summary of older revisions of
  `GS components/performance_manager/public/performance_manager.h`; I did not find the
  commit that moved it).
- Policies in `chrome/browser/performance_manager/policies/` include
  `discard_eligibility_policy`, `page_discarding_helper`, `urgent_page_discarding_policy`,
  `memory_saver_mode_policy`, `working_set_trimmer_policy`, `termination_target_policy`,
  `background_tab_loading_policy`, `frame_throttling_policy`,
  `sustained_memory_pressure_evaluator`, and platform ones (`oom_score_policy_chromeos`,
  `process_rank_policy_android`). **[source]** directory listing of
  `GS chrome/browser/performance_manager/policies/`
- The graph can be inspected at `chrome://discards` (Graph tab). **[docs]** README.

## 4. Freeze and discard decisions

### Who decides

- The browser process decides; the page only hears about it. The Page Lifecycle article
  calls frozen and discarded "system-initiated rather than user-initiated" and says browsers
  freeze and discard hidden tabs "at their discretion". **[docs]**
  <https://developer.chrome.com/docs/web-platform/page-lifecycle-api>
- The policies run in the browser process on the PM graph (section 3). **[source]**
- A page can only opt out through an origin trial (`BackgroundPageFreezeOptOut`) for
  freezing; for discarding, an extension can mark a tab not auto-discardable and an
  enterprise policy can opt a URL out. **[docs]**
  <https://developer.chrome.com/blog/freezing-on-energy-saver>; **[source]**
  `discard_eligibility_policy.cc` (`kExtensionProtected`, `kOptedOut`).

### Freezing

- `FreezingPolicy` "Freezes sets of connected pages when no page in the set is opted-out".
  Types: `kVoting` (embedders add freeze votes; a set freezes when all its pages have a vote
  and none has a `CannotFreezeReason`), `kBatterySaver` (CPU-intensive background tabs when
  Battery/Energy Saver is on), and `kInfiniteTabs` (tabs outside the most-recently-used set,
  with periodic unfreezing). CPU is measured per browsing instance through a resource
  attribution query ("1.0 = 100% of 1 core"). **[source]**
  `GS components/performance_manager/freezing/freezing_policy.h`
- Energy Saver freeze (Chrome 133+): a browsing context group whose pages are all "hidden and
  silent for more than five minutes" and in which a same-origin subgroup is
  "CPU-intensive". Exempt: audio or video conferencing (microphone, camera, screen
  capture), Web USB / Bluetooth / HID / Serial, and a Web Lock or IndexedDB connection that
  blocks others. A frozen tab is "automatically unfrozen" when it comes back, "without loss
  of state". **[docs]** <https://developer.chrome.com/blog/freezing-on-energy-saver>
- The threshold was planned to freeze about 10% of background tabs with Energy Saver on.
  **[reported]** blink-dev thread, via search:
  <https://groups.google.com/a/chromium.org/g/blink-dev/c/OVoSqEhtpNI>
- All `CannotFreezeReason` values: visible, recently visible, audible, recently audible,
  origin trial opt-out, holding Web Lock, holding IndexedDB lock, holding blocking IndexedDB
  lock, USB, Bluetooth, HID, serial, capturing video, capturing audio, being mirrored,
  capturing window, capturing display, active WebRTC connection, loading, notification
  permission, opted out, most recently used, WebUI. **[source]** `cannot_freeze_reason.cc`
- After a freeze: the trimmer policy empties the working set when all frames of a process
  are frozen (ChromeOS only today), and pages whose footprint grew after the freeze are
  discarded (`DiscardFrozenPagesWithGrowingMemoryOnMemoryMeasurement`). **[source]**
  `working_set_trimmer_policy.h/.cc`, `freezing_policy.h`

### Discarding

- `DiscardEligibilityPolicy::CanDiscard` returns `kEligible`, `kProtected` or `kDisallowed`.
  `kDisallowed`: not a tab, a discard was already attempted, no main frame, already
  discarded. `kProtected` reasons: visible, recently visible, audible, recently audible,
  picture-in-picture, PDF, invalid URL, web app window, not web or internal scheme, opted out
  by policy, notifications (for proactive discards), extension protected, capturing video /
  audio / window / display, being mirrored, Bluetooth, USB, active tab, pinned tab, DevTools
  open, background activity (title or favicon changes, for proactive discards), form
  interactions, user edits. **[source]**
  `GS chrome/browser/performance_manager/policies/discard_eligibility_policy.cc`
- Discard reasons: `URGENT`, `PROACTIVE`, `SUGGESTED`, `EXTERNAL`,
  `FROZEN_WITH_GROWING_MEMORY`. `EXTERNAL` and `FROZEN_WITH_GROWING_MEMORY` skip the
  protections. **[source]** same file.
- `PageDiscardingHelper` picks candidates sorted by importance, marks each attempt so a
  failed page is not tried again, and uses an 80 MiB estimate for a page with no memory
  measurement yet. **[source]** `GS .../policies/page_discarding_helper.cc`
- Urgent discard: on memory pressure (or sustained pressure, every 5 s while it lasts), and
  on Windows when available commit falls below a percentage (`kDiscardOnCommitLimit`), it
  calls `DiscardAPage(URGENT)`; ChromeOS discards toward a reclaim target. **[source]**
  `GS .../policies/urgent_page_discarding_policy.cc`
- Memory Saver: when a tab becomes hidden, a one-shot timer starts; when it becomes visible,
  the timer is cancelled. Time before discard: 6 h (conservative), 4 h (medium, default),
  2 h (aggressive); discard reason `PROACTIVE`. **[source]**
  `GS .../policies/memory_saver_mode_policy.cc`, `memory_saver_mode_policy.h`
- Edge "sleeping tabs": "Sleeping a tab saves 83% of its memory on average"; since Edge 105
  it sleeps "high resource tabs when your device's memory is near its limit". **[docs]**
  <https://blogs.windows.com/msedgedev/2022/12/06/sleeping-tabs-edge-105-sleep-before-discarding/>
  Default sleep after two hours, configurable. **[reported]** Windows Central, via search.

### Where the state is kept, and how a tab comes back

- The lifecycle state is a property of `PageNode` (`GetLifecycleState`, a
  `mojom::LifecycleState`), and freezing keeps per-browsing-instance state
  (`BrowsingInstanceState`, votes, the MRU list). **[source]** `page_node.h`,
  `freezing_policy.h`. I did not confirm the enum values of `mojom::LifecycleState`.
- Frozen: "the browser suspends execution of freezable tasks"; the page gets `freeze`, then
  `resume` when it runs again. Discarded: the page is unloaded; when the user returns it
  loads again and `document.wasDiscarded` is true. **[docs]**
  <https://developer.chrome.com/docs/web-platform/page-lifecycle-api>
- So freeze keeps the process and its memory, and discard ends the page's content but keeps
  the tab (title, history) in the browser. **[inferred]** This is the same split as Ralphy's
  suspended (process kept, stopped) and hibernated (process ended, screen and session id
  kept). **[inferred]**

## 5. Process model and memory pressure

- Site isolation: "each process is locked to documents from a single site" on desktop.
  Chromium sets a "soft" process limit "based on the memory available"; above it, it
  "makes an attempt to start randomly reusing same-site processes". Extensions may use one
  third of the limit. A lock "remains constant for the lifetime of the RenderProcessHost".
  **[docs]** `GS docs/process_model_and_site_isolation.md`
- The exact limit formula: old code divided physical memory by 256 MB to pick a tier, with a
  maximum constant (once 42). **[reported]** old revisions found by search, for example
  `content/common/content_constants.cc` at revision `88d2338d3823` (prefix
  `https://chromium.googlesource.com/chromium/src/+/<revision>/`).
  I did not confirm the current formula or maximum.
- Spare renderer: "Chromium often creates a spare RenderProcessHost with a live but unlocked
  renderer process" so a navigation does not wait for a launch. The Task Manager shows it as
  its own task (`SpareRenderProcessHostTaskProvider`, subtype `kSpareRenderer`). **[docs]**
  process model doc; **[source]** `task.h`, `task_manager_impl.cc`
- Several tasks share one process: frames of one site, their dedicated workers, and
  extensions. The Task Manager lists tasks of one process together. **[docs]** /
  **[source]** as above.
- Private memory footprint (finding 8): Windows `private_bytes`; Linux/ChromeOS/Android/
  Fuchsia `(rss_anon_bytes + vm_swap_bytes)`; macOS `phys_footprint - shared_resident`; iOS
  not implemented. **[source]** `queued_request_dispatcher.cc`. The UMA names are
  `Memory.{Total,Renderer,Browser,Gpu,Extension}.PrivateMemoryFootprint`. **[docs]**
  `GS docs/memory/README.md`
- System memory sampling: `base::AvailableMemoryMonitor` polls `base::GetSystemMemoryInfo()`
  every 2 s (`kPollInterval`), and "polls only while observers exist". On Windows the commit
  values come from `ullAvailPageFile` (available commit) and the page-file total (commit
  limit). **[source]** `GS base/memory/available_memory_monitor.cc`
- macOS: event-driven, a `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` source plus the
  `kern.memorystatus_vm_pressure_level` sysctl; WARN maps to moderate, CRITICAL to critical.
  **[source]** `GS components/memory_pressure/system_memory_pressure_evaluator_mac.cc`
- Windows: no `system_memory_pressure_evaluator_win.cc` exists in `main` today (404). The
  directory has threshold, free-memory, compressor and available-memory evaluators and a
  voter. **[source]** listing of `GS components/memory_pressure/`. Urgent discarding on
  Windows reacts to available commit. **[source]** `urgent_page_discarding_policy.cc`.
  Which evaluator Linux desktop uses today was not confirmed.
- What Chromium reacts to: memory pressure level (moderate, critical), sustained pressure,
  low available commit on Windows, a ChromeOS reclaim target, CPU in background (Energy
  Saver freeze), hidden time (Memory Saver), and memory growth while frozen. **[source]**
  the policy files above.

## 6. Remote and other processes

- `SystemInfo.getProcessInfo` "Returns information about all running processes": each has
  `type`, `id` (PID) and `cpuTime` (cumulative CPU seconds). The domain is experimental.
  **[docs]** `CDP SystemInfo.pdl`
- The handler builds the list from `base::Process::Current()` ("browser"),
  `RenderProcessHost::AllHostsIterator()` ("renderer") and `BrowserChildProcessHostIterator`
  (the child's metrics name), with `ProcessMetrics::GetCumulativeCPUUsage`. It works only on
  the browser target. **[source]** `GS content/browser/devtools/protocol/system_info_handler.cc`
  So the remote list is again the launcher's own registry, not an OS scan. **[inferred]**
- `Performance.enable` / `getMetrics` / the `metrics` event give named run-time metrics for
  one target. **[docs]** `CDP Performance.pdl`
- `Memory` (experimental): DOM counters, `simulatePressureNotification`,
  `setPressureNotificationsSuppressed`, sampling profiles. **[docs]** `CDP Memory.pdl`
- `Tracing.requestMemoryDump` returns a global memory dump id, at level `background`,
  `light` or `detailed`. **[docs]** `CDP Tracing.pdl`
- What a remote viewer can read: the process list with PIDs and CPU time, page metrics, and
  memory dumps, over the DevTools WebSocket. It cannot end a process through a process
  command; the closest is closing a target. No Chromium feature joins the process state of
  several machines into one view. **[inferred]** from the domains above.
- The `chrome.processes` extension API (Dev channel) has `onCreated`, `onUpdated`,
  `onUpdatedWithMemory`, `onUnresponsive`, `onExited` (with `exitType` and `exitCode`),
  `terminate(processId)` and `getProcessIdForTab`. **[docs]**
  <https://developer.chrome.com/docs/extensions/reference/api/processes>

## What Ralphy can copy

Map (Chromium piece → Ralphy piece: note) **[inferred]**:

- Browser process → daemon: the only launcher of consoles; it owns the registry.
- `ChildProcessLauncher` + host → session manager + console: hold the root process
  handle for the console's whole life.
- Task → console (workbench session): the thing the user sees and names.
- Process / `TaskGroup` → the console's process tree: in Ralphy one console has many
  processes; in Chromium one process has many tasks (the reverse).
- Host unique id vs PID → console id vs PIDs: key on the console id; keep PID plus process
  start time as data.
- `ArcProcessTaskProvider` and the VM map → peer (WSL, ssh): a separate map per peer,
  because a PID is unique only inside one machine or VM.
- Task Manager window → workbench process view: sample only while it is open, and only
  the columns it shows.
- Performance Manager graph → the process-management module: one model, observers,
  policies, mechanisms.
- `CannotFreezeReason` / `CannotDiscardReason` → ADR-0083 blockers: a closed enum, one
  label each, shown in the UI.
- Freeze / discard → suspend / hibernate: the same two levels in the same order (freeze,
  trim, end).
- `chrome://discards` → a debug view of reasons: show why a console is not suspended.
- `ChildProcessTerminationInfo` → an exit record per process: status, exit code,
  cumulative CPU at exit.

Things to copy, in order of value **[inferred]**:

1. One owner of process facts in the daemon, with a registry keyed by a Ralphy id, and a
   PID that is only data. Hold the root process handle (Windows) or keep the child unreaped
   until its status is read (POSIX), as Chromium's launcher does.
2. Observer-driven sampling: the first viewer starts the sampler, the last stops it, the
   interval is the minimum asked, and metrics are the union of what is asked. Note that
   ADR-0083 D4 needs a once-a-minute pass even with no viewer; that is a separate, cheap
   consumer (like Chromium's policies, which run without the Task Manager window).
3. Two separate consumers of one model: policies (suspend/hibernate) and the view
   (workbench). Chromium has two models (Task Manager and PM); Ralphy can avoid that split.
4. Named blocker reasons, exposed to the workbench.
5. Per-OS memory formulas, so the numbers match what Chromium and the OS tools show.
6. A kill that ends the whole unit: "End process" in Ralphy should end the console's tree,
   and the UI should say that every task in it ends.

What does not transfer **[inferred]**:

- **Grandchildren.** Chromium launches every child and forbids renderers from creating
  children (job limit of 1). An agent CLI spawns shells, tools and MCP servers that Ralphy
  never launches. Ralphy needs discovery of descendants (process table scan, job object or
  cgroup membership) that Chromium does not have. The other research note covers the OS
  side.
- **IPC as the life signal.** Chromium learns of a death from a broken Mojo pipe. Ralphy's
  link to a console is a PTY; a grandchild has no link at all.
- **Freeze inside the runtime.** Chromium freezes a page by stopping its task queues inside
  Blink, with `freeze`/`resume` events. Ralphy can only stop processes from outside; the
  agent gets no event. This is why ADR-0083 needs a process state-change object or SIGSTOP.
- **Discard keeps the tab.** Chromium reloads a discarded page from its URL. Ralphy must keep
  the screen and the agent session id and relaunch with a resume flag; not every CLI supports
  that (ADR-0083 table).
- **PID-based kill in Task Manager.** Do not copy `Task::Kill()`'s open-by-PID: use the held
  handle, or check the start time, before a signal.
- **Peers.** Chromium has no peer daemons. The ChromeOS VM providers and DevTools remote
  targets are the nearest examples, and neither lets one browser act on another machine's
  processes.

## Gaps

- I did not read `base/process/process.h`, so the claim that a held handle blocks PID reuse
  on Windows is inferred, not quoted.
- Current renderer process limit formula and maximum: not confirmed.
- Which memory pressure evaluator Linux desktop uses today: not confirmed.
- The values of `mojom::LifecycleState` and the time constants `kTabAudioProtectionTime` and
  `kNonVisiblePagesUrgentProtectionTime`: not read.
- The commit that moved the Performance Manager from its own sequence to the UI thread: not
  found.
- How the Windows working-set trimmer mechanism was used in the past (today only ChromeOS
  builds the trimmer policy): not researched.
- No Microsoft Learn page for the Edge Browser task manager was found; Edge facts come from
  the Edge blog and third parties.
- The DevTools `Target` and `Browser` domains (for example closing a target, histograms) were
  not read in detail.
