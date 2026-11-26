# The OpenCode adapter supports 1.x and 2.x, reads the version once, and runs 2.x standalone

Status: accepted
Kind: structural
Protects: recoverability, observability and cost; constraint portability

Amends [ADR-0005](./0005-opencode-adapter.md) D2, D3, D5, D6 and D9. Epic #1011, spike #1010,
issues #1008 and #1009.

## Context

OpenCode 2 is a new product under the same program name. The spike of #1010 measured
OpenCode 1.19.0 and 2.3.2 on Windows 11 and in WSL Ubuntu-22.04 on 2026-11-24 and
2026-11-25. macOS was not probed. The facts are in
[docs/research/opencode-2-spike/](../research/opencode-2-spike/README.md); each rule below
names the file and the section it rests on.

The facts that force a decision:

- One name, two products. `opencode` is 1.x or 2.x depending on which installer ran last,
  and the folder does not tell which (`install-and-command-line.md`, Q1, *Installers*).
  1.x prints `1.19.0`; 2.x prints `opencode v2.3.2` (Q1, *Version output*).
- Two flags that the adapter passes break on 2.x: `--variant` and `--log-level ERROR`
  (`install-and-command-line.md`, Q2, *Command line*).
- A 2.x `run` with no server flag is a client of one shared background service per
  machine. A tree kill of the client does not stop the work: the service wrote and
  committed the change 42 s (Windows) and 52 s (WSL) after the kill. A `--standalone` run
  owns its server, and a tree kill leaves nothing (`server-and-console.md`, Q5, *Kill of
  the run's tree mid-run*).
- The 2.x stream keeps the 1.x envelope, but the `error` event is typed (`error.type`),
  and a new `retry` event has an `error` key that the parser of today reads as an error
  (`stream-and-outcomes.md`, Q3, *Event table*). An auth error and a usage limit are
  typed `error` events; the 1.x matchers miss both (Q4).
- A fresh 2.x database has no `message` and no `session` table, so both token readers
  return 0 for a 2.x session (`state-and-reads.md`, Q6). The 1.x state plugin does not
  load on 2.x (Q7).
- 2.x takes the folder of a session from `PWD` before the process folder, with and
  without the service (`stream-and-outcomes.md`, Q4 *Changes*; `server-and-console.md`,
  Q5, *The working folder*).

## Decision

### D1 — Ralphy supports OpenCode 1.x and 2.x, and refuses a run on any other major version

- **D1a.** A run, a plan and the one-shot sessions accept major version 1 and major
  version 2. Tested: 1.16.2 (ADR-0005), 1.18.32 (ADR-0005 D5 amendment), 1.19.0 and
  2.3.2 (the spike). 1.19.6 is the last 1.x release; 2.4.1 was the latest 2.x on the day
  of the spike and was not probed.
- **D1b.** A version whose major is not 1 or 2, or a version that cannot be read (D2),
  stops the run before the agent starts, with a message that names the version it read
  and the two supported majors. A run never guesses the version.
- **D1c.** The console is not refused by version: it starts the TUI of whatever is
  installed (D7).

### D2 — The version is read from `--version` once per binary and kept next to the located binary

- **D2a.** The version is read by running the located program with `--version`. The
  number is the last word of the first line, with a leading `v` removed: this reads both
  `1.19.0` and `opencode v2.3.2` (`install-and-command-line.md`, Q1, *Version output*).
  The folder and the file name are never used to decide the version (Q1 *Changes*).
- **D2b.** The reader and its parser live in `ralphy-proc-util`, in
  `crates/ralphy-proc-util/src/opencode.rs`, next to `locate_opencode` (D8). The crate is
  core-free, so the daemon roster, the doctor and the adapter use one reader. No other
  crate parses `opencode --version`.
- **D2c.** The result is one value, the located program and its version together. It is
  kept in memory, keyed by the path of the program and its modified time, so a process
  reads it once per binary and reads it again after an update. It is not written to disk.
- **D2d.** The read has a time limit of a few seconds; a read that times out is a version
  that cannot be read (D1b).
- **D2e.** The version is a closed set in the types: an enum of the two supported majors,
  with the full version text kept for display. Every place that differs by version
  matches it exhaustively.
- **D2f.** The doctor and the daemon roster show the version. The roster row of the
  OpenCode agent gains an optional `version` text; the row of every other agent leaves it
  out.

### D3 — The argv and the environment are chosen by the version, in one function

The adapter builds every `opencode run` (plan, execute, and the one-shots of `tasks.rs`)
in `build_opencode_command`, which takes the version (`install-and-command-line.md`, Q2,
*Command line*; `state-and-reads.md`, Q7):

| Part         | 1.x                                            | 2.x                                        |
| ------------ | ---------------------------------------------- | ------------------------------------------ |
| server       | none (the server is in the process)            | `--standalone` (D6)                        |
| permissions  | `--dangerously-skip-permissions`               | `--auto`                                   |
| logs         | `--print-logs --log-level ERROR`               | `--print-logs --log-level error`           |
| model        | `-m provider/model`, only when set             | the same                                   |
| variant      | `--variant <v>`, only when set                 | `-m provider/model#<v>`; never `--variant` |
| deny map     | `permission.bash` in `OPENCODE_CONFIG_CONTENT` | the same key                               |
| skills       | `skills.paths` in `OPENCODE_CONFIG_CONTENT`    | the same key                               |
| state plugin | the file entry in `plugin`                     | none (D5a)                                 |
| folder       | `current_dir(root)` and `PWD=<root>`           | the same                                   |
| keys removed | `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`          | the same                                   |

- **D3a.** A variant set with no model is not passed on 2.x, because the variant has no
  flag of its own there; the adapter says so in the run log.
- **D3b.** `PWD` is set to the run folder for both versions: 1.x ignores it, and 2.x
  uses it as the folder of the session. Without it a 2.x run started by the daemon works
  in the folder of the inherited `PWD` (#278 again). The daemon's `OpenCode` console spec
  sets it too.
- **D3c.** `--auto` replaces the hidden flag on 2.x because the hidden flag is in no 2.x
  help text and can go with no notice. The deny map must still block under `--auto`; the
  tool name in the 2.x denial is `shell` (`state-and-reads.md`, Q7).

### D4 — The stream and the outcome are read by the version

The 1.x readers stay as they are. For 2.x (`stream-and-outcomes.md`, Q3 and Q4):

- **D4a.** Text, the sentinels and the session id are read as on 1.x: the envelope, the
  `text` event and the top-level `sessionID` did not change.
- **D4b.** The `error` event is read by `error.type`, `error.message` and
  `error.status`. The 1.x reader of `error.name` and `error.data` does not run on 2.x.
- **D4c.** A `retry` event is not an error and not a limit. It is a degraded line; the
  run goes on, and the idle clock counts it as output. Only the last `error` event and
  the exit decide the outcome.
- **D4d.** `provider.auth` is the auth error, with the same precedence as on 1.x.
  `provider.no-route` ("Model unavailable") is a stop with the same precedence, and its
  message names both causes: no login for that provider, or a wrong model id.
- **D4e.** `provider.quota`, `provider.free-limit`, `provider.go-limit`,
  `provider.budget-limit`, `provider.model-budget-limit`, `provider.chatgpt-limit`, and a
  last `provider.rate-limit` are a usage limit. Every other type is an error (`Stuck`).
  The wording regex of 1.x stays only as a fallback for an `error.message` with an
  unknown type.
- **D4f.** ADR-0005 D9a holds for both versions: the limit is always `Limit(None)`.
  `retryAfterMs` is not used as the reset.
- **D4g.** On 2.x the limit is a typed stdout event that ends the process with exit 1, so
  the stderr early kill of ADR-0005 D9b runs for 1.x only. On 2.x, stderr holds the
  `level=INFO` lines of the standalone server, and the stderr scan must accept them.
- **D4h.** The token usage of a run comes from the database for both versions; it is not
  summed from the stream. The reader picks the table by what the database has, not by
  the version: `message` for a 1.x session and `session_message` with
  `type = 'assistant'` for a 2.x session, with the model from `model.id`. The usage scan
  reads both sets of tables, because an upgraded database holds old sessions in the old
  tables and new ones in the new tables (`state-and-reads.md`, Q6).
- **D4i.** Until the readers of D4h ship, a 2.x run whose token count is 0 says that the
  count is not read for this version. A failed query is not shown as zero usage.

### D5 — The 1.x state plugin is for 1.x only

- **D5a.** A 2.x run is started with no state plugin, because 2.x loads only plugin
  folders and its plugin API is a default export with an `id` (`state-and-reads.md`, Q7).
  Until a 2.x plugin exists, a 2.x run gives the attention list no agent state; the run
  outcome is not changed by this.
- **D5b.** The agent state of 2.x comes from a plugin folder written to the 2.x plugin
  API that writes the same status lines. The server event stream is not the source:
  with `--standalone` (D6) the server talks only to its own client, on stdio.

### D6 — Every 2.x run Ralphy starts uses `--standalone`, with the operator's own state

- **D6a.** Every plan, execute and one-shot session on 2.x passes `--standalone`. The run
  owns its server and its tools, so the tree kill of the timeout, of the limit and of the
  daemon stops the work (`server-and-console.md`, Q5, *Kill of the run's tree mid-run*).
  The server log reaches stderr only in this mode (`stream-and-outcomes.md`, Q4, *Usage
  limit*). The run leaves no process and no file watcher after it ends (Q5, *What a run
  starts, and what stays*). Two standalone runs at once stay apart and share no port (Q5,
  *Two runs at once*).
- **D6b.** The run uses the operator's own `XDG_*` folders, where the 2.x login is (the
  `credential` table, `state-and-reads.md`, Q6). Ralphy does not give a 2.x run a scratch
  state folder.
- **D6c.** Ralphy never runs a 2.x plan or execute through the shared background service,
  and never passes `--server`. A run that goes through a server Ralphy did not start would
  need an interrupt call on every stop; that path is rejected (Considered options).
- **D6d.** Precondition, not probed by the spike: a standalone run on the operator's own
  state while the service runs on the same database. The spike ran two standalone servers
  on one scratch database at once with no error (Q5, *Two runs at once*). #1009 probes the
  case with the operator's state on Windows and WSL before it ships D6a. If the probe
  fails, this ADR is amended before the code, not worked around in it.
  Result: the probe passed on Windows and WSL. A 2.3.2 `--standalone` run on the
  operator's state exited 0 and committed while the service ran, and the service still
  answered after it (`docs/research/opencode-2-spike/live-checks-1009.md`).
- **D6e.** `opencode models` is a passthrough that writes nothing; it may use the
  service. #1009 passes `--standalone` to it when 2.x accepts the flag there, so a model
  list does not start a service the operator did not start.

### D7 — The console starts the 2.x TUI on the shared service, as the operator's own OpenCode

- **D7a.** The daemon's `OpenCode` console passes no server flag. The 2.x TUI joins the
  shared service and starts no server of its own; ended by PID, it leaves nothing but the
  service, which was there before (`server-and-console.md`, Q8).
- **D7b.** Closing or hibernating an `OpenCode` console ends the TUI but not a prompt it
  sent: the work runs in the service, as in the kill probe. This is accepted. A console
  is the operator's own session, and other clients of the service see the same sessions.
  The daemon does not call the interrupt route for a console.
- **D7c.** The console sets `PWD` to its folder (D3b).

### D8 — The program is found by the shared locator in `ralphy-proc-util`

As built by #1008 (commits `8b32f21e` and `a66a3c30`):

- **D8a.** `ralphy_proc_util::opencode::locate_opencode()` is the one locator. Its order
  is `PATH`, `~/.local/bin`, nvm, then `<home>/.opencode/bin/opencode` (`opencode.exe` on
  Windows). A copy on `PATH` wins, so the operator's choice of `PATH` decides.
- **D8b.** `opencode2` is never looked for: it is a shim that runs the `opencode` next to
  it (`install-and-command-line.md`, Q1, *Installers*).
- **D8c.** The run path, the doctor, `init`, the daemon roster and the console spec call
  this locator; none of them resolves `opencode` by name on its own.
- **D8d.** A Windows mount path is never used on Linux: the roster skips it, as for
  every agent.

## Consequences

- ADR-0005 is amended: D2 (the stream for 2.x), D3 (the variant on 2.x), D5 (`--auto` on
  2.x), D6 (the typed auth error) and D9 (the typed limit; the early kill is 1.x only).
  ADR-0005 keeps every rule for 1.x.
- Every `opencode` session costs one more short spawn per binary and process, for
  `--version`. On Windows the npm shim starts `cmd.exe` for it.
- The roster's wire format gains one optional field. The workbench shows it when it is
  there and does nothing when it is not.
- A 2.x run that is killed writes no `step_finish`, so its usage is what the database
  has at the kill, as for 1.x.
- 2.x on macOS is not measured. The same folders are documented, and the release workflow
  runs the tests there; a live macOS run is outside this ADR.
- Not decided here, and left to the issues of the change list in the spike index: the
  2.x plugin of D5b and the use of `--archived` and `--title` for scripted runs.

## Considered options

- **Run 2.x through the shared service, and call the interrupt route on every stop.**
  Rejected: the work then outlives every tree kill until a second call succeeds; the
  stderr log and the early kill see nothing; the service watches every run folder for
  about an hour; and the interrupt needs a session id that a run killed early may not
  have printed (`server-and-console.md`, Q5).
- **A scratch state folder for each 2.x run.** Rejected: the 2.x login is in the database
  of the state folder, so a scratch folder is a run with no login.
- **Decide the version from the folder** (`~/.opencode/bin` means 2.x). Rejected: both
  installers write the same path, and npm installs both majors in one folder.
- **Sum the stream's `step_finish` tokens for 2.x usage.** Rejected: it is a second
  computation of a fact the database owns, and the usage scan reads the database. The run
  total and the scan total would differ.
- **Trust `retryAfterMs` as the reset of a 2.x limit.** Rejected for now: ADR-0005 D9a
  keeps every OpenCode reset out of the schedule. A later ADR may trust a typed field.
- **An amendment inside ADR-0005.** Rejected: the version split is a decision of its own
  that #1009 and later vendors cite by number.

## Compliance

- D1: checked by `the_1x_argv_is_unchanged` and `the_2x_argv_runs_standalone_with_auto`
  (`crates/ralphy-agent-opencode/src/command/tests.rs`), and by
  `a_third_major_stops_the_run_before_the_spawn` and
  `an_unreadable_version_stops_the_run_before_the_spawn`
  (`crates/ralphy-agent-opencode/src/phases.rs`).
- D2: checked by `crates/ralphy-proc-util/src/opencode/tests.rs`
  (`the_version_is_read_from_both_outputs`, `an_unreadable_version_is_refused`,
  `a_third_major_is_refused_with_the_text_it_read`), by
  `crates/ralphy-proc-util/tests/opencode_version.rs`
  (`a_version_read_past_its_limit_is_unreadable`,
  `the_opencode_version_is_read_once_per_binary`), by
  `doctor_reads_the_opencode_version_through_proc_util`
  (`crates/ralphy-cli/src/doctor/probe.rs`) and by `the_opencode_row_carries_its_version`
  (`crates/ralphy-daemon/src/roster.rs`). The core-free rule is checked by
  `daemon_manifest_has_no_vendor_dependency` (`crates/ralphy-daemon/src/roster.rs`).
- D3: checked by `the_2x_argv_runs_standalone_with_auto`,
  `a_2x_variant_without_a_model_is_not_passed`, `a_dropped_2x_variant_is_logged` and
  `the_run_command_sets_pwd_to_its_folder`
  (`crates/ralphy-agent-opencode/src/command/tests.rs`), and by
  `an_opencode_console_sets_pwd_to_its_folder` (`crates/ralphy-daemon/src/session/spec/tests.rs`).
  The deny map under `--auto` is checked live by `a_denied_call_is_refused_under_auto_on_2x`
  (ignored by default; it needs a vendor CLI and a model that answers).
- D4: checked by `v1_fixtures_classify_as_before` (`crates/ralphy-agent-opencode/src/events/tests.rs`)
  and `crates/ralphy-agent-opencode/src/events/v2/tests.rs` (`v2_execute_fixtures_end_done`,
  `a_2x_retry_event_is_not_an_error`, `the_2x_auth_fixture_is_the_auth_error`,
  `every_2x_limit_type_is_an_unschedulable_limit`, `an_unknown_2x_error_type_is_stuck`),
  which read `crates/ralphy-agent-opencode/tests/fixtures/v1/` and `v2/`. D4h is checked by
  `a_1x_session_is_read_from_the_message_table`, `a_2x_session_is_read_from_session_message`
  and `an_upgraded_database_reads_both_sessions` (`crates/ralphy-agent-opencode/src/usage.rs`)
  and by `opencode_scan_reads_both_table_sets` (`crates/ralphy-usage-scan/src/opencode/tests.rs`).
- D5: checked by `a_2x_command_names_the_plugin_folder`
  (`crates/ralphy-agent-opencode/src/command/tests.rs`), `a_2x_watcher_writes_the_plugin_folder`
  (`crates/ralphy-agent-opencode/src/status/tests.rs`) and `a_2x_console_loads_the_2x_plugin` and
  `a_1x_console_keeps_the_file_plugin` (`crates/ralphy-daemon/src/session/spec/tests/opencode_plugin.rs`).
  That the 2.x plugin writes the same status lines as 1.x is a live probe
  (`docs/research/opencode-2-spike/plugin-v2.md`).
- D6: D6a is checked by `the_2x_argv_runs_standalone_with_auto`, and D6e by
  `the_models_command_follows_the_version` (`crates/ralphy-agent-opencode/src/tasks.rs`).
  D6b and D6d need a live vendor CLI: the D6d probe passed on Windows and WSL
  (`docs/research/opencode-2-spike/live-checks-1009.md`), and the live acceptance
  runs are in `docs/spike/evidence/1009-opencode-1-and-2-live.md`.
- D7: not checked by code: needs a live vendor CLI; manual, reviewed in the PR of the
  console change.
- D8: checked by `crates/ralphy-proc-util/src/opencode/tests.rs`
  (`the_installer_folder_is_found_when_path_has_none`,
  `a_path_copy_wins_over_the_installer_folder`, `the_opencode2_shim_is_never_found`) and
  `the_program_comes_from_the_shared_opencode_locator`
  (`crates/ralphy-agent-opencode/src/command.rs`).
