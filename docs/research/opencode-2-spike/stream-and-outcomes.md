# OpenCode 2: the event stream and the three outcome signals

Spike of #1010, questions Q3 and Q4. Plan stage 4. Markers are in the
[README](README.md). Raw output is in `docs/spike/live/opencode-1010-stream-*.log`.
The `--format json` stdout of each run is a fixture in
`crates/ralphy-agent-opencode/tests/fixtures/v1/` and `.../v2/`, redacted (home
folder, user name, request ids).

Versions probed: OpenCode 1.19.0 (scratch install) and OpenCode 2.3.2 (the
operator's install), on Windows 11 (from git-bash, where `/tmp` is `%TEMP%`) and
in WSL Ubuntu-22.04. macOS is not probed.

## How the runs were made

Every run used the argv that `command.rs` builds today, with the per-version
changes of the command-line table in
[install-and-command-line.md](install-and-command-line.md):

- 1.x: `opencode run --format json --dangerously-skip-permissions --print-logs
  --log-level ERROR -m opencode/deepseek-v4.1-flash-free`.
- 2.x: the same with `--log-level error`, once with `--standalone` and the scratch
  `xdg-v2` tree, and once with no `--standalone`, through the operator's
  background service and the operator's own state.

The prompt was on stdin. `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` were removed and
`OPENCODE_CONFIG_CONTENT` held a `permission.bash` deny map, as in the adapter.
The state plugin was not loaded: it does not load on 2.x (`state-and-reads.md`,
Q7). Each run was in a fresh clone of FinCal. Two prompts:

- plan-shaped: read `CONTEXT.md`, write `.ralphy/plan.md`, print `RALPHY_DONE_EXIT`;
- execute-shaped: write `SPIKE.md`, `git add` and `git commit` it, run `git log`,
  print `RALPHY_DONE_EXIT`.

| Fixture                        | Host    | Version | Server                 | Exit | Commit made | Log                                 |
| ------------------------------ | ------- | ------- | ---------------------- | ---- | ----------- | ----------------------------------- |
| `v1/plan-win.jsonl`            | Windows | 1.19.0  | in the process         | 0    | no (plan)   | `stream-plan-win-v1.log`            |
| `v1/execute-win.jsonl`         | Windows | 1.19.0  | in the process         | 0    | yes         | `stream-execute-win-v1.log`         |
| `v1/execute-wsl.jsonl`         | WSL     | 1.19.0  | in the process         | 0    | yes         | `stream-execute-wsl-v1.log`         |
| `v2/plan-win.jsonl`            | Windows | 2.3.2   | `--standalone`         | 0    | no (plan)   | `stream-plan-win-v2.log`            |
| `v2/execute-win.jsonl`         | Windows | 2.3.2   | `--standalone`         | 0    | yes         | `stream-execute-win-v2.log`         |
| `v2/execute-wsl.jsonl`         | WSL     | 2.3.2   | `--standalone`         | 0    | yes         | `stream-execute-wsl-v2.log`         |
| `v2/execute-service-win.jsonl` | Windows | 2.3.2   | the background service | 0    | yes         | `stream-execute-service-win-v2.log` |
| `v2/execute-service-wsl.jsonl` | WSL     | 2.3.2   | the background service | 0    | yes         | `stream-execute-service-wsl-v2.log` |
| `v1/auth-bad-key-win.jsonl`    | Windows | 1.19.0  | in the process         | 1    | -           | `stream-auth-bad-key-win-v1.log`    |
| `v1/no-login-win.jsonl`        | Windows | 1.19.0  | in the process         | 1    | -           | `stream-no-login-win-v1.log`        |
| `v2/auth-bad-key-win.jsonl`    | Windows | 2.3.2   | `--standalone`         | 1    | -           | `stream-auth-bad-key-win-v2.log`    |
| `v2/no-login-win.jsonl`        | Windows | 2.3.2   | `--standalone`         | 1    | -           | `stream-no-login-win-v2.log`        |

The plan-shaped and the execute-shaped runs have the same event types and the
same fields, in both versions 🔬. So one fixture of each shape is enough for the
tests; the shape of the prompt does not change the stream.

## Q3: the event stream of a full run

### Event table

One row per event or field that `events.rs` or `usage.rs` reads, then the event
types that only 2.x writes. "Read by" names the function of the adapter.

| Event or field               | Read by                                                    | 1.19.0                                                                                                            | 2.3.2                                                                                                                                                           | Marker |
| ---------------------------- | ---------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| envelope                     | `event_payload`                                            | `{type, timestamp, sessionID, part:{...}}`, `timestamp` in milliseconds                                           | the same                                                                                                                                                        | 🔬     |
| `step_start`                 | nothing                                                    | `part:{id, messageID, sessionID, snapshot, type:"step-start"}`                                                    | the same keys                                                                                                                                                   | 🔬     |
| `step_finish`                | nothing (tests only)                                       | `part:{id, messageID, sessionID, reason, snapshot, cost, tokens, type:"step-finish"}`                             | the same keys; `reason` is `tool-calls` between tools and `stop` at the end, as in 1.x                                                                          | 🔬     |
| token fields                 | `usage.rs` reads the database, not the stream              | `part.tokens` = `{total, input, output, reasoning, cache:{read, write}}`                                          | `{input, output, reasoning, cache:{read, write}}`: no `total`                                                                                                   | 🔬     |
| `tool_use`                   | nothing                                                    | `part:{id, callID, tool, state:{status, input, output, title, metadata, time}}`; tools `read`, `write`, `bash`    | `part:{partID, id, tool, state:{...}}`: `id` is the call id, `partID` the part id, no `callID`; the shell tool is `shell`; `write` takes `path`, not `filePath` | 🔬     |
| `text`                       | `text_part` (the sentinel scan)                            | `part:{type:"text", text, time}`                                                                                  | the same; `RALPHY_DONE_EXIT` arrives in a `text` event before the last `step_finish`                                                                            | 🔬     |
| `error`                      | `is_error_event`, `parse_opencode_limit`, the auth matcher | `{type:"error", sessionID, error:{name, data:{message, statusCode, isRetryable, responseHeaders, responseBody}}}` | `{type:"error", sessionID, error:{type, message, status?, response?:{body}, retryAfterMs?}}`: no `name`, no `data`, `status` and not `statusCode`               | 🔬 🔎  |
| session id                   | `session_id_from_stream`                                   | top-level `sessionID` on every event, `ses_...`                                                                   | the same, also on the error event                                                                                                                               | 🔬     |
| model id                     | `usage.rs` (`modelID` from the database)                   | not in the stream                                                                                                 | not in the stream                                                                                                                                               | 🔬     |
| `retry` (2.x only)           | nothing; `is_error_event` sees it as an error              | not seen in the 1.x runs                                                                                          | `{type:"retry", sessionID, messageID, attempt, nextRetryAt, error:{type, message, ...}}`, one per retry of a failed provider call                               | 🔎     |
| `reasoning` (2.x only)       | nothing                                                    | not seen in the 1.x runs                                                                                          | written only with `--thinking`, which Ralphy does not pass                                                                                                      | 🔎     |
| `background_wait` (2.x only) | nothing                                                    | not seen in the 1.x runs                                                                                          | `{type:"background_wait", jobs:[{id, kind, title}]}` while background jobs run                                                                                  | 🔎     |

The 🔎 rows are from the `run` code in the 2.3.2 binary: the event writer writes
`{type, timestamp, sessionID, ...}` for `step_start`, `step_finish`, `tool_use`,
`text`, `reasoning`, `retry`, `background_wait` and `error`, and nothing else
(`stream-strings-win-v2.log`). The 🔬 rows are in the fixtures.

What the parser of today does with the 2.x stream, by the rules of `events.rs`
at this commit:

- A clean 2.x run reads as a clean 1.x run: the text and the sentinel are found,
  no error is seen, and the session id is found. The same holds for a run
  through the background service. Every fixture of a clean run gives
  `done`, no error and a session id.
- `is_error_event` is true for any event that has an `error` key. A 2.x `retry`
  event has one, so a run that retries once and then ends well is read as a
  run with an error (`Stuck`), and its message is passed to the limit matcher.
- The token fields are in the stream of both versions. `usage.rs` does not read
  them: it reads the `message` table, which a 2.x database does not have
  (`state-and-reads.md`), so the 2.x usage is 0.

**Q3 answer:** The 2.x `--format json` stream keeps the 1.x envelope and the
`step_start`, `step_finish`, `text` and `tool_use` events, with the same
`sessionID` on every event, so the text, sentinel and session-id reads work
unchanged. The changes are in the `error` event (a typed `error.type` and
`status`, no `name` and no `data`), in the token object (no `total`), in tool
fields (`id` for the call id, `shell` for `bash`), and in a new `retry` event
that today's parser counts as an error.

**Changes:**

- #1009: read the 2.x `error` event by `error.type`, `error.message` and
  `error.status`; keep the 1.x reader of `error.name` and `error.data`. The
  version decides which reader runs.
- #1009: a `retry` event is not an error and not a limit. Count it as a degraded
  line (the run goes on), and take the outcome only from the last `error` event
  or from the exit.
- #1009: for 2.x, sum the token fields of the `step_finish` events of the stream,
  because the database reader finds no `message` table. Add `reasoning` the way
  the 1.x reader does; there is no `total` to check against.
- #1009: the tests read the fixtures of this stage: `v1/*.jsonl` and
  `v2/*.jsonl`, one clean run of each shape, the service runs and the error runs.

## Q4: completion, auth error and usage limit

### Completion

Every clean run, in both versions, on both hosts, with and without the
background service, ended with exit code 0. Its last event is a `step_finish`
with `part.reason` `stop`, and the `text` event with `RALPHY_DONE_EXIT` comes just
before it 🔬 (all clean-run fixtures). There is no other end-of-run event. 2.x
sets exit code 1 when the session ends in a failure: the `run` code writes the
error as an `error` event and sets `process.exitCode = 1`; it also writes
`{type:"error", error:{type:"unknown", message}}` for a failure of the command
itself 🔎 (`stream-strings-win-v2.log`). The 1.x stderr at `--log-level ERROR`
was empty. The 2.x stderr with `--standalone` held 59 to 69 `level=INFO` lines
(the file watcher) and no error; through the background service it was empty 🔬.

**Q4 completion:** both versions exit 0 and end with a `step_finish` whose
`part.reason` is `stop`, after the `text` event that carries the sentinel; 2.x
exits 1 after an `error` event when the session fails (🔬 fixtures, 🔎 binary).

### Auth error

Two ways to fail the login were run on Windows with an empty scratch `XDG_*` tree
and the paid model `anthropic/claude-haiku-4-5`:

| Case                                    | Version | stdout                                                                                                                                        | stderr (`--print-logs`)                                                                                                | Exit | Marker |
| --------------------------------------- | ------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------- | ---- | ------ |
| no login at all                         | 2.3.2   | one event: `{"type":"error",...,"error":{"type":"provider.no-route","message":"Model unavailable: anthropic/claude-haiku-4-5"}}`              | INFO lines, then `level=ERROR message="Failed to drain Session" cause="SessionRunnerModel.ModelUnavailableError: ..."` | 1    | 🔬     |
| a key the provider refuses (a fake key) | 2.3.2   | `step_start`, then `{"type":"error",...,"error":{"type":"provider.auth","message":"invalid x-api-key","status":401,"response":{"body":...}}}` | `level=ERROR message="Failed to drain Session" cause="AI.Error: invalid x-api-key ..."`                                | 1    | 🔬     |
| no login at all                         | 1.19.0  | `{"type":"error",...,"error":{"name":"UnknownError","data":{"message":"Unexpected server error. ...","ref":...}}}`                            | `ProviderModelNotFoundError: Model not found: anthropic/claude-haiku-4-5`                                              | 1    | 🔬     |
| a key the provider refuses (a fake key) | 1.19.0  | `{"type":"error",...,"error":{"name":"APIError","data":{"message":"invalid x-api-key","statusCode":401,...}}}`                                | `message="stream error" ... error.error="AI_APICallError: invalid x-api-key"`                                          | 1    | 🔬     |

- With no login, 2.x does not list the models of a provider that has no
  credential, so the run fails at model lookup with `provider.no-route`. It does
  not say "auth". A free model (`opencode/...-free`) needs no login and runs.
- `ProviderAuthError` is in none of these outputs. In the 2.3.2 binary it is only
  in the old (`SessionV1`) message schema and in the function that maps old error
  names to the new types (`ProviderAuthError` becomes `provider.auth`) 🔎. So
  `is_opencode_auth_error` does not fire on 2.x, and the operator gets a generic
  failure and not the "run `opencode auth login`" message.
- The 2.x classifier puts every 401 and every 403 into `provider.auth`, unless
  the body names a quota or billing error first 🔎 (`stream-strings-win-v2.log`,
  the `fp` function). A provider that answers a quota block with a plain 403
  (the Kimi billing-cycle limit of 1.x times) is read as an auth error by 2.x.
- The login of 2.x is in the database (`credential` table, `state-and-reads.md`);
  the auth command of 2.x was not run, to keep the operator's login as it is.

**Q4 auth error:** on 2.x a refused login is a stdout `error` event with
`error.type` `provider.auth` (and `status` 401), exit 1, and a stderr line
`message="Failed to drain Session" cause="AI.Error: ..."` with `--standalone`; a
missing login is `provider.no-route` ("Model unavailable"); `ProviderAuthError`
never appears, so today's matcher misses both (🔬 runs, 🔎 binary).

### Usage limit

No quota was spent to reach a limit. The signal is read from the 2.3.2 binary and
from two real limits in the operator's own state.

From the binary 🔎 (`stream-strings-win-v2.log`):

- 2.x classifies a provider error before it reaches the stream. A 402, a body
  with a known billing tag (`insufficient_quota`, `billing_error`,
  `FreeUsageLimitError`, `GoUsageLimitError`, `BudgetLimitError`, ...), or a 429
  whose text says `insufficient balance`, `insufficient quota`, `quota exceeded`,
  `usage limit` or `limit exhausted` is a quota error. A 429 or the words
  `rate limit` or `too many requests` without that wording is a rate limit.
- A quota error is not retried. The step fails and the stream gets one `error`
  event with `error.type` one of `provider.quota`, `provider.free-limit`,
  `provider.go-limit`, `provider.budget-limit`, `provider.model-budget-limit` or
  `provider.chatgpt-limit`, then the exit is 1.
- A rate limit is retried: the wait starts at 2 seconds and grows to 10 seconds,
  or follows the `retry-after` header of the provider, at most 15 minutes. Each
  retry writes a `retry` event with `error.type` `provider.rate-limit` and
  `nextRetryAt`. When the retries end, the last error is an `error` event
  `provider.rate-limit`.
- The error object carries `retryAfterMs`, and not `retryAfter`.

From the operator's state 🔬 (`stream-operator-limit-v2.log`; the log files were
read in place, the database through the copy of the state stage):

