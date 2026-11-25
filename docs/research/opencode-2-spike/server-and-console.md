# OpenCode 2: the background service, `--standalone`, kill and the console

Spike of #1010, questions Q5 and Q8. Plan stage 5. Markers are in the
[README](README.md). Raw output is in `docs/spike/live/opencode-1010-server-*.log`:
one log per probe, with the process tree before, during and after the run, by
PID and parent PID.

Version probed: OpenCode 2.3.2 (the operator's install), on Windows 11 and in WSL
Ubuntu-22.04. macOS is not probed. OpenCode 1.x has no background service; its
side is in [stream-and-outcomes.md](stream-and-outcomes.md) and
[install-and-command-line.md](install-and-command-line.md).

## How the probes were made

A Python script started each run the way the adapter does: the argv of
`command.rs` with the 2.x changes of the command-line table (`opencode run
--format json --dangerously-skip-permissions --print-logs --log-level error -m
<model>`), the prompt on stdin, the process folder set to a scratch clone of
FinCal, `PWD` set to the same folder, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY`
removed, and `OPENCODE_CONFIG_CONTENT` set. On Windows the program was
`%APPDATA%\npm\opencode.cmd`; in WSL it was `~/.opencode/bin/opencode`.

Two modes:

- default: no server flag. The run is a client of the operator's background
  service (`opencode serve --service`, Windows PID 44232, WSL PID 2216799) and
  uses the operator's state.
- `--standalone`, with the scratch `xdg-v2` tree.

The script took a process table before the run, during it (when the shell tool
of the run was seen), and 5 seconds after it. The kill was the one of
`ralphy_proc_util::kill_tree_by_pid`: on Windows every process in the tree of
the run's PID by parent PID, root first; in WSL `SIGKILL` to the process group
that the run leads (the run was started with `setpgrp`, as `own_process_group`
does).

Three prompts: "run `sleep N`, then create `AFTER.md` and commit it"; "create
`PA.md` (or `PB.md`) and commit it", for the parallel runs; "create `CWD.md`",
for the folder probe.

The operator's processes at the start: Windows `opencode.exe serve --service`
PID 44232 and its `opencode-pty.exe daemon` PID 4484; WSL PID 2216799 and its
`opencode-pty daemon` PID 3980080. They were never touched and were alive at the
end.

## Q5: the background service and the processes of a run

### What a run starts, and what stays

| Mode           | Host    | Processes of the run                                                                                                        | Where the shell tool runs                                                          | After the exit | Log                               |
| -------------- | ------- | --------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- | -------------- | --------------------------------- |
| default        | Windows | `cmd.exe /c opencode.cmd run ...`, then its child `opencode.exe run ...`                                                    | `pwsh.exe -Command "sleep 15"`, a child of the service's `opencode-pty` (PID 4484) | nothing new    | `server-win-after-default.log`    |
| default        | WSL     | `opencode run ...`                                                                                                          | `sleep 15`, a child of the service's `opencode-pty` (PID 3980080)                  | nothing new    | `server-wsl-after-default.log`    |
| `--standalone` | Windows | `cmd.exe`, `opencode.exe run ...`, its child `opencode.exe serve --stdio --port 0`, and its child `opencode-pty.exe daemon` | `bash.exe -c "sleep 15"` and `sleep.exe`, children of that `opencode-pty`          | nothing        | `server-win-after-standalone.log` |
| `--standalone` | WSL     | `opencode run ...`, its child `opencode serve --stdio --port 0`, and its child `opencode-pty daemon`                        | `sleep 15`, a child of that `opencode-pty`                                         | nothing        | `server-wsl-after-standalone.log` |

Every run exited 0 and committed `AFTER.md` in its own clone 🔬.

- In the default mode the run starts no server. The work is done in the
  service: the shell of a tool is a child of the service's `opencode-pty`
  daemon, not of the run 🔬. The `opencode-pty` daemon of the service was
  started by the first service run of the stream stage, and it stays with the
  service.
- With `--standalone` the run starts its own server as a child, on stdio and
  with no TCP port (`--port 0`), and that server starts its own `opencode-pty`
  daemon under the scratch state folder 🔬. All of them end with the run.
- The service keeps each folder it served. It opens file watchers on the folder,
  on its `AGENTS.md`, `CLAUDE.md` and `.git/HEAD`, and closes them about an hour
  later: `repos\w2s` of the stream stage was booted at 01:46:12 and its watchers
  stopped at 02:46:48 🔬 (`server-service-log-win.log`). The watchers do not
  lock the folder: at the end of this stage the scratch root was removed on
  Windows while the service still watched its clones 🔬.
- The shell of the tool differs: `pwsh.exe` under the service, Git Bash under a
  standalone server with the scratch state 🔬. The cause was not looked for.

**Q5 after a run:** in the default mode a 2.x run leaves no process of its own
(the service and its `opencode-pty` daemon, Windows PIDs 44232 and 4484, WSL
2216799 and 3980080, were there before and stay), but the service keeps watchers
on the folder for about an hour; with `--standalone` the run starts
`opencode serve --stdio --port 0` and an `opencode-pty` daemon, and nothing
stays after the exit (🔬 Windows and WSL).

### Two runs at once

Two runs were started at the same moment in two clones, each told to create and
commit a file with its own name.

| Mode           | Host    | Session ids                                     | Files                                | Other repo in the stream | Log                             |
| -------------- | ------- | ----------------------------------------------- | ------------------------------------ | ------------------------ | ------------------------------- |
| default        | Windows | `ses_def6703abffe...` and `ses_def6703a1ffe...` | `PA.md` only in A, `PB.md` only in B | no                       | `server-win-par-default.log`    |
| default        | WSL     | `ses_def62a98bffe...` and `ses_def62a98cffe...` | `PA.md` only in A, `PB.md` only in B | no                       | `server-wsl-par-default.log`    |
| `--standalone` | Windows | `ses_def65592cffe...` and `ses_def65592bffe...` | `PA.md` only in A, `PB.md` only in B | no                       | `server-win-par-standalone.log` |
| `--standalone` | WSL     | `ses_def6261f3ffe...` and `ses_def6261eaffe...` | `PA.md` only in A, `PB.md` only in B | no                       | `server-wsl-par-standalone.log` |

- Every run exited 0 and made its commit in its own clone 🔬.
- Through the service, both runs are clients of the same service process; with
  `--standalone`, each run has its own server and its own `opencode-pty`, and
  the two servers used one scratch database at the same time with no error 🔬.
- The two standalone servers have no port to share (`--port 0`, stdio), so the
  fixed port 28894 of the service is not a limit for parallel standalone runs 🔬.
- The session ids differ, and every event of a stream carries only its own id
  (`sessionID` on each event, as in the stream stage).

**Q5 parallel runs:** two 2.x runs at once in two repos stay apart in both modes
and on both hosts: each wrote and committed only in its own repo, the session
ids differ, and no stream names the other repo (🔬 Windows and WSL).

### Kill of the run's tree mid-run

The kill came while the `sleep 40` of the first tool ran. For 60 seconds after
it, the script looked at the clone every 10 seconds and took the process table.

| Mode                    | Host    | Killed                                                                                         | Within 60 s after the kill                                                                                                                 | Log                                     |
| ----------------------- | ------- | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------- |
| default                 | Windows | `cmd.exe` and `opencode.exe run` (PIDs 31224, 31812)                                           | the `pwsh.exe` sleep under the service ran on; `AFTER.md` was written and committed, seen at the check 42 s after the kill                 | `server-win-kill-default.log`           |
| default                 | WSL     | the process group of `opencode run` (PID 4023060)                                              | the `sleep 40` under the service ran on; `AFTER.md` was written and committed, seen at the check 52 s after the kill                       | `server-wsl-kill-default.log`           |
| default, then interrupt | Windows | `cmd.exe` and `opencode.exe run` (PIDs 46900, 47296)                                           | `opencode api POST /api/session/<id>/interrupt` answered `{"interrupted":true}`; the sleep was gone 10 s later; nothing written, no commit | `server-win-kill-interrupt-default.log` |
| `--standalone`          | Windows | 7 processes: `cmd.exe`, `run`, `serve`, `opencode-pty`, `bash.exe`, `sleep.exe`, `conhost.exe` | no process left; nothing written, no commit                                                                                                | `server-win-kill-standalone.log`        |
| `--standalone`          | WSL     | the process group of `opencode run` (PID 4023674)                                              | no process left (`serve` and `opencode-pty` were in the group); nothing written, no commit                                                 | `server-wsl-kill-standalone.log`        |

- When the client dies, the service answers its open
  `POST /api/experimental/session/<id>/wait` with status 499 and goes on with
  the session 🔬 (`server-service-log-win.log`, `server-service-log-wsl.log`).
  Nothing in the service log says that the session stopped.
- The 2.3.2 binary has a `POST /api/session/:sessionID/interrupt` route
  (`session.interrupt`, the `escape` key of the TUI) 🔎, and `opencode api`
  calls it 🔬. With it, the kill stops the work.
- A killed run of the default mode wrote no `step_finish` before the kill, so
  the stream had no token count; the service went on spending tokens after it.

**Q5 kill:** a tree kill of a default-mode 2.x run ends only the client: the
service goes on with the session, its tool processes run on, and the run's
change is written and committed within 42 s (Windows) and 52 s (WSL) of the kill;
`opencode api POST /api/session/<id>/interrupt` after the kill stops it; a tree
kill of a `--standalone` run ends the server and its tools, and nothing more is
written (🔬 Windows and WSL).

### The working folder (#278)

The 1.x problem of #278 was a run that worked in the folder of Ralphy's own
process and not in the repo. The probe: process folder `repos/w5c` (WSL
`repos/l5c`), `PWD` set to another clone `repos/a`, through the service.

- On both hosts the session opened in `repos/a`: `CWD.md` was written there and
  not in the process folder 🔬 (`server-win-cwd-default.log`,
  `server-wsl-cwd-default.log`). The service log says
  `location services booted directory=...\repos\a` for that run 🔬.
- Every other run of this stage set `PWD` to the process folder and wrote in its
  own clone 🔬. The stream stage saw the same with `--standalone`
  (`stream-pwd-win.log`), and saw 1.19.0 keep the process folder.
- So the client sends `PWD` to the service as the folder of the session. Ralphy
  sets only the process folder (`current_dir(root)` in `command.rs`); no code
  in the adapter, `ralphy-adapter-support` or the daemon's session spec sets
  `PWD`. A run started from another folder, such as a run the daemon starts,
  passes the `PWD` of its parent, and a 2.x run then works in that folder. This
  is #278 again, with a new cause, in both modes.
- On Windows the cause needs a `PWD` in the environment, which Git Bash sets
  and a plain Windows process does not. In WSL every shell sets it.

**Q5 answer:** a default-mode 2.x run is a thin client: the work, its tools and
its file watchers live in the shared service, so a tree kill does not stop the
work (an interrupt call does), while a `--standalone` run owns its server and
its tools and leaves nothing after an exit or a kill. Parallel runs stay apart
in both modes, and in both modes a 2.x run works in the folder that `PWD` names,
not in the process folder.

**Changes:**

- #1009: run every 2.x plan and execute with `--standalone`. Reasons: the tree
  kill of the timeout and of the daemon stops the work only then; the server
  log reaches stderr only then (stream stage); the service does not keep the
  folder watched for an hour. Parallel runs work in this mode. Not probed:
  `--standalone` with the operator's own state while the service runs on the
  same database; this stage used a scratch state. Probe it in #1009 before the
  default changes.
- #1009: when a 2.x run goes through a server that Ralphy did not start (the
  service, or a `--server` URL), stopping the run must also call
  `opencode api POST /api/session/<id>/interrupt` with the session id from the
  stream. A tree kill alone leaves the work running.
- New issue under #1011: set `PWD` to the run folder in the `OpenCode` command
  (`command.rs`, both versions; 1.x ignores it) and in the daemon's `OpenCode`
  console spec, with a test that the child gets `PWD` equal to its folder. It
  is the cause of the wrong-folder run on 2.x, through the service and with
  `--standalone`.

## Q8: the TUI

The TUI is `opencode` with no subcommand, with the operator's state.

- Windows (`server-win-tui.log`): started with `Start-Process` in a console
  window of its own, in `repos\w5t`. During: `cmd.exe /c opencode.cmd`, its
  child `opencode.exe` (no arguments) and a `conhost.exe`; no server process
  🔬. The service log shows the TUI (`role=cli`, "cli starting"), the service
  booting the folder `repos\w5t`, and an `/api/event` stream that closed when
  the TUI ended 🔬. After 20 s the tree (3 PIDs) was ended by PID; 5 s later only
  the service and its `opencode-pty` were left 🔬.
- WSL (`server-wsl-tui.log`): `timeout 20 script -qfc opencode /dev/null` in
  `repos/l5t`. During: one `opencode` process; no server process 🔬. The
  service log shows the same client lines and the folder `repos/l5t` 🔬. The
  exit was 124 (the timeout); 5 s later only the service and its
  `opencode-pty` were left 🔬. The TUI wrote 15 943 bytes to the terminal; the
  screen text shows the folder, the branch, the version and an "update
  available" notice for 2.4.1 🔬.
- With `PWD` set to another clone (`server-wsl-tui-pwd.log`), the footer of the
  TUI still showed the process folder `repos/l5t` 🔬; its header was cut, so the
  probe does not show which folder the session would use. The daemon's console
  sets no `PWD` either.
- When the service is not running, a 2.x client starts it (the install stage
  saw a client try to start a managed service on port 28894). For the TUI this
  was not probed: the operator's service was not stopped.

**Q8 answer:** the 2.x TUI joins the shared background service and starts no
server of its own; when its tree is ended by PID, nothing of it stays, and the
service with its `opencode-pty` daemon, there before, goes on (🔬 Windows and
WSL). Whether the TUI draws and takes input inside a Ralphy console is the
manual check of the operator.

**Changes:**

- #1008: the console needs no new flag to start the 2.x TUI: the locator finds
  it and it joins the service.
- New issue under #1011: closing an `OpenCode` console ends the TUI but not a
  prompt that it sent: the work runs in the service, as in the kill probe. The
  ADR decides whether the console passes `--standalone`, or interrupts the
  session when it closes, or keeps the shared service on purpose (other clients
  of the service see the same sessions).
- The `PWD` issue above covers the console spec too.

## Other observations

- A real 2.x free-model limit 🔬 (`server-win-freelimit-par-standalone.log`):
  partway through this stage, every run on `opencode/deepseek-v4.1-flash-free`
  ended with one `error` event `{"type":"provider.free-limit","message":"Free
  usage limit reached. ...","status":429,"retryAfterMs":77890000}` and exit 1,
  on both hosts. The stream stage had this type only from the binary 🔎.
  `opencode/mimo-v2.6-flash-free` gave the same error and
  `opencode/hy4-flash-free` still answered, so the limit covers some free models
  and not all; the rest of this stage used `opencode/hy4-flash-free`. #1009: this is a usage limit, and
  `retryAfterMs` (here 21.6 hours) is the time to wait.

### Live-probe cost

| Probe                                                       | Host         | Version | Model                               | Tokens                                                                                                 |
| ----------------------------------------------------------- | ------------ | ------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------ |
| after a run, default and `--standalone`                     | Windows      | 2.3.2   | `opencode/deepseek-v4.1-flash-free` | about 64 100 and 80 600 (cost 0)                                                                       |
| after a run, default                                        | WSL          | 2.3.2   | `opencode/deepseek-v4.1-flash-free` | about 74 500 (cost 0)                                                                                  |
| after a run, `--standalone`                                 | WSL          | 2.3.2   | `opencode/hy4-flash-free`           | about 72 800 (cost 0)                                                                                  |
| two runs at once, default                                   | Windows      | 2.3.2   | `opencode/deepseek-v4.1-flash-free` | about 47 900 and 64 100 (cost 0)                                                                       |
| two runs at once, `--standalone`, stopped at the free limit | Windows      | 2.3.2   | `opencode/deepseek-v4.1-flash-free` | about 31 900 and 15 900 (cost 0)                                                                       |
| two runs at once, `--standalone`                            | Windows      | 2.3.2   | `opencode/hy4-flash-free`           | about 63 000 and 62 900 (cost 0)                                                                       |
| two runs at once, default and `--standalone`                | WSL          | 2.3.2   | `opencode/hy4-flash-free`           | about 58 600, 58 200, 58 200 and 58 000 (cost 0)                                                       |
| kill, default and `--standalone`                            | Windows, WSL | 2.3.2   | both free models                    | not known for the default mode (the service went on); about 15 900 or less for `--standalone` (cost 0) |
| kill, then interrupt                                        | Windows      | 2.3.2   | `opencode/hy4-flash-free`           | not known, less than one step (cost 0)                                                                 |
| working folder (`PWD`)                                      | Windows, WSL | 2.3.2   | `opencode/hy4-flash-free`           | about 31 200 and 28 800 (cost 0)                                                                       |
| runs that hit the free limit at once                        | WSL          | 2.3.2   | `opencode/deepseek-v4.1-flash-free` | none (refused)                                                                                         |
| TUI (no prompt), service log, binary strings, help          | Windows, WSL | 2.3.2   | none                                | none                                                                                                   |

The token counts are the sums of `input`, `output`, `reasoning` and the cache
fields of the `step_finish` events of each stream.
