# OpenCode 2 spike: the state plugin on the 2.x plugin API

Spike of #1042 (ADR-0094 D5b). Markers are in the [README](README.md). Raw output
is in `docs/spike/live/opencode-1042-*.log`, and the status files the checks
compare are `docs/spike/live/opencode-1042-status-<v1|v2>-<win|wsl>.jsonl`.

Versions probed: OpenCode 2.3.2 (the operator's install, always with
`--standalone` and a scratch `XDG_*` tree) and OpenCode 1.19.0 (scratch install,
own `XDG_*` tree). Scratch roots: `%TEMP%\ralphy-1042\` and `/tmp/ralphy-1042/`.
The plugin is `assets/opencode/ralphy-state-plugin-v2/` (`index.js` and
`package.json`). The 1.x plugin `assets/opencode/ralphy-state-plugin.js` is not
changed.

**Plugin API answer:** pass: a 2.x plugin is a folder whose entry has a default export `{ id, setup(ctx) }`; it reads events from `ctx.event.subscribe()`; the config entry is a `file://` URL of the folder in `plugin` (read as `plugins`) 🔬 📖

## Export shape

- 2.x skips a config path that is a file: "Configured local paths must be
  directories" (opencode.ai/v2/docs/plugins) 📖, and the 2.3.2 log says
  `configured plugin path must be a directory` (`opencode-1010-state-plugin-dir-win-v2.log`) 🔬.
- In a folder, the entry is the `"."` entry of `exports` in `package.json`; a
  folder without `exports` falls back to a file named `server` or `index`
  (opencode.ai/v2/docs/build/plugins) 📖. The plugin's `package.json` names
  `./index.js` 🔬.
- The default export is `{ id, setup }` (or `effect` for an Effect plugin).
  `setup(ctx)` runs when the plugin loads and may return a cleanup function
  (opencode.ai/v2/docs/build/plugins/server) 📖. A plain object works: the docs
  wrap it in `Plugin.define` from `@opencode/plugin`, but the plugin imports
  only `node:fs` and loads with no install (Windows and WSL, 2.3.2) 🔬.
- A named export that returns hooks, the 1.x shape, is rejected: `Plugin must
  export a default definition with an id and an effect or s...` (2.3.2) 🔬. One
  file can carry both shapes (a default object with `id`, `setup` and a 1.x
  `server()`), but the docs say this object form needs 1.x 1.18.29 or later 📖,
  so Ralphy keeps two files.
- `ctx` has these keys on 2.3.2 (Windows): `app`, `location`, `options`,
  `agent`, `aisdk`, `command`, `credential`, `event`, `experimental`,
  `generate`, `model`, `provider`, `integration`, `mcp`, `permission`,
  `plugin`, `reference`, `rpc`, `skill`, `storage`, `tool`, `vcs`, `websearch`,
  `worktree`, `session`, `shell` 🔬 (`opencode-1042-probe-win-v2.log`).
- Events: `for await (const event of ctx.event.subscribe({ signal }))` 📖 🔬. An
  event is `{ id, created, type, location?, durable?, data }`: the payload is in
  `data`, where 1.x had `properties` 🔬.
- In one probe on a fresh scratch tree, `setup` ran twice in one process
  (`opencode-1042-probe-win-v2.log`, two `loaded` lines and every event twice) 🔬.
  The plugin keeps its state on `globalThis` and the newest `setup` takes the
  stream, so `plugin.loaded` and each line are written once.

## Config entry

`OPENCODE_CONFIG_CONTENT={"plugin":["file:///<folder>"]}` with the folder, not a
file 🔬. The 2.x docs name the key `plugins` 📖; 2.3.2 reads `plugin` as an alias
(`debug config` in `state-and-reads.md` Q7) 🔬. The `file://` URL is
`file:///C:/Users/<user>/AppData/Local/Temp/ralphy-1042/plugin-v2` on Windows
and `file:///tmp/ralphy-1042/plugin-v2` on WSL. `RALPHY_STATUS_FILE` reaches the
plugin through `process.env` on both hosts 🔬. The log shows `loading plugin`
with `entrypoint=.../plugin-v2/index.js` (log level `info`) 🔬.

## Event mapping

The 2.x names come from the 2.3.2 event schemas in the binary 🔎 and, where a
headless run raises them, from the stream 🔬. The plugin writes the 1.x line names
and fields, so `opencode_signal` and the daemon map need no change.