- One 2.x run on each host, on 2026-11-22, hit the Z.ai balance limit (code 1113,
  `zai-coding-plan`, glm-5.3). The Windows database row has
  `error: {type: "provider.quota", message: "Insufficient balance or no resource
  package. Please recharge.", status: 429, response: {body}}` and `finish:
  "error"`. The service log of each host has one line:
  `level=ERROR message="Failed to drain Session" cause="AI.Error: Insufficient balance or no resource package. Please recharge. ..."`.
- `usage_limit_regex` does not match "Insufficient balance". The error has no
  `name` and no `statusCode`. So `parse_opencode_limit` and
  `parse_opencode_log_limit` both return `None` for this real 2.x limit, and the
  run would end `Stuck` and not `Limit`.
- The 1.x form of a limit in the log was
  `message="stream error" ... error.error="AI_APICallError: You've reached your usage limit ..."`
  (Kimi, 2026-07-09). The string `"stream error"` is not in the 2.3.2 binary 🔎:
  the 2.x log line is `"Failed to drain Session"` with a `cause`.

Where the stderr line goes 🔬:

- With `--standalone`, `--print-logs` puts the server log on stderr, and the
  `Failed to drain Session` line of a failed step is there (seen for the auth
  error). A limit line would be there in the same form.
- Through the background service, stderr was empty in every run on both hosts.
  The service writes its log to `<data>/log/opencode.log`, which is the log of
  every client of that service. So the stderr scan and the stderr early kill see
  nothing on 2.x unless the run uses `--standalone`.
