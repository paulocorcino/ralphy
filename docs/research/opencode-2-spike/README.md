# OpenCode 2 spike

Facts about OpenCode 2 that the ADR and the two issues #1008 and #1009 stand on.
This is the spike of #1010, part of #1011. Each file answers some questions from
the probes of the machine, with the raw output in `docs/spike/live/`. Nothing here
is an instruction for the code: the decision is in the ADR.

## Platform table

| Host             | Version                             | Binary                                                                                                                   | Marker |
| ---------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ------ |
| Windows 11       | 2.3.2 (operator)                    | `%APPDATA%\npm\node_modules\@opencode\cli\bin\opencode.exe`, started by `%APPDATA%\npm\opencode.cmd` and `opencode2.cmd` | 🔬     |
| Windows 11       | 1.19.0 (scratch)                    | `%TEMP%\ralphy-1010\v1\node_modules\opencode-windows-x64\bin\opencode.exe`                                               | 🔬     |
| Windows 11       | 1.14.24 (desktop app, on no `PATH`) | `%LOCALAPPDATA%\opencode\opencode-cli.exe`                                                                               | 🔬     |
| WSL Ubuntu-22.04 | 2.3.2 (operator)                    | `~/.opencode/bin/opencode`, and the `opencode2` shell script next to it                                                  | 🔬     |
| WSL Ubuntu-22.04 | 1.19.0 (scratch)                    | `/tmp/ralphy-1010/v1/node_modules/opencode-linux-x64/bin/opencode`                                                       | 🔬     |
| macOS            | not probed                          | `~/.opencode/bin/opencode` by the installer scripts                                                                      | 📖     |

The latest releases on the day of the probes: 2.4.1 (`@opencode/cli`) and 1.19.6
(`opencode-ai`). The operator's 2.3.2 was not updated.

## Evidence markers

Every fact has the tool, the version and the OS, and one marker:

- 🔬 observed: a command ran; the output is quoted or is in a raw log.
- 🔎 read from the binary, a script or a file on disk.
- 📖 documented only: a page of the vendor, not run. macOS is always this one.

## Scratch rules

- Scratch roots: `%TEMP%\ralphy-1010\` on Windows and `/tmp/ralphy-1010/` in WSL.
  No scratch file is written under `/mnt/c/` from WSL.
- OpenCode 1.x is a scratch npm install (`opencode-ai@1.19.0`). Every 1.x command
  runs with `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and
  `XDG_CACHE_HOME` under `<root>/xdg-v1/`, so 1.x never opens the 2.x database.
- OpenCode 2.x is the operator's install, called by absolute path. It is never
  updated, reinstalled or logged out. A probe of 2.x that is not about the
  operator's state uses `--standalone` and a scratch `<root>/xdg-v2/` tree.
- The database of the operator is read through a copy of its three files.
- Probes run in clones of the lab repo under `<root>/repos/`, never in the lab
  repo itself.
- Raw logs hold `<redacted>` in place of tokens, keys, account ids and e-mails,
  and `<home>` and `<user>` in place of the home folder and the user name.

## Spike files

- [Install, version and command line](install-and-command-line.md): Q1 and Q2.
- [State on disk and what Ralphy reads](state-and-reads.md): Q6 and Q7.
- [The event stream and the three outcome signals](stream-and-outcomes.md): Q3 and Q4.
- [The background service, `--standalone`, kill and the console](server-and-console.md):
  Q5 and Q8.