| 1.x line             | 1.x event read                          | 2.x event read                                                                                  | Fields of the line                                               | Marker |
| -------------------- | --------------------------------------- | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | ------ |
| `plugin.loaded`      | the plugin loads                        | `setup` runs (once per process)                                                                 | none                                                             | 🔬     |
| `session.created`    | `session.created` without `parentID`    | `session.created` without `data.parentID`, or the first busy event of a session not seen before | `session_id`                                                     | 🔬 🔎  |
| `session.status`     | `session.status` with `type: "busy"`    | `session.execution.started`, `session.step.started`, `session.step.ended`                       | `session_id`, `agent_id` for a subagent                          | 🔬     |
| `session.idle`       | `session.idle`                          | `session.execution.succeeded`, `session.execution.failed`, `session.execution.interrupted`      | `session_id`, `agent_id` for a subagent                          | 🔬 🔎  |
| `permission.asked`   | `permission.asked` (`permission`, `id`) | `permission.asked` (`data.action`, `data.id`)                                                   | `session_id`, `tool_name`, `tool_use_id`                         | 🔎 📖  |
| `permission.replied` | `permission.replied` (`requestID`)      | `permission.replied` (`data.requestID`)                                                         | `session_id`, `tool_use_id`                                      | 🔎 📖  |
| `question.asked`     | `question.asked` (`questions[0]`)       | `form.created` (`data.form.id`, `data.form.sessionID`, `data.form.title`)                       | `session_id`, `tool_name: "question"`, `tool_use_id`, `question` | 🔎 📖  |
| `question.replied`   | `question.replied` (`requestID`)        | `form.replied` (`data.id`)                                                                      | `session_id`, `tool_use_id`                                      | 🔎 📖  |
| `question.rejected`  | `question.rejected` (`requestID`)       | `form.cancelled` (`data.id`)                                                                    | `session_id`, `tool_use_id`                                      | 🔎 📖  |

A subagent is a session whose `session.created` has `data.parentID` 🔎; its lines
name it in `agent_id`, as in 1.x.

Notes:

- 2.3.2 did not deliver `session.created` for the session of `opencode run`:
  the session exists before the plugin subscribes (`opencode-1042-probe-win-v2.log`,
  the first session event is `session.inbox.enqueued`) 🔬. So the plugin writes
  `session.created` when it first sees a lead session's busy event, before the
  `session.status` line. The 1.x order (`plugin.loaded`, `session.created`, then
  busy lines) is kept.
- One prompt with one step: 1.19.0 writes `session.status` busy three times
  (prompt start, step start, step end) 🔬. 2.3.2 raises
  `session.execution.started`, `session.step.started` and `session.step.ended`
  once each for that prompt 🔬, so the counts match for one step. A repeated busy
  line does not change the state Ralphy maps.
- 2.x has no `session.status` and no `session.idle` event: the turn ends with
  `session.execution.succeeded`, `failed` or `interrupted` 🔬 🔎. A run that
  hit the free limit ended with `session.step.failed` and
  `session.execution.failed` 🔬.
- `permission.*` and the form events (the 2.x questions) cannot be raised by a
  free headless `opencode run`: the run allows every tool and has no one to
  answer a form. Their mapping is read from the 2.3.2 schemas in the binary
  (`Permission.Request` = `id`, `sessionID`, `action`, `resources`, `source`,
  `message`; `form.created` = `{ form: { id, sessionID, title, fields } }`;
  `form.replied` = `{ id, sessionID, answer }`; `form.cancelled` = `{ id,
  sessionID }`) 🔎, and from the TUI code in the binary that ends a wait on
  `permission.replied`, `form.replied` and `form.cancelled` 🔎. Not observed.
  `tool_name` of a permission is the 2.x `action` (for example `shell`).

## Load and status lines

The probe prompt is "Reply with the word ok." with the free model
`opencode/hy4-flash-free` (the free `deepseek-v4.1-flash-free` was over its free
limit on this machine on the day of the probe) 🔬.

**Load answer (Windows):** pass: 2.3.2 on Windows 11 logs `loading plugin` for the folder, has no `failed to load plugin` line, and the status file starts with `plugin.loaded` (`opencode-1042-load-win-v2.log`) 🔬

**Load answer (WSL):** pass: 2.3.2 on WSL Ubuntu-22.04 logs `loading plugin` for the folder, has no `failed to load plugin` line, and the status file starts with `plugin.loaded` (`opencode-1042-load-wsl-v2.log`) 🔬

Windows, one prompt (`opencode-1042-status-v1-win.jsonl` and
`opencode-1042-status-v2-win.jsonl`): both files hold `plugin.loaded`,
`session.created`, three `session.status` and `session.idle`, with the same keys
per line 🔬.

WSL, one prompt: the 1.19.0 file (`opencode-1042-status-v1-wsl.jsonl`) holds the same six lines as on
Windows 🔬. Every 2.3.2 run on WSL hit the free limit of the model
(`opencode-1042-load-wsl-v2.log`, `opencode-1042-load-wsl-v2-limited.log`): the plugin wrote
`plugin.loaded`, `session.created`, two `session.status` and `session.idle`, the lines of a turn
whose one step failed 🔬. With the operator's provider (`anthropic/claude-haiku-5-5`, `--standalone`),
the 2.3.2 file of a turn that ends with a reply (`opencode-1042-status-v2-wsl.jsonl`, run log
`opencode-1042-status-v2-wsl-run.log`) holds the same six lines as the 1.19.0 file 🔬.
