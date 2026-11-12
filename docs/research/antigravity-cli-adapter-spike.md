# Antigravity CLI (`agy`) — adapter spike

Evidence for a prospective Antigravity adapter, the replacement of the Gemini
adapter (#729, #782). It answers the C-questions of
[ADR-0040](../adr/0040-agent-adapter-onboarding-contract.md) on both target
platforms:

| Platform | Binary | Version |
| --- | --- | --- |
| Windows 11 Pro | `%LOCALAPPDATA%\agy\bin\agy.exe` | `1.4.9`, then `1.5.6` (it updated itself during the probes, §11) |
| WSL (Ubuntu) | `~/.local/bin/agy` | `1.5.6` |

This document records **observations**, not decisions; decisions belong in the
Antigravity adapter ADR. Every probe runs in a scratch git repo under a temp
directory, never in an operator project. Raw output is in
`docs/spike/live/antigravity-782-*.log`, with tokens, OAuth parameters and
emails replaced by `<redacted>`.

Session date: 2026-11-12.

## Evidence markers — read these before trusting any row

| Marker | Meaning |
| --- | --- |
| 🔬 | **Observed.** A command was run in this spike; its output is quoted or in a raw log. |
| 🔎 | **Read from the binary or its files.** Strings of `agy.exe`, the embedded changelog, the bundled skills under `builtin/`. Stronger than documentation, weaker than a run. |
| 📖 | **Documented only.** From `https://antigravity.google/docs/…`. Not exercised. |

### Live-probe cost

| Probe | Host | Model | Tokens |
| --- | --- | --- | --- |
| Logged-out probes (C5), `agy models` in temp roots, subcommand listings | Windows, WSL | none | 0 |
| Isolated-root check: one `-p` turn under `--gemini_dir=<tmp>` (C9, C10, C11) | Windows | `gemini-3.8-flash-low` | 8 475 in, 17 out, 0 thinking, 0 cache read (8 492 total) |
| `agy models`, unknown-model and `--effort` rejections, every `--print-timeout 1ms` probe (prompt channel, `--conversation`, NDJSON input, image block), `-p /quota`, `/usage`, `/credits`, `/model`, `/effort` (C1, C4, C6, C7, parity 14) | Windows | none (refused or cut before the model call; the store has no model record) | 0 |
| Plan probe, `--mode plan` (C1–C3) | Windows | `gemini-3.8-flash-low` | 18 352 in, 153 out |
| Execute probe, writes `hello.txt` and runs `git status` (C1–C3) | Windows | `gemini-3.8-flash-low` | 25 355 in, 161 out |
| No `--dangerously-skip-permissions`: a file write (C1) | Windows | `gemini-3.8-flash-low` | 16 729 in, 99 out |
| No `--dangerously-skip-permissions`: a shell command, denied (C1, C3) | Windows | `gemini-3.8-flash-low` | 8 300 in, 42 out |
| Timeout probe, `--print-timeout 30s` on a 90 s `ping` (C3, parity 9) | Windows | `gemini-3.8-flash-low` | 17 173 in, 95 out |
| Parity 13: read a gitignored `.ralphy/` file | Windows | `gemini-3.8-flash-low` | 16 766 in, 107 out |
| Parity 14: read a PNG by path | Windows | `gemini-3.8-flash-low` | 17 854 in, 52 out |
| Four slash-command probes lost to Git Bash path rewriting (`/quota` became `C:/Program Files/Git/quota`) and sent without `--model`, so the default model answered them as prompts | Windows | `gemini-4-argon-medium` (default) | 13 635 in, 504 out, 14 378 cache read; 8 295 in, 53 out; 8 287 in, 131 out; 13 731 in, 648 out (314 thinking), 14 386 cache read |

## 0. Executive summary

Filled by Stage 5.

## 1. C1 — Invocation and the headless contract

Raw logs: `docs/spike/live/antigravity-782-plan.log` (prompt channel, plan
probe), `docs/spike/live/antigravity-782-execute.log` (execute probe,
permissions). Host: Windows, agy 1.5.6, isolated root `--gemini_dir=<tmp>`,
stdin never a terminal. Every C1–C4, C6 and C7 probe ran on Windows: on WSL the
credential is a file inside the root (§5), so an isolated root there is logged
out, and a signed-in probe would write into the operator's real `~/.gemini`.

Which text became the prompt was read from the CLI log line
`Print mode: starting (promptLength=N, …)` with `--print-timeout 1ms`, which
cuts the turn before any model call (0 tokens).

| Invocation (stdin = `stdin1.txt`, 27 bytes) | Exit | What happened 🔬 |
| --- | --- | --- |
| `agy -p --model …` | 2 | `Error: -p took "--model" as its prompt, …` |
| `agy … -p` (last) | 2 | `flag needs an argument: -p` |
| `agy … -p ""` | 1 | `Error: empty prompt. Usage: agy --print "your prompt here"`, `promptLength=0`: stdin not read |
| `agy … -p -` | 3 | `promptLength=1`: `-` is a literal prompt |
| `agy … -p ARG` | 3 | `promptLength=3`: stdin ignored, not appended |
| `agy …` (no `-p`) | 3 | `promptLength=27`: **piped stdin alone starts print mode and is the whole prompt** |

- 🔬 **The prompt goes on stdin.** With no `-p` and a piped stdin, agy runs
  headless with stdin as the prompt. 72 025 bytes of UTF-8 (`ação ✓ 日本` on
  every line) arrived whole (`promptLength=72025`), so the ~32 KB argv limit
  of Windows is avoided. 🔬 There is no grace timer: stdin written 3 s after
  the spawn was still read (`promptLength=26`); the CLI waits for EOF, so
  Ralphy writes the prompt and closes stdin.
- 🔬 Never pass `-p` together with a stdin prompt: `-p <text>` wins and stdin
  is dropped silently.
- 🔬 A second stdin channel, `--input-format stream-json` (needs
  `--output-format stream-json`), takes one NDJSON line per turn:
  `{"event":"user","message":{"role":"user","content":"…"}}`. A bare string
  for `message` is refused (exit 1, `cannot unmarshal string into Go struct
  field StreamInputMessage.message`). Content blocks other than `text` are
  refused (parity 14).
- 🔬 **Full autonomy is `--dangerously-skip-permissions`** (the init event
  then says `"permission_mode":"always-proceed"`). Without it the mode is
  `request-review`: a file write in the workspace still ran, but a shell
  command was **auto-denied without a prompt**, and the run still exited 0 with
  `"status":"SUCCESS"`, an empty `response`, and
  `"denied_actions":[{"action":"command","display_name":"RunCommand"}]`;
  stderr: `jetski: no output produced — a tool required the "command"
  permission that headless mode cannot prompt for, so it was auto-denied. …
  re-run with --dangerously-skip-permissions to auto-approve all tools.` So a
  missing flag does not hang; it produces an empty, successful-looking run.
  📖 `--mode accept-edits|auto|plan` and `--sandbox` exist (`agy --help`); only
  `--mode plan` was run.
- 🔬 **Working directory:** there is no `--cwd` flag; the spawned process's cwd
  is the workspace (`init.cwd` equals it, and tool paths resolve under it).
  `--add-dir` adds more roots (📖 `--help`).
- 🔬 **No PTY is needed for the plan's quota:** headless runs are billed to the
  same quota buckets as the console (`-p /quota` showed the Gemini 5-hour
  bucket at 99% after the Flash probes, C7).
- 🔬 Wall time is 3–7 s more than the JSON `duration_seconds` (plan probe: 13.2
  s against 6.5 s; execute: 13.7 s against 9.8 s): sign-in, `loadCodeAssist`
  and `fetchAvailableModels` run before the turn. The 63 s Flash run of §9 was
  not reproduced: the six other Flash runs here took 9–16 s.

**C1 answer:** Headless one-shot is print mode; the prompt goes on **stdin with no `-p` at all** (🔬 piped stdin alone starts print mode, 72 KB UTF-8 arrived whole, no grace timer — write and close stdin), while `-p <text>` takes the next argument and silently drops stdin, and `-p` before another flag exits 2; full autonomy is `--dangerously-skip-permissions` (without it a shell command is auto-denied and the run still exits 0 with an empty response and `denied_actions`); there is no cwd flag, the spawned cwd is the workspace; no PTY is needed for plan billing.

## 2. C2 — The output stream

Raw logs: `antigravity-782-plan.log` §2, `antigravity-782-execute.log`.

Modes 📖 `--help`, 🔬 run: `text` (default, the response only), `json` (one
object at the end), `stream-json` (NDJSON). `stream-json` is the one Ralphy
needs: `text` gives no usage and no error state.

🔬 **`stream-json` discriminators** (`event`, then `step_type` and `state`):

| `event` | Content |
| --- | --- |
| `init` | first line: `conversation_id`, `init.model` (only when `--model` is given, and as written in argv), `init.cwd`, `init.tools`, `init.permission_mode`, `expanded_commands` |
| `step_update` | `step_index`, `state` (`ACTIVE`, `DONE`), `step_type`: `user_input`, `agent_response` (with `text_delta` while `ACTIVE`; `usage` and `duration_seconds` when `DONE`), `tool` (`tool_name`, `tool_info.parameters`, and on `DONE` an `output` summary such as `1 lines, 10 bytes` or the command output) |
| `result` | **the terminal envelope**: `status` (`SUCCESS`, `ERROR`), `response`, `error`, `duration_seconds`, `num_turns`, `usage`, `finished_reason` (omitted on a normal finish; `print_timeout`, `error`, 🔎 `interrupted`), `denied_actions` when a tool was auto-denied |

- 🔬 **The terminal envelope exists**: every run, including errors before the
  turn (unknown model, logged out, bad NDJSON input), ended with exactly one
  `result` line. A capture that ends without it is truncated.
- 🔬 **The final assistant message** is `result.response`; it equals the
  `text_delta` text of the last `agent_response` step, the one that has no
  following tool step. An earlier `agent_response` that only called a tool has
  no text. On a timeout, `response` holds the partial last message.
- 🔬 **No progress field**: no record counts files or lines changed. A
  `write_file` tool step carries only the path; a `run_command` step carries
  the command line and its output. Progress must come from the HEAD diff.
- 🔬 Text is UTF-8 on Windows (`text_delta` and `response` carried `ação ✓ 日本`
  in §10; 🔎 the changelog says a fix stopped `stream-json` deltas from turning
  non-ASCII into replacement characters).
- 🔬 stderr carries the human lines, not JSON, except one: `AGY_ERROR: {…}` (C3).

**C2 answer:** Use `--output-format stream-json`: NDJSON with `event` = `init` (conversation id, cwd, tools, permission mode, model only when `--model` is given), `step_update` (`step_type` `user_input` | `agent_response` | `tool`, `state` `ACTIVE` | `DONE`, text in `text_delta`, per-call `usage` on a `DONE` agent_response) and one terminal `result` envelope (`status`, `response`, `usage`, `finished_reason`, `denied_actions`) that every observed run ended with, errors included; the final message is `result.response` (the text of the last agent_response step); no stream field reports files changed, so progress comes from the HEAD diff (🔬).

## 3. C3 — Completion and the sentinel

Raw logs: `antigravity-782-plan.log`, `antigravity-782-execute.log`,
`antigravity-782-timeout.log`.

- 🔬 **The sentinel holds on Flash.** Every prompt that asked for
  `RALPHY_DONE_EXIT` as the last line got it as the last line of
  `result.response` (plan, execute, write, gitignored read, image: 5 of 5),
  followed by one `\n`. The run with a denied shell command ended with an
  empty `response`, so no sentinel: a missing autonomy flag reads as "not
  done", not as success.
- **Exit codes** observed 🔬 (and 🔎 from the embedded changelog):

| Exit | When | Marker |
| --- | --- | --- |
| 0 | the turn ended; **also** when a tool was auto-denied (empty response), and for `--input-format stream-json` when `--print-timeout` cut the turn (`status:"SUCCESS"`, `finished_reason:"print_timeout"`) | 🔬 |
| 1 | refused before the turn: unknown model, bad `--effort`, effort conflict, empty prompt, bad NDJSON input, logged out (§5) | 🔬 |
| 2 | argv usage error (`-p` swallowed a flag, `flag needs an argument: -p`, unknown flag) | 🔬 |
| 3 | `--print-timeout` expired (`finished_reason:"print_timeout"`); 🔎 also a turn that ends on a model or agent API failure | 🔬 / 🔎 |

- 🔬 **`AGY_ERROR` line.** On exit 3, stderr has one line
  `AGY_ERROR: {"short_error":…,"status":"DEADLINE_EXCEEDED","retryable":false,"reason":"print_timeout","tasks":[…]}`.
  🔎 The changelog says the same line carries the canonical status, the HTTP
  or gRPC code, retryability and an error id on an API failure. This is the
  structured error channel for the classifier.
- 🔬 **`--print-timeout` works and cleans up** (`antigravity-782-timeout.log`):
  with `30s` on a turn waiting on `ping -n 90`, the run exited 3 at 33 s,
  stderr said `terminating 1 background task(s) on exit`, and no `PING.EXE`
  was left. The default is `0s` (no limit).
- Hooks as a completion signal: the hook probes are in _Hooks and
  integrations_ (Stage 5). 🔎 The changelog names `Stop`, `PreInvocation`,
  `PostInvocation`, `PreToolUse`, `PostToolUse` and `SessionStart`.
- `CompletionSignals` (ADR-0023) from the stream 🔬: `done` = last line of
  `result.response` is the sentinel; `blocked` = the blocked sentinel in the
  same text; `timed_out` = `finished_reason:"print_timeout"` or exit 3 with
  `reason:"print_timeout"` (or Ralphy's own deadline); `exited_ok` = exit 0
  **and** `result.status:"SUCCESS"` **and** no `finished_reason` (exit 0 alone
  is not enough, see the NDJSON-input row); `errored` = `status:"ERROR"`, an
  `AGY_ERROR` line, or a non-empty `denied_actions`; `limit` = C7;
  `committed` = Ralphy's HEAD check, as for every vendor.

**C3 answer:** The `RALPHY_DONE_EXIT` last line held in 5 of 5 Flash runs (🔬); exit codes are 0 (turn ended — also with an auto-denied tool and, under `--input-format stream-json`, with a print timeout), 1 (refused before the turn: model, effort, empty prompt, logged out), 2 (argv usage) and 3 (`--print-timeout` expired, 🔬; model or agent API failure, 🔎), and exit 3 comes with a JSON `AGY_ERROR:` line on stderr; `--print-timeout` ends the turn, kills its background tasks and exits 3 (🔬), so `exited_ok` must read `result.status` and `finished_reason`, not the exit code alone; hooks are probed in _Hooks and integrations_.

## 4. C4 — Models

Raw log: `docs/spike/live/antigravity-782-models.log`.

🔬 `agy models` is free and deterministic (no model call; the progress line
goes to stderr). On Windows it listed 13 ids; Stage 3 saw the same 13 on WSL.
`agy models --output-format json` does not exist in 1.5.6 (exit 1). The
provider below comes from the quota groups of `-p /quota --output-format json`
(C7): _"Models within this group: Gemini Flash, Gemini Pro"_ and _"Claude
Opus, Claude Sonnet, GPT-OSS"_.

| Id (`agy models`) | Label | Provider | Effort variants of the base id | Store's backend id |
| --- | --- | --- | --- | --- |
| `gemini-4-argon-low`, `-medium`, `-high` | Gemini 4 Argon | Google | low, medium, high | `gemini-4-argon-trial` (medium) |
| `gemini-3.8-flash-low`, `-medium`, `-high` | Gemini 3.8 Flash | Google | low, medium, high | `gemini-3.8-flash-n` (low) |
| `claude-opus-5-5-low`, `-medium`, `-high` | Claude Opus 5.5 | Anthropic | low, medium, high | not observed |
| `claude-sonnet-5-5-low`, `-medium`, `-high` | Claude Sonnet 5.5 | Anthropic | low, medium, high | not observed |
| `gpt-oss-120b-medium` | GPT-OSS 120B | OpenAI (open-weight model) | medium only | not observed |

- 🔬 **Multi-model, multi-provider.** The **default** with no `--model` is
  `Gemini 4 Argon (Medium)` (CLI log `Propagating selected model override …
  label="Gemini 4 Argon (Medium)"`). No router call was seen: each run used one
  model for every call (store, C6).
- 🔬 **The stream does not name the model that answered.** `init.model`
  appears only when `--model` is given, and it echoes argv (`gemini-3.8-flash`
  for a base id the CLI resolved to `gemini-3.8-flash-low`). The store names
  the model per call (C6), by label and backend id.
- 🔬 **Unknown model: refused before any model call**, exit 1:
  `error: invalid model selection (--model "no-such-model" --effort ""): model
  no-such-model is not recognized as a known model or custom model in
  settings` followed by `Available models:` and the 13 labels. The same JSON
  `error` is on stdout.
- 🔬 **Effort**: `--effort` takes `low|medium|high|xhigh|max` and is the same
  axis as the id suffix. A base id is an alias: `--model gemini-3.8-flash`
  resolves to `-medium` (the model's default), `--model gemini-3.8-flash
  --effort low` to `-low`. A suffixed id with a different `--effort` is
  refused (`--model gemini-3.8-flash-low conflicts with --effort=max`); the
  same effort is accepted. An effort the model lacks is refused with its list
  (`gpt-oss-120b has no "high" effort (available: medium)`;
  `claude-opus-5-5 has no "max" effort (available: low, medium, high)`). No
  model in this listing offers `xhigh` or `max`. All these refusals cost 0
  tokens.
- 🔬 `-p /model --output-format json` returns the current selection for free:
  `{"id":"gemini-3.8-flash-low","label":"Gemini 3.8 Flash (Low)","effort":"low","is_default":false}`.
- **Entitlement**: probed at one tier only (the operator's plan). The quota
  output says the Argon bucket differs by plan (_"For paid Pro plans, Argon
  quota is separate … Ultra plans"_) and the backend id is
  `gemini-4-argon-trial`, so the list is plan-scoped; `model` must stay
  `Option<String>`, omitted when `None`.
- 🔬 **Price table**: none of the 13 ids, and none of their base names, has a
  row in `crates/ralphy-pricing/src/floor.rs` at this commit. The price key
  needs the effort suffix removed (`-low|-medium|-high`) and rows for
  `gemini-4-argon`, `gemini-3.8-flash`, `claude-opus-5-5`, `claude-sonnet-5-5`
  and `gpt-oss-120b`.

**C4 answer:** Multi-model, multi-provider: `agy models` (free, deterministic) lists 13 ids — `gemini-4-argon-{low,medium,high}` and `gemini-3.8-flash-{low,medium,high}` (Google), `claude-opus-5-5-{low,medium,high}` and `claude-sonnet-5-5-{low,medium,high}` (Anthropic), `gpt-oss-120b-medium` (OpenAI open-weight) — and the default is `gemini-4-argon-medium`; the id suffix is the effort, `--effort low|medium|high|xhigh|max` selects it from a base id (a base id alone takes the model's default, medium here) and a conflicting pair is refused; an unknown model or effort is refused before any model call with exit 1 and `model <id> is not recognized as a known model or custom model in settings` plus the available list (🔬); the stream names the model only as argv gives it, so the answering model comes from the store (C6); no id is in the price table yet.

## 5. C5 — Authentication

Raw log: `docs/spike/live/antigravity-782-logged-out.log`.

### There is no login command 🔬

`agy --help` lists no `login` or `logout` subcommand. Sign-in happens on the
first interactive launch: 🔬 `agy models` in a logged-out root says _"Please
sign in to view available models. Launch the CLI without arguments to sign
in."_ Inside the console, `/login` and `/logout` exist (🔎 changelog: _"run
`/logout` and `/login`"_; 📖 `/logout` "purge[s] authentication tokens from the
secure keyring"). The command for the adapter's auth error message is
therefore **`agy`** (launch it with no arguments and sign in).

### 🔬 The logged-out signature (WSL, agy 1.5.6, `HOME=<tmp>`)

| Command (stdin `/dev/null`, own session) | Exit | stdout | stderr |
| --- | --- | --- | --- |
| `agy models` | 1 | empty | `Fetching available models...` then `Error: Please sign in to view available models. Launch the CLI without arguments to sign in.` |
| `agy -p "Reply with OK" --print-timeout 60s` | 1 | empty | `Authentication required. Please visit the URL to log in:` + OAuth URL, `Waiting for authentication (timeout 60s)...`, `Or, paste the authorization code here and press Enter:`, then `Error: authentication timed out.` and `error: authentication failed or timed out` |
| same, `--output-format json` | 1 | `{"conversation_id":"","status":"ERROR","response":"","error":"authentication failed or timed out",…,"finished_reason":"error"}` | as above |
| same, `--output-format stream-json` | 1 | one line `{"event":"result","result":{…"status":"ERROR",…"error":"authentication failed or timed out"…}}` | as above |

Three traps for the detector:

- 🔬 **A logged-out `-p` blocks for 60 s before it exits.** The wait is fixed:
  with `--print-timeout 5s` the run still waited the full 60 s, and with no
  `--print-timeout` it was still waiting when the outer timeout ended it at
  30 s (stderr then reads `Error: authentication interrupted.`). The first
  stderr line, `Authentication required. Please visit the URL to log in:`,
  arrives at once, so the adapter can match it and stop the child early.
- 🔬 **With a terminal on stdin, the wait reads the terminal.** A first attempt
  without its own session left `agy` stopped (`Tl`, SIGTTIN/SIGTTOU) inside
  `timeout`. Ralphy must spawn with stdin not a terminal.
- 🔬 The JSON error object has `status:"ERROR"` and the same `error` text as
  stderr, so the stdout envelope alone also identifies the state.

### Windows: the logged-out state is not reachable without a logout 🔬

A temp `USERPROFILE`/`HOME` moves the whole state root (🔬 the temp root's
`cli.log`: `CLI app data directory: …\agyprobe\winhome\.gemini\antigravity-cli`),
but the run was **still signed in**: `keyringAuth: loaded token` and
`ChainedAuth: authenticated via keyring (effective: keyring)`, and `agy models`
listed 13 ids with exit 0. The credential is in **Windows Credential Manager**,
🔬 target `LegacyGeneric:target=gemini:antigravity` (`cmdkey /list`), a fixed
name that no variable or flag of agy 1.5.6 changes (🔎 the binary's
`shouldBypassKeyring` is the Linux D-Bus check). The hidden `--gemini_dir`
flag (§11) is isolated in the same way and also authenticates from the keyring.
So the Windows logged-out probe would cost the operator's session; it was not
run. 🔎 The four stderr strings above are each present in `agy.exe` 1.5.6
(`strings`: `Authentication required. Please visit the URL to log in`,
`Waiting for authentication (timeout`, `authentication failed or timed out`,
`Please sign in to view available models`), so the Windows signature is
expected to match WSL. The exit code on Windows is not observed.

### Where the credential lives

- 🔬 Windows: Credential Manager, target `gemini:antigravity`. The real state
  root has no token file.
- 🔬 WSL: `~/.gemini/antigravity-cli/antigravity-oauth-token` (mode 0600), even
  though a D-Bus session bus exists. A temp `HOME` is therefore logged out.
- 🔎 Linux in general: the OS keyring when a D-Bus session bus answers, else
  the file (changelog: _"bypasses the keyring when no D-Bus session bus is
  present"_). macOS: the keychain (📖, not probed).

### Environment variables that change auth

- 📖 `GEMINI_API_KEY`, only with `"modelProvider": "gemini"` in
  `~/.gemini/antigravity-cli/settings.json` (_"Setting the variable alone has
  no effect"_). This is the variable the Gemini adapter already handles: the
  adapter must decide whether it passes through.
- 📖 `AGY_ADC_AUTH=true` selects Application Default Credentials;
  `GOOGLE_CLOUD_QUOTA_PROJECT`, `GOOGLE_CLOUD_LOCATION` refine it. 🔎 _"To log
  out, please run `unset AGY_ADC_AUTH`"_.
- 🔎 `GOOGLE_GEMINI_BASE_URL`, `GOOGLE_API_KEY`, `GOOGLE_GENAI_USE_VERTEXAI`
  appear in the binary.

**C5 answer:** Login is interactive only: the operator runs `agy` with no arguments and signs in (no `login` subcommand). WSL logged out (🔬, temp `HOME`): `agy -p` exits **1** after a fixed 60 s wait, stderr starts with `Authentication required. Please visit the URL to log in:` and ends with `error: authentication failed or timed out`, and JSON output has `"status":"ERROR"` with that error; `agy models` exits 1 with `Error: Please sign in to view available models.`. Windows logged out: not observable without the operator's `/logout`, because the credential is in Windows Credential Manager (target `gemini:antigravity`), which a temp `USERPROFILE` or `--gemini_dir` does not isolate (`agy models` in a temp root still signed in, exit 0); the same four stderr strings are in `agy.exe` 1.5.6 (🔎), so the signature is expected to match WSL, but its exit code is not observed.

## 6. C6 — Usage and the session store

Raw log: `docs/spike/live/antigravity-782-store.log`.

### Where the counts live

| Source | Per what | Fields | Model | Marker |
| --- | --- | --- | --- | --- |
| `stream-json` `step_update` (`agent_response`, `DONE`) | one model call | `input_tokens`, `output_tokens`, `thinking_tokens`, `cache_read_tokens`, `total_tokens` | no | 🔬 |
| `stream-json` / `json` `result.usage` | the whole run | the same five | no | 🔬 |
| `<root>/brain/<id>/.system_generated/logs/transcript_full.jsonl` (`PLANNER_RESPONSE` lines) | one model call | `input_tokens`, `output_tokens`, `cache_read_tokens` (no thinking) | no | 🔬 |
| `<root>/conversations/<id>.db` (SQLite), table `gen_metadata`, protobuf `data` | one model call | input, output, cache read, thinking, visible output | backend id (`gemini-3.8-flash-n`) and label (`Gemini 3.8 Flash (Low)`) | 🔬 decoded without a schema |
| `<root>/conversation_summaries.db` (SQLite) | one conversation | title, preview, step count, workspace, status; no tokens | no | 🔬 |

`<root>` is `~/.gemini/antigravity-cli` (or the `--gemini_dir` root, §11).

- 🔬 **Per-call records are incremental: sum them.** Execute probe: steps
  8 325 + 8 451 + 8 579 = 25 355 input and 50 + 38 + 73 = 161 output, which is
  exactly `result.usage` (25 355 / 161); the transcript and the SQLite rows
  hold the same per-call numbers. Each call's `input_tokens` is that call's
  own prompt, not a running total. A test for the adapter: three step records
  and a `result` whose input is their sum; keep-last gives 8 579 and fails.
- 🔬 **The four counts.** `input_tokens` is the uncached input: in the Argon
  run, call 2 had 3 315 input and 6 239 cache read, and `result` had
  `input_tokens` 13 731 with `cache_read_tokens` 14 386, larger than the input.
  So cache read is separate from input. `output_tokens` **includes thinking**:
  the SQLite rows split output into thinking + visible (132 + 64 = 196,
  11 + 66 = 77), and `total_tokens` = input + output (13 731 + 648 = 14 379),
  with `thinking_tokens` 314 inside the 648. **There is no cache-creation
  count** in the stream, the transcript or the SQLite rows read.
- 🔬 **Model attribution:** per call only in the SQLite `gen_metadata` rows
  (backend id and label; the label maps to an `agy models` id one to one).
  The stream gives no model when `--model` is omitted (C4).
- 🔬 **The session id cannot be chosen before the spawn.** `--conversation`
  with an unknown id (a UUID or `ralphy-run-42`) prints `warning: conversation
  "…" not found` and starts a new conversation with a CLI-made UUID. But the
  id is the **first stdout line** (`init.conversation_id`) and is repeated in
  `result`, so the lookup is a direct key read from the stream, with no
  snapshot-diff. 🔬 `<root>/cache/last_conversations.json` maps the workspace
  path to the latest id, and is overwritten by every run.
- 🔬 **Billing unit is a quota share, not tokens.** `-p /quota --output-format
  json` says _"Quota is consumed proportionally to the cost of the tokens"_,
  per group, as a `remaining_fraction` of a 5-hour and a weekly bucket (C7).
  🔎 `/credits` reports a separate credit balance (`Remaining credits 0`) that
  is used when the plan quota is spent. Tokens drive `Usage`; the quota share
  is a separate reading.
- 🔬 **For `ralphy-usage-scan`**: the store is local and readable. A scan can
  walk `<root>/brain/*/.system_generated/logs/transcript_full.jsonl` (JSON,
  no thinking, no model) or open `<root>/conversations/*.db` read-only
  (protobuf, with model and thinking). A run cut before the model call still
  leaves a conversation with one `USER_INPUT` step and no usage (14 of 25
  conversations in the probe root), so the scan must accept a conversation
  with no usage. Subagent conversations: 🔎 `conversation_parents` exists in
  `conversation_summaries.db`; no subagent ran here.

**C6 answer:** Usage is per model call and incremental — sum the `stream-json` `step_update` usage, or read `result.usage`, which is that sum (🔬 8 325 + 8 451 + 8 579 = 25 355); `input_tokens` excludes cache reads (`cache_read_tokens` is separate and can exceed it), `output_tokens` includes `thinking_tokens`, `total_tokens` = input + output, and **no cache-creation count exists** anywhere; the same per-call counts are on disk in `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript_full.jsonl` (no thinking, no model) and in the SQLite `conversations/<id>.db` `gen_metadata` protobuf (with the model's label and backend id, the only per-call model record); the session id cannot be minted (`--conversation <unknown>` warns and starts a new id) but is the first stream line (`init.conversation_id`), so no snapshot-diff is needed; the plan bills a quota share per model group, not tokens.

## 7. C7 — Limits

Raw log: `docs/spike/live/antigravity-782-limit.log`.

**No limit was reached.** The authorized budget was Flash calls, and the
operator's buckets stayed at 98–100% (a 5-hour bucket would need hours of
Flash runs). What follows is the free quota read-out (🔬) and the limit signals
from the binary (🔎).

### 🔬 The quota is readable for free, with reset times

`agy -p /quota` (also `/usage`) answers without a model call (0 tokens, no
conversation), one tab-separated record per bucket (tabs shown as two spaces):

```text
Gemini Models  Weekly Limit Remaining  100%  2026-11-19T11:32:40Z
Gemini Models  Five Hour Limit Remaining  99%  2026-11-12T16:32:40Z
Claude and GPT models  Weekly Limit Remaining  100%  2026-11-19T12:13:38Z
Claude and GPT models  Five Hour Limit Remaining  100%  2026-11-12T17:13:38Z
Gemini 4 Argon  Gemini 4 Argon Weekly Limit  98%  2026-11-19T12:11:52Z
```

With `--output-format json`, `command.data.groups[].buckets[]` carries `id`
(`gemini-5h`, `gemini-weekly`, `3p-5h`, `3p-weekly`,
`gemini-argon-wtu-weekly`), `window` (`5h`, `weekly`), `remaining_fraction`
and `reset_time` (UTC). Every print run also calls
`v1internal:retrieveUserQuotaSummary` (CLI log). The weekly buckets of
`Claude and GPT models` moved their `reset_time` between three reads a few seconds apart
(12:13:38Z, 12:13:43Z, 12:13:52Z) while unused at 100%, so a reset time of an
unused bucket means nothing. Under Git Bash, `/quota` must be passed with
`MSYS_NO_PATHCONV=1`; Ralphy spawns directly and is not affected.

### 🔎 How a limit would surface (agy.exe 1.5.6 strings and changelog)

- A turn that ends on a model API failure exits **3** with an `AGY_ERROR:`
  JSON line (canonical status, HTTP or gRPC code, `retryable`, error id).
- The CLI retries per-minute `429` and `5xx` itself, honours the server's
  retry delay, and **stops at once** when the delay is over 30 s or the quota
  is a daily or billing cap; "reaching your individual quota is reported right
  away instead of being retried".
- Message strings: `You have exhausted your quota on this model.`,
  `individual quota reached`, `Your AI credits balance is too low to
  continue`, `MODEL_CAPACITY_EXHAUSTED`, `RESOURCE_EXHAUSTED`,
  `Too Many Requests`, `Usage limit reached`. A bundled script matches
  `/RESOURCE_EXHAUSTED|quota|\b429\b|exhausted your capacity|rate.?limit/i`.
- Credits: a `UseG1Credits` setting spends G1 credits when the plan quota runs
  out (the operator has 0). No spend-cap flag exists in `agy --help`.

For the adapter: match a limit class over the `AGY_ERROR` text and stderr
(`RESOURCE_EXHAUSTED | quota | 429 | rate.?limit | exhausted | credits balance
is too low`); for the reset, read `-p /quota --output-format json` after the
failure and take the `reset_time` of the exhausted bucket (the one with
`remaining_fraction` 0 in the model's group). If no bucket is at 0, emit
`Limit(None)`.

**C7 answer:** Not observed — no limit was reached at the authorized cost (Flash only; buckets stayed at 98–100%); 🔎 a limit is expected as exit 3 with a structured `AGY_ERROR:` stderr line (status, HTTP/gRPC code, `retryable`) and strings such as `You have exhausted your quota on this model.`, `individual quota reached`, `RESOURCE_EXHAUSTED`, `Your AI credits balance is too low to continue`, with per-minute 429s retried inside the CLI and daily or billing caps reported at once; 🔬 a schedulable reset hint is available for free from `agy -p /quota --output-format json` (per-group 5-hour and weekly buckets with `remaining_fraction` and UTC `reset_time`); no spend-cap flag exists.

## 8. C8 — Skills and prompts

Filled by Stage 4 or 5.

**C8 answer:** _Stage 4/5_

## 9. C9 — Blast radius and the product ethos

Raw log: `docs/spike/live/antigravity-782-state.log`.

- **Bundled MCP servers** 🔬: none. `agy mcp list` in a fresh root prints `No
  MCP servers configured.`, and `config/mcp_config.json` is empty.
- **Bundled plugins and skills** 🔬: `agy plugin list --json` prints `[]`, but
  the first run extracts `builtin/plugins/swarm` (with a `swarm_board`
  sidecar) and writes `{"plugins": {"swarm": {"enabled": true}}}` into
  `<root>/config/config.json`. The first run also extracts 13 built-in skills,
  among them `permissioned-github` (_"Guidelines for interacting with GitHub
  and request permissions from the user…"_), `automation` (scheduled
  background tasks), `native_swarm` and `run-workflow` (fan-out to many
  agents), and `migrate-agent-settings` (copies MCP servers, skills and
  instruction files from Claude Code, Codex, Cursor and Gemini CLI). These are
  the delegate and background verbs the agent could reach; whether a headless
  run loads them is C8 (Stage 4).
- **Remote control and export** 🔬: `agy remote-control status` says `Daemon
  status: not registered` on Windows; WSL has no `antigravity-cli-daemon`
  systemd user unit; Windows has no scheduled task or `Run` entry for agy.
  Every launch logs `[RemoteControl] RemoteControlEnabled value: false …
  staying disconnected`. It is opt-in (`--remote-control`,
  `agy remote-control start`); its stance belongs to _Hooks and integrations_.
- **Background processes** 🔬: every launch that is not `--version` spawns a
  detached `agy.exe --bg-updater --app_data_dir=antigravity-cli
  --gemini_dir=.gemini`, which outlives the run (one ran 3 minutes after its
  parent exited and replaced the binary, §11). After the Flash run, no `agy`
  process was left; on WSL no `agy` process was left after the logged-out
  runs.
- **Auto-update mid-run** 🔬: yes, and it cannot be turned off (§11).
- **Repo-local instruction files** 🔬: with `AGENTS.md`, `GEMINI.md` and
  `CLAUDE.md` in the scratch repo, each asking for its own marker word, the
  Flash reply was `ação ✓ 日本 MARKER-AGENTS-MD MARKER-GEMINI-MD`: **`AGENTS.md`
  and `GEMINI.md` are read, `CLAUDE.md` is not.** 🔎 The bundled
  `agy-customizations` skill names the other roots: `.agents/` (or `.agent/`,
  `_agents/`, `_agent/`) from the cwd up to the repo root, `.agents/rules/*.md`,
  `~/.gemini/config/`, and global `~/.gemini/GEMINI.md` / `~/.gemini/AGENTS.md`
  (changelog). No flag to turn them off was found; `--disable-slash-commands`
  covers slash commands and skill expansion only. A Ralphy repo's `AGENTS.md`
  will therefore compete with the charter.
- **Upload of the repository** 🔬: the one-turn run (no tool call) contacted
  only `loadCodeAssist` (4×), `fetchAvailableModels`, `streamGenerateContent`
  and `recordTrajectoryAnalytics` (telemetry) on
  `daily-cloudcode-pa.googleapis.com`. No indexing or upload endpoint
  appeared, and the instruction files went inside the prompt (8 475 input
  tokens for a one-line question). A run with tool calls, and an A/B on two
  fresh repos as ADR-0040 asks, are left to the live capstone.
- **Open-a-PR verbs**: no `agy` subcommand opens a PR; the reach is through the
  agent's shell and the `permissioned-github` skill.

**C9 answer:** No MCP server is bundled (🔬 `mcp list` empty), but every first run extracts 13 built-in skills (including `permissioned-github`, `automation`, `native_swarm`, `run-workflow`, `migrate-agent-settings`) and the `swarm` plugin, enabled in `config/config.json`; remote control is off and no daemon is registered on either host; every launch except `--version` leaves a detached `--bg-updater` process that can outlive the run by minutes and replaces the shared binary (no opt-out found); the agent reads the repo's `AGENTS.md` and `GEMINI.md` (not `CLAUDE.md`) with no flag to stop it; a one-turn headless run sent the instruction files in the prompt and contacted no indexing or upload endpoint, only model, quota and telemetry calls (a run with tool calls is not yet observed).

## 10. C10 — Cross-platform and I/O hygiene

Raw log: `docs/spike/live/antigravity-782-install.log`.

| OS | Install (📖 getting-started page) | Binary observed | Marker |
| --- | --- | --- | --- |
| Windows | `irm https://antigravity.google/cli/install.ps1 \| iex`, or the `install.cmd` route | `%LOCALAPPDATA%\agy\bin\agy.exe` (242 MB, one name only) | 🔬 |
| Linux | `curl -fsSL https://antigravity.google/cli/install.sh \| bash` | `~/.local/bin/agy` (ELF, 279 MB) on WSL | 🔬 |
| macOS | the same `install.sh` | `~/.local/bin/agy` | 📖 (no host) |

- 🔬 **One binary name per OS**: `agy` / `agy.exe`. `antigravity.cmd` is the
  IDE launcher (first facts, #729), not this CLI. The CLI also writes helper
  shims into its root: `bin/agentapi.bat` (Windows) and `bin/agentapi` (WSL);
  🔬 after the self-update the WSL shim still pointed at
  `~/.local/bin/agy.1793217806081684432.old`.
- 🔬 **Off `PATH` by default in a running process**: the Windows folder is on
  the user `PATH` only after a new shell (#729 first facts). `agy install`
  edits the shell profile (`--skip-path`, `--skip-aliases`). The adapter needs
  an explicit probe list: `%LOCALAPPDATA%\agy\bin\agy.exe` and
  `~/.local/bin/agy`.
- 📖 **CI**: no package manager names it (no Homebrew, npm or winget); CI would
  run the install script, then sign in without a browser with
  `GEMINI_API_KEY` + `"modelProvider": "gemini"` or `AGY_ADC_AUTH=true` with
  Application Default Credentials.
- 🔬 **Windows encoding**: stdout to a pipe is **UTF-8**, not the console code
  page: the reply `ação ✓ 日本` arrived as `a 303 247 303 243 o 342 234 223 346
  227 245 346 234 254`. 🔎 `NO_COLOR` is honoured by the plugin subcommands,
  and `AGY_CLI_OSC7501=off` turns off the terminal status escape sequence.
- 🔬 **Usage-error trap**: `-p` takes the next argument as its prompt, so
  `agy -p --print-timeout 60s` exits 2 with `-p took "--print-timeout" as its
  prompt`. Prompt delivery is C1 (Stage 4).

**C10 answer:** Windows installs one binary, `%LOCALAPPDATA%\agy\bin\agy.exe` (🔬; `install.ps1` or `install.cmd`, 📖), and Linux installs `~/.local/bin/agy` (🔬 on WSL; `install.sh`, 📖); macOS uses the same `install.sh` into `~/.local/bin/agy` (📖 only, no host); neither path is reliably on a running process's `PATH`, so the adapter needs an explicit probe list; CI has no package manager route and must run the install script and sign in with `GEMINI_API_KEY` plus `modelProvider: gemini` or with `AGY_ADC_AUTH` (📖); on Windows, stdout to a pipe is UTF-8 (🔬), and a terminal on stdin makes a logged-out run block on reading it (§5).

## 11. C11 — Persistent state the vendor owns and Ralphy can corrupt

Raw log: `docs/spike/live/antigravity-782-state.log`.

### 🔬 A config-root flag exists: `--gemini_dir=<absolute path>`

It is hidden (not in `agy --help`); the updater's own command line shows it.
`agy --gemini_dir=C:\…\Temp\agyprobe\gd models` wrote the state root to
`gd\antigravity-cli\` and the config to `gd\config\`, and nothing under the
real `%USERPROFILE%\.gemini` changed. A relative value is refused and falls
back to the default (`.gemini must be an absolute path … falling back to
default`). A temp `HOME` (Linux) or `USERPROFILE` (Windows) has the same
effect. **On Windows, auth is not part of the root** (§5): an isolated root
still signs in from the OS keyring. On WSL, the token file is part of it.

### 🔬 What one headless run writes (Windows, Flash, isolated root)

`brain/<conversation-id>/.system_generated/logs/transcript.jsonl` and
`transcript_full.jsonl` (per-step records with `input_tokens`,
`output_tokens`, `cache_read_tokens`), `conversations/<id>.db`,
`conversation_summaries.db`, `implicit/<id>.pb`, `jetbox_summaries_proto.pb`,
`cache/last_conversations.json` (`{"<repo path>": "<id>"}`), `log/cli-*.log`,
`presence/<id>.lock` (left after exit), `last_check.timestamp`,
`updater/update_status.json`, `config/config.json`,
`config/projects/default-cli-project.json`. Size after one run: 14 MB, mostly
`builtin/`. The run wrote nothing into the repo (`git status` empty).

### 🔬 Write-back of argv

`--model gemini-3.8-flash-low` was **not** written back: no `settings.json`
was created in the root, and no file outside the conversation store and the
log names the model. A headless run also did not add the repo to
`trustedWorkspaces` (the operator's real `settings.json` holds the folders
trusted in interactive sessions). The rejected-flag-then-rerun probe of
ADR-0040 C11 costs model calls and is left to Stage 4.

### 🔬 The binary itself is shared state, and the CLI rewrites it

`USERPROFILE=<tmp> agy models` (agy 1.4.9) logged `Spawned background update
process with PID 8872`. PID 8872 (`agy.exe --bg-updater …`) outlived its
parent, downloaded about 210 MB, and at 08:19:51 replaced
`%LOCALAPPDATA%\agy\bin\agy.exe` with 1.5.6, keeping
`agy.exe.<n>.old` until the next run removed it. Its status went to the
isolated root (`Update successful, restart CLI to use`), but the binary is the
one every root shares. The check runs at most every 15 minutes per root
(`Last check was less than 15 minutes ago, skipping update (fast path)`), so a
fresh Ralphy-owned root checks on its first run. 🔬 The operator's own roots
show the same: the WSL binary updated itself at 07:47
(`Update successful`), and a Windows attempt at 07:46 failed (`Update failed,
please install from website`). 🔎 No flag, setting or environment variable
turns it off (`AUTO_UPDATER_TEST_MODE` is the only updater name in the
strings); `agy --version` does not spawn it.

### Other state shared with other tools

- 🔬 The root `~/.gemini/` is shared with the old Gemini CLI: agy reads
  `~/.gemini/extensions` (`Detecting Gemini extensions in
  C:\Users\PICHAU\.gemini\extensions`) and `~/.gemini/config/config.json`.
- 🔬 The Windows keyring already holds `gemini-cli-api-key/default-api-key`
  next to agy's `gemini:antigravity`.
- 🔎 `agy plugin import` imports plugins _"from gemini or claude"_; it runs
  only on that command.

**C11 answer:** agy keeps everything under `~/.gemini/antigravity-cli/` plus `~/.gemini/config/` (shared with the old Gemini CLI tree), and the hidden flag `--gemini_dir=<absolute path>` (or a temp `HOME`/`USERPROFILE`) moves both to an isolated root (🔬); on Windows the OAuth credential stays in Windows Credential Manager whatever the root; a headless run with `--model` wrote no settings and did not change `trustedWorkspaces` (🔬); but every launch except `--version` can replace the shared binary through a detached background updater, at most every 15 minutes per root, with no opt-out found (🔬 1.4.9 → 1.5.6 during this spike), so a run can change the agent version under the next run.

## Hooks and integrations

Filled by Stage 5.

## Feature parity

What Ralphy has for Claude Code and for Gemini today, and what `agy` must show.
**A** is required to replace Gemini. **B** is what Claude has and Gemini may
lack. **C** is optional and may be left to a later issue. The table is copied
from #782; the answer column is filled by Stages 4 and 5.

| # | Tier | Feature | What `agy` must prove | Answer |
| --- | --- | --- | --- | --- |
| 1 | A | Tokens per call: input, output, cache read, cache creation (ADR-0008) | The four counts are separate. Per-call records are cumulative or incremental, with a test that fails the wrong choice | 🔬 Separate: `input_tokens` (uncached), `output_tokens` (thinking included), `cache_read_tokens`, `thinking_tokens`; no cache-creation count exists. Per-call records are incremental (sum = `result.usage`: 8 325 + 8 451 + 8 579 = 25 355), so a keep-last parser fails the test (C6) |
| 2 | A | Priced usage (ADR-0034) | Every id is in the price table, with its provider | 🔬 No: none of the 13 ids (Google, Anthropic, OpenAI) nor their base names has a row in `crates/ralphy-pricing/src/floor.rs`; the key needs the effort suffix removed (C4) |
| 3 | A | Model per turn, `Usage::fold_usage` | Which model answered each turn | 🔬 Not in the stream (`init.model` only echoes `--model`); per call in the SQLite `gen_metadata` row (label and backend id). One model per run was observed (C4, C6) |
| 4 | A | Model recovery (ADR-0053) | The model can be read back from the store | 🔬 Yes, from `conversations/<id>.db` `gen_metadata` (label `Gemini 3.8 Flash (Low)` maps to the `agy models` id); the transcript JSONL has no model (C6) |
| 5 | A | Usage scan and interactive usage (ADR-0033) | A read-only scan of the store that never errors on a missing store. If there are no local counts, the gap is stated and no number is invented | 🔬 A read-only scan is possible: `brain/*/.system_generated/logs/transcript_full.jsonl` (input, output, cache read) or `conversations/*.db` (also model and thinking); conversations with no usage exist and must be skipped; no cache-creation count to report (C6) |
| 6 | A | Auth preflight (ADR-0013) | The login command, and the exit code and stderr marker when logged out | 🔬 Login is `agy` with no arguments; logged out (WSL) exits 1 with `Authentication required. Please visit the URL to log in:` after a fixed 60 s wait; Windows exit code not observed (C5) |
| 7 | A | Limits and synthetic reset (ADR-0003, ADR-0030) | How a limit shows up, and a reset hint or `Limit(None)` | 🔎 Not observed: expected exit 3 with an `AGY_ERROR:` line and quota strings (`RESOURCE_EXHAUSTED`, `You have exhausted your quota on this model.`); 🔬 reset hint from `agy -p /quota --output-format json` (`reset_time` per bucket), else `Limit(None)` (C7) |
| 8 | A | Outcome classifier signals (ADR-0023) | Which `CompletionSignals` each stream fact fills | 🔬 `done`/`blocked`: sentinel in `result.response`; `timed_out`: `finished_reason:"print_timeout"`; `exited_ok`: exit 0 and `status:"SUCCESS"` and no `finished_reason`; `errored`: `status:"ERROR"`, `AGY_ERROR`, or `denied_actions`; `limit`: C7 (C3) |
| 9 | A | Budgets: per issue, idle, run deadline (ADR-0038) | Whether `--print-timeout` helps, and whether the output stops when the agent hangs | 🔬 `--print-timeout 30s` ended a hung turn at 33 s with exit 3, `finished_reason:"print_timeout"`, and killed its background task; stdout is silent while the agent waits on a background command (25 s here), so an idle budget must allow it (C3) |
| 10 | A | Effort ladder (ADR-0044) | The map from `low`..`max` to `--effort` and the id suffix | 🔬 `--effort` (`low`, `medium`, `high`, `xhigh`, `max`) = the id suffix; a base id plus `--effort` selects the variant, a base id alone takes the model's default (medium); a conflicting pair or a missing level is refused with exit 1; no listed model has `xhigh` or `max`, `gpt-oss-120b` has `medium` only (C4) |
| 11 | A | The five roles and the four one-shots | `plan`, `execute`, `diagnose_repo`, `draft_issues`, `triage_issues`, `consolidate_knowledge` | 🔬 `plan` ran with `--mode plan` (read-only plan, no file changed) and `execute` with `--dangerously-skip-permissions` (wrote a file, ran a command); the four one-shots use the same stdin + `stream-json` path and were not run one by one |
| 12 | A | Overlay slots and plan charters | `overlay.antigravity.md`, including the `stagedplan` charter | |
| 13 | A | Run artifacts under `.ralphy/` | Whether the agent can read gitignored files. Gemini could not (the inlining in `crates/ralphy-agent-gemini/src/context.rs`) | 🔬 Yes: with `.ralphy/` in `.gitignore`, `read_file` read `.ralphy/note.md` and the reply quoted its marker; no inlining is needed |
| 14 | A | Images (ADR-0025) | Whether headless mode accepts an attachment | 🔬 No attachment channel headless: the NDJSON input refuses an `image` block (`only "text"`, exit 1); but the agent read a PNG by path with `read_file` and named its color, so an image saved in the repo (for example under `.ralphy/`) and named in the prompt works |
| 15 | A | Autonomy revocation | Whether an administrator or a policy can silently turn off auto-approve, as detected in `crates/ralphy-agent-gemini/src/revocation.rs` | |
| 16 | A | Wiring of ADR-0040: five tiers, three agent enums, `config.rs`, the daemon roster and keyboard digit | `ralphy config set` works for every axis | |
| 17 | B | Agent state by hooks (ADR-0059) | The hook events of section 2 | |
| 18 | B | Guard on every tool call (ADR-0072) | A hook or a policy that can veto | |
| 19 | B | Cooperative stop (ADR-0054) | Which signal makes the agent end cleanly | |
| 20 | B | Degraded API state | A banner or a record for a failing API, and a restart path (compare `crates/ralphy-agent-claude/src/api_watch.rs`) | |
| 21 | B | Resume with context | `--continue` and `--conversation <id>` for a retry, and whether the id can be chosen first | |
| 22 | B | Context window and compaction | Whether the CLI compacts on its own, and whether the used share of the window is readable | |
| 23 | B | Account rotation on a limit (ADR-0060) | Whether more than one account can be used, and how one is chosen | |
| 24 | B | Complexity routing (CONTEXT.md) | Whether the planner can pick the model, with the multi-vendor catalog | |
| 25 | B | Review role (ADR-0077) | Whether `agy` can act as reviewer | |
| 26 | C | Interactive console on a PTY, screen snapshot, image paste (ADR-0079, ADR-0055) | The console runs, and the screen can be read | |
| 27 | C | Notes on the stage and review notes on a diff (ADR-0064, ADR-0061) | What the console needs from the agent | |
| 28 | C | Supervised session and peers | `SessionSpec` carries both `args` and `env`, so a launch is as contained as a run | |
| 29 | C | Doorbell and letters (ADR-0076) | A documented way into a running session | |