- [Live checks for #1009](live-checks-1009.md): D6d, D6e, D3c and the fixture databases.

The decision that stands on these facts is
[ADR-0094](../../adr/0094-opencode-1-and-2.md) (proposed).

## Summary

**Q1, install and locate.** OpenCode 2 installs to `~/.opencode/bin/opencode` with an
`opencode2` shim next to it (the 2.x script), or as npm `@opencode/cli` with `opencode`
and `opencode2` in the npm global folder. On both hosts of the operator `opencode` is
2.3.2. A non-interactive WSL shell does not have `~/.opencode/bin` on `PATH`, so the
locator needs that folder after `PATH`. 1.x prints `1.19.0` and 2.x prints
`opencode v2.3.2`, and the folder does not tell the two apart.

**Q2, the command line.** `run`, `--format json`, `-m`, `--print-logs`, the prompt on
stdin, `OPENCODE_CONFIG_CONTENT` and the process folder work on 2.3.2 as on 1.19.0, and
2.x still accepts the hidden `--dangerously-skip-permissions`. Two flags break on 2.x:
`--variant` (the variant goes after `#` in `-m`) and `--log-level ERROR` (2.x takes only
lowercase). With no server flag a 2.x run uses the shared background service.

**Q3, the event stream.** The 2.x stream keeps the 1.x envelope and the `step_start`,
`step_finish`, `text` and `tool_use` events with `sessionID` on each, so text, sentinels
and the session id read unchanged. The `error` event is typed (`error.type`, `status`;
no `name`, no `data`), the token object has no `total`, the shell tool is `shell`, and a
new `retry` event has an `error` key that today's parser counts as an error.

**Q4, the outcomes.** Completion is the same in both versions: exit 0 and a last
`step_finish` with `reason` `stop` after the sentinel. On 2.x an auth error is an `error`
event `provider.auth` (a missing login is `provider.no-route`), and a usage limit is an
`error` event of a `provider.*-limit` or `provider.quota` type, each with exit 1. The
1.x matchers miss both, and through the service stderr is empty.

**Q5, the server mode.** A default 2.x run is a client of the shared service: the work,
its tools and its file watchers live there, so a tree kill does not stop the work (the
interrupt route does). A `--standalone` run owns its server and its tools and leaves
nothing after an exit or a kill. Two runs at once stay apart in both modes. In both modes
2.x works in the folder that `PWD` names, not in the process folder.

**Q6, the state on disk.** 2.x keeps its state in the same XDG folders as 1.x, in one
SQLite database with the login in a `credential` table. A fresh 2.x database has no
`message` and no `session` table: the assistant rows are in `session_message` under
`session_v2`, so both token readers of Ralphy return 0 for a 2.x session. An upgraded
database holds both sets.

**Q7, agent state, skills and tasks.** `skills.paths`, the forge-write deny map and
`opencode models` work on 2.x. The state plugin does not: 2.x loads only plugin folders,
and its plugin API is a default export with an `id`, so no status line is written.

**Q8, the console.** The 2.x TUI joins the shared service and starts no server; ended by
PID, it leaves nothing of its own. A prompt it sent runs on in the service after it ends.
Whether it draws and takes input in a Ralphy console is the operator's manual check.

## Change list

Every `**Changes:**` item of the four files, in file order. "Done" names the commit on
the branch of #1011. Three new issues come out of the spike:

- **N1 — "Set `PWD` to the folder of every OpenCode run and console"**: `command.rs`
  and the daemon's `OpenCode` console spec, both versions, with a test that the child
  gets `PWD` equal to its folder (ADR-0094 D3b, D7c).
- **N2 — "Report the agent state of an OpenCode 2 run through a 2.x plugin"**: a plugin
  folder to the 2.x plugin API that writes the same status lines (ADR-0094 D5b).
- **N3 — "A token reader whose query fails says so, and does not report zero"**: the
  OpenCode readers first, then any other vendor store (ADR-0094 D4i).

| #  | From                       | Item                                                                                   | Goes to                                          |
| -- | -------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------------------ |
| 1  | install, Q1                | The locator order `PATH`, `~/.local/bin`, nvm, `~/.opencode/bin` is confirmed          | #1008, done (`8b32f21e`, `a66a3c30`)             |
| 2  | install, Q1                | The doctor text names the npm global folder, where Windows has 2.x                     | #1008                                            |
| 3  | install, Q1                | A version check reads `1.19.0` and `opencode v2.3.2`                                   | #1009 (ADR-0094 D2: the reader is in proc-util)  |
| 4  | install, Q1                | The adapter asks `--version`; it does not decide the version from the folder           | #1009                                            |
| 5  | install, Q2                | 2.x: `-m provider/model#variant`, never `--variant`                                    | #1009                                            |
| 6  | install, Q2                | 2.x: `--log-level error`; 1.x keeps `ERROR`, in one function with the version          | #1009                                            |
| 7  | install, Q2                | 2.x: `--auto`; 1.x keeps `--dangerously-skip-permissions`                              | #1009                                            |
| 8  | install, Q2                | 2.x: the limit detector accepts `level=INFO` lines on stderr                           | #1009                                            |
| 9  | install, Q2                | No folder flag in 2.x `run`; check which folder a service run uses                     | answered in Q5 (`PWD`); the work is N1           |
| 10 | state, Q6                  | `usage.rs` reads `session_message` for a 2.x session, the table chosen by the database | #1009                                            |
| 11 | state, Q6                  | The usage scan reads both sets of tables, the folder from `session_v2.directory`       | #1009                                            |
| 12 | state, Q6                  | A test per database shape, fixture databases made by the real versions                 | #1009                                            |
| 13 | state, Q6                  | `opencode_db_path()` stays as it is                                                    | #1008, no change                                 |
| 14 | state, Q6                  | Tell the operator that 2.x token counts are 0 until the readers change                 | N3                                               |
| 15 | state, Q7                  | `status.rs`: the state plugin is chosen per version                                    | #1009                                            |
| 16 | state, Q7                  | The agent state for 2.x                                                                | N2                                               |
| 17 | state, Q7                  | A test that reads the resolved deny map through `opencode debug config` on 2.x         | #1009                                            |
| 18 | state, Q7                  | `build_opencode_command` writes the file plugin entry for 1.x only                     | #1009                                            |
| 19 | stream, Q3                 | The 2.x `error` event is read by `error.type`, `message`, `status`                     | #1009                                            |
| 20 | stream, Q3                 | A `retry` event is a degraded line, not an error and not a limit                       | #1009                                            |
| 21 | stream, Q3                 | 2.x usage from the `step_finish` tokens of the stream                                  | #1009, replaced: ADR-0094 D4h reads the database |
| 22 | stream, Q3                 | The tests read the `v1/` and `v2/` fixtures                                            | #1009                                            |
| 23 | stream, Q4                 | 2.x outcome from `error.type`; the wording regex only as a fallback                    | #1009                                            |
| 24 | stream, Q4                 | `provider.no-route` gets a message that names no login and a wrong model               | #1009                                            |
| 25 | stream, Q4                 | No limit from stderr on 2.x (`Failed to drain Session`, empty through the service)     | #1009                                            |
| 26 | stream, Q4                 | `retry` events reset the idle clock                                                    | #1009                                            |
| 27 | stream, Q4                 | A 2.x quota fixture built from the operator's Z.ai row                                 | #1009                                            |
| 28 | stream, Q4                 | 2.x takes its folder from `PWD`; set `PWD` for 2.x                                     | N1                                               |
| 29 | server, Q5                 | Every 2.x plan and execute uses `--standalone`; probe it on the operator's state first | #1009 (ADR-0094 D6a, D6d)                        |
| 30 | server, Q5                 | An interrupt call when a run goes through a server Ralphy did not start                | #1009, no code: ADR-0094 D6c forbids that path   |
| 31 | server, Q5                 | Set `PWD` in `command.rs` and the console spec, with a test                            | N1                                               |
| 32 | server, Q8                 | The console needs no new flag for the 2.x TUI                                          | #1008, no change                                 |
| 33 | server, Q8                 | Closing a console does not stop a prompt it sent                                       | no issue: ADR-0094 D7b keeps the shared service  |
| 34 | server, Q8                 | The `PWD` issue covers the console spec                                                | N1                                               |
| 35 | server, other observations | `provider.free-limit` is a usage limit; `retryAfterMs` is the wait                     | #1009 (ADR-0094 D4e; D4f drops the reset)        |