- On 2.x the stdout `error` event is enough: a quota error is terminal and typed.

**Q4 usage limit:** 2.x reports a usage limit as a stdout `error` event whose
`error.type` is `provider.quota`, `provider.free-limit`, `provider.go-limit`,
`provider.budget-limit`, `provider.model-budget-limit` or
`provider.chatgpt-limit` (marker 🔎 binary, 🔬 the operator's Z.ai row), and a
rate limit as `retry` events then `provider.rate-limit`; `--print-logs` puts the
`Failed to drain Session` line on stderr only with `--standalone`, and
`usage_limit_regex` misses the real Z.ai text.

**Q4 answer:** Completion is the same in both versions (exit 0, last
`step_finish` with `reason` `stop`). On 2.x the auth error and the usage limit
are both typed in the stdout `error` event (`provider.auth`, the
`provider.*-limit` and `provider.quota` types), with exit 1. Today's
`ProviderAuthError` matcher and the wording regex miss them, and the stderr log
scan is empty through the background service.

**Changes:**

- #1009: on 2.x, take the outcome from `error.type`: `provider.auth` is the auth
  error; `provider.quota`, `provider.free-limit`, `provider.go-limit`,
  `provider.budget-limit`, `provider.model-budget-limit`,
  `provider.chatgpt-limit` and a final `provider.rate-limit` are a usage limit;
  every other type is an error. Keep the wording regex only as a fallback.
- #1009: on 2.x, `provider.no-route` with "Model unavailable" for a paid
  provider is the usual sign of a missing login. Show a message that names both
  causes (no login, or a wrong model).
- #1009: do not rely on stderr for the limit on 2.x. Through the background
  service it is empty; with `--standalone` the line is `Failed to drain Session`
  and not `stream error`.
- #1009: a rate limit retries for up to 15 minutes per wait. The idle watchdog
  must not kill a run that writes `retry` events; check that a degraded line
  resets the idle clock.
- #1009: a fixture for a 2.x quota error is built from the operator's row
  (`provider.quota`, status 429, the 1113 text) with the 2.x error envelope;
  the error fixtures of this stage cover `provider.auth` and `provider.no-route`.
- New issue under #1011: 2.x `run` takes its folder from the `PWD` environment
  variable before the process folder 🔎 (`process.env.PWD ?? process.cwd()` in the
  `run` code) 🔬 (`stream-pwd-win.log`: with `PWD` set to another clone, the
  session opened there; 1.19.0 kept the process folder). Ralphy sets only the
  process folder (`current_dir(root)`) and passes its own `PWD` to the child, so
  a 2.x run can open in the wrong folder. Set `PWD` to the same folder for 2.x,
  and check it in the server stage for a run through the background service.

### Live-probe cost

| Probe                                          | Host         | Version       | Model                               | Tokens                                                    |
| ---------------------------------------------- | ------------ | ------------- | ----------------------------------- | --------------------------------------------------------- |
| plan-shaped full run                           | Windows      | 1.19.0        | `opencode/deepseek-v4.1-flash-free` | about 74 300 (cost 0)                                     |
| execute-shaped full run                        | Windows      | 1.19.0        | `opencode/deepseek-v4.1-flash-free` | about 97 600 (cost 0)                                     |
| execute-shaped full run                        | WSL          | 1.19.0        | `opencode/deepseek-v4.1-flash-free` | about 66 100 (cost 0)                                     |
| plan-shaped full run, `--standalone`           | Windows      | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | about 54 600 (cost 0)                                     |
| execute-shaped full run, `--standalone`        | Windows, WSL | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | about 65 200 and 59 200 (cost 0)                          |
| execute-shaped full run, background service    | Windows, WSL | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | about 64 500 and 59 600 (cost 0)                          |
| refused key and no login                       | Windows      | 1.19.0, 2.3.2 | `anthropic/claude-haiku-4-5`        | none (the provider refused the call, or no call was made) |
| `PWD` probe (`-m nosuch/nosuch`)               | Windows      | 1.19.0, 2.3.2 | none (stops at model lookup)        | none                                                      |
| binary strings, operator log and database copy | Windows, WSL | 2.3.2         | none                                | none                                                      |

The token counts are the sums of `input`, `output`, `reasoning` and the cache
fields of the `step_finish` events of each fixture.
