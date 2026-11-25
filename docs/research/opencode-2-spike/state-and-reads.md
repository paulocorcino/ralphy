# OpenCode 2: state on disk and what Ralphy reads

Spike of #1010, questions Q6 and Q7. Plan stage 3. Markers are in the
[README](README.md). Raw output is in `docs/spike/live/opencode-1010-state-*.log`.

Versions probed: OpenCode 1.19.0 (scratch install, own `XDG_*` tree) and
OpenCode 2.3.2 (the operator's install). Reads of the operator's 2.x database
used a copy of the three files (`opencode.db`, `-wal`, `-shm`) in the scratch
root. The operator's files were not opened.

## Q6: where 2.x keeps its state, and what changed in the database

### Folders

`opencode debug paths` prints them (`state-layout-win-v2.log`,
`state-layout-wsl-v2.log`) 🔬. 1.x and 2.x use the same folders, and both honor
`XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` 🔬
(a scratch tree changed every path in `install-config-win-v1.log` and
`install-config-win-v2.log`).

| What                    | Windows 2.3.2                                                                     | WSL 2.3.2                                              | Note                                                            |
| ----------------------- | --------------------------------------------------------------------------------- | ------------------------------------------------------ | --------------------------------------------------------------- |
| data folder             | `%USERPROFILE%\.local\share\opencode`                                             | `~/.local/share/opencode`                              | the same folder for 1.x and 2.x                                 |
| database                | `<data>\opencode.db` with `-wal` and `-shm`                                       | the same                                               | WAL mode, `user_version` 0                                      |
| logs                    | `<data>\log\` (`opencode.log`, and dated files from 1.x)                          | `<data>/log/opencode.log`                              | 2.x writes one `opencode.log`                                   |
| sessions                | rows in the database                                                              | rows in the database                                   | no session files                                                |
| snapshots, shell, repos | `<data>\snapshot`, `shell`, `repos`, `tool-output`                                | `<data>/snapshot`, `shell`, `repos`                    |                                                                 |
| login                   | the `credential` table (3 rows); `auth.json` and `account.json` are old 1.x files | the `credential` table (1 row); no `auth.json`         | a copy of `auth.json` does not carry a 2.x login                |
| config                  | `%USERPROFILE%\.config\opencode` (`opencode.jsonc`, `service.json`)               | `~/.config/opencode` (`opencode.json`, `service.json`) |                                                                 |
| state                   | `%USERPROFILE%\.local\state\opencode` (`service.json`, `model.json`, `locks`)     | the same under `~`                                     | `service.json` holds the url and port of the background service |
| cache                   | `%USERPROFILE%\.cache\opencode` (`models.json`, a copy of the service binary)     | `~/.cache/opencode` (`models.json`)                    |                                                                 |
| temp                    | `%TEMP%\opencode`                                                                 | `/tmp/opencode`                                        |                                                                 |

macOS 📖: the same XDG layout (`~/.local/share/opencode`); not probed.

The database is shared by the two versions when `HOME` is the same. The 2.x
database of the operator has 62 migrations; a fresh 1.19.0 database has 38.
Whether 1.x can open a database that 2.x migrated was not probed: the scratch
rule keeps 1.x away from the operator's database.

### Tables that the usage scan reads

Both `ralphy-agent-opencode/src/usage.rs` (`SELECT data FROM message WHERE
session_id = ?`) and `ralphy-usage-scan/src/opencode.rs` (`message` joined to
`session.directory`) read the legacy tables `message` and `session`.

| Database                        | `message`                     | `session` | `session_v2` | `session_message`             | Log                                                                                 |
| ------------------------------- | ----------------------------- | --------- | ------------ | ----------------------------- | ----------------------------------------------------------------------------------- |
| 1.19.0, fresh, one run          | 9 rows                        | 8 rows    | no table     | 0 rows                        | `state-schema-win-v1-scratch.log`                                                   |
| 2.3.2, fresh, one run           | no table                      | no table  | 6 rows       | 13 rows                       | `state-schema-win-v2-scratch.log`, `state-v2-tables-win-v2-scratch.log`             |
| 2.3.2, operator, Windows (copy) | 2 383 rows, newest 2026-11-16 | 194 rows  | 195 rows     | 2 477 rows, newest 2026-11-22 | `state-schema-win-v2-operator-copy.log`, `state-v2-tables-win-v2-operator-copy.log` |
| 2.3.2, operator, WSL (copy)     | 44 rows                       | 10 rows   | 12 rows      | 53 rows                       | `state-schema-wsl-v2-operator-copy.log`, `state-v2-tables-wsl-v2-operator-copy.log` |

All numbers are 🔬.

- A fresh 2.x database has **no `message` table and no `session` table**.
- The operator's databases have both sets, because 2.x migrated a 1.x database
  and kept the old tables. The old tables stopped growing when the operator
  moved to 2.x (Windows: newest `message` 2026-11-16, newest `session_message`
  2026-11-22). New 2.x sessions go to the new tables only.
- The legacy tables have the same set of columns in 1.19.0 and in both migrated
  2.x databases (the order differs in the older Windows one): `message(id, session_id, time_created, time_updated, data)` and
  `session(id, project_id, ..., directory, ..., tokens_input, ..., model,
  time_created, ...)`. Only the new ones changed.

The new tables (DDL in `state-v2-tables-win-v2-scratch.log`):

- `session_v2`: the 1.x `session` columns plus `fork_session_id`,
  `fork_boundary`, `time_busy`, `time_idle`, `time_viewed`, `idle_outcome`,
  `time_suspended`, `resume_attempts`. `directory` is the folder of the process
  that ran (the scratch repo, in the probe). `model` is a JSON text
  `{"id":..., "providerID":...}`. Tokens are columns (`tokens_input`, ...).
- `session_message(id, session_id, type, seq, time_created, time_updated,
  data)`. `type` is `user`, `assistant`, `system`, `synthetic`, `idle` or
  `model-switched`.
- `session_part(message_id, session_id, ordinal, type, call_id, ..., data)`
  and `event(id, aggregate_id, seq, created, type, data)`.

The `data` of an assistant row, as the keys of the JSON (🔬, both hosts):

| Field the usage code reads    | 1.x `message.data`                                  | 2.x `session_message.data` (`type = 'assistant'`)                    |
| ----------------------------- | --------------------------------------------------- | -------------------------------------------------------------------- |
| which rows are assistant rows | `role == "assistant"`                               | the `type` column is `assistant`; there is no `role`                 |
| model                         | `modelID` (and `providerID`)                        | `model.id` (and `model.providerID`, `model.variant`)                 |
| input, output, reasoning      | `tokens.input`, `tokens.output`, `tokens.reasoning` | the same keys                                                        |
| cache                         | `tokens.cache.read`, `tokens.cache.write`           | the same keys                                                        |
| total                         | `tokens.total`                                      | not present                                                          |
| time                          | `time.created` (unix ms)                            | `time.created` (unix ms), plus `firstToken`, `streamed`, `completed` |
| cwd                           | `path.cwd` and `path.root`                          | not present; the folder is `session_v2.directory`                    |

**Q6 answer:** 2.x keeps everything in the same XDG folders as 1.x, in one
SQLite database (`opencode.db`, WAL), with the login in a `credential` table.
The token data moved: a fresh 2.x database has no `message` and no `session`
table, and the assistant rows are in `session_message` (`type = 'assistant'`,
with `model.id` and the same `tokens` keys) under `session_v2`. Both token
readers of Ralphy therefore return zero for a 2.x run, with no error, because
each swallows the failed query.

**Changes:**

- #1009: `ralphy-agent-opencode/src/usage.rs` must read `session_message` for a
  2.x session: `SELECT data FROM session_message WHERE session_id = ?1 AND type
  = 'assistant' ORDER BY time_created`, and take the model from `model.id`.
  Pick the table by what exists in the database, not by the version number,
  because the operator's database has both.
- #1009: the same change in `ralphy-usage-scan/src/opencode.rs`, with the
  directory from `session_v2.directory`. The scan must read both sets: an
  upgraded database holds old sessions in `message` and new ones in
  `session_message`, and no session is in both.
- #1009: a test for each shape, with a fixture database made by the real
  versions (a fresh 2.x database and an upgraded one).
- #1008: no change. The data folder is the same for both versions, so
  `opencode_db_path()` stays right.
- New issue under #1011: tell the operator that the token counts of 2.x are 0
  until the readers change; today nothing says so, because a failed query is an
  empty result.

## Q7: what the state plugin, skills and the task sessions need

Probe: one run per version and host with `OPENCODE_CONFIG_CONTENT` set to the
three things Ralphy sets: `skills.paths` (a probe skill with a unique name),
`permission.bash` (a deny rule for `echo`), and `plugin` (the real state plugin
of `assets/opencode/ralphy-state-plugin.js`), with `RALPHY_STATUS_FILE` set
(`state-skills-plugin-win-*.log`, `state-skills-detail-*.log`,
`state-plugin-dir-win-v2.log`, `state-models-win.log`). The free model
`opencode/deepseek-v4.1-flash-free` was used. 2.x ran with `--standalone`.

| Reader                | What it passes or reads                                                                                           | 1.19.0                                                                                                                 | 2.3.2                                                                                                                      |
| --------------------- | ----------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `skills.rs`           | `skills.paths` in the config; skills are folders with a `SKILL.md`                                                | works: the probe skill is in the model's list (Windows, WSL) 🔬                                                        | works: the probe skill is in the model's list (Windows, WSL) 🔬                                                            |
| `command.rs` deny map | `permission.bash` with `deny`, under `--dangerously-skip-permissions`                                             | works: the call returned an error naming the rule 🔬                                                                   | works: the call returned `Permission denied: shell`. The tool is now named `shell`, and the text does not show the rule 🔬 |
| `status.rs` plugin    | `plugin: ["file:///.../ralphy-state-plugin.js"]`, events through `event()`, status file from `RALPHY_STATUS_FILE` | works: `plugin.loaded`, `session.created`, `session.status`, `session.idle` lines in the status file (Windows, WSL) 🔬 | **does not work**: the status file is never created 🔬                                                                     |
| `tasks.rs`            | `opencode models` as a passthrough; `run` through `build_opencode_command`                                        | `models` prints 10 lines on the scratch login (free models) 🔬                                                         | `models` prints 36 lines on the operator login, one `provider/model` per line 🔬                                           |

Why the plugin fails on 2.x (🔬 and 📖):

- With a file entry the log says `configured plugin path must be a directory`
  (Windows and WSL). The docs say the same: "Configured local paths must be
  directories. A path to a single file is skipped" (opencode.ai/v2/docs/plugins).
- With a directory (`index.js` plus a `package.json`) the plugin is found
  (`loading plugin ... entrypoint=.../index.js`) and then rejected:
  `failed to load plugin ... Plugin must export a default definition with an id
  and an effect or s...` (the text is cut in the raw log). The 1.x plugin has a
  named export, `RalphyAgentState`, that returns an `event` hook. The 2.x
  plugin has another API: a default export with an `id` and, per the docs, parts
  named `server`, `tui`, `rpc` and `gui`.
- The config key is `plugins` in the 2.x docs. `debug config` shows that the 1.x
  key `plugin` is read as an alias (the value is listed under `plugins`).

So on 2.x Ralphy gets no agent-state events from a file plugin. The state of the
agent (waiting for a permission or a question, turn end) has to come from
another source: a 2.x plugin written to the 2.x API, or the server events.

Other findings:

- `skills.paths` is accepted as the same key; `debug config` lists it as
  `skills`, a plain list of paths.
- `opencode models` on 2.x does not need `--standalone`: it answered through
  the background service. `list_models` runs it with the inherited `PATH` only.
- `diagnose_repo`, `draft_issues`, `triage_issues` and `consolidate_knowledge`
  all go through `build_opencode_command`, so they have the Q2 problems and no
  others.

**Q7 answer:** On 2.3.2, `skills.paths`, the forge-write deny map and `opencode
models` work as on 1.x, but the state plugin does not: 2.x loads only plugin
directories, and its plugin API (a default export with an `id`) is not the 1.x
named-export hook, so no status line is written. The deny map still blocks under
`--dangerously-skip-permissions`; its tool name in 2.x is `shell`.

**Changes:**

- #1009: `status.rs` and the plugin: do not name the 1.x plugin file for a 2.x
  binary (it prints a warning and does nothing). Choose per version.
- New issue under #1011: the agent state for 2.x. Two candidates: a plugin
  directory written to the 2.x plugin API with the same `status` lines, or the
  event stream of the server (the server stage measures it). The 1.x plugin
  stays for 1.x.
- #1009: keep the deny map as it is for 2.x, but add a test that reads the
  resolved config (`opencode debug config`) with the forge-write rules, since
  the 2.x document form is `permissions: [{action: "shell", ...}]`.
- #1009: `build_opencode_command` writes the file plugin entry only for 1.x.

### Live-probe cost

| Probe                                                        | Host         | Version       | Model                               | Tokens                                                |
| ------------------------------------------------------------ | ------------ | ------------- | ----------------------------------- | ----------------------------------------------------- |
| `debug paths`, `debug config`, `models`                      | Windows, WSL | 1.19.0, 2.3.2 | none                                | none                                                  |
| database schema dumps (scratch and copies of the operator's) | Windows, WSL | 1.19.0, 2.3.2 | none                                | none                                                  |
| skills, deny and plugin run (2 prompts)                      | Windows      | 1.19.0        | `opencode/deepseek-v4.1-flash-free` | about 49 500 total (cost 0)                           |
| skills, deny and plugin run (2 prompts)                      | Windows      | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | about 32 700 total (cost 0)                           |
| skills and plugin detail run                                 | Windows, WSL | 1.19.0, 2.3.2 | `opencode/deepseek-v4.1-flash-free` | about 25 000 each on 1.x, 16 000 each on 2.x (cost 0) |
| plugin directory run                                         | Windows      | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | about 16 000 (cost 0)                                 |
