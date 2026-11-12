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
| Hook probes H1–H4 and H6 in `-p`: quoted path, every event, veto, workspace allow against global deny (hooks) | Windows | `gemini-3.8-flash-low` | 8 263 in, 39 out; 16 593 in, 41 out; 16 654 in, 40 out; 16 634 in, 57 out; 17 206 in, 79 out |
| Interactive console: one turn with a tool call, then one resumed turn (hooks, resume) | Windows | `gemini-3.8-flash-low` | 25 272 in, 45 out for the two turns (the conversation total less the headless resume) |
| Headless resume with `--conversation` (resume) | Windows | `gemini-3.8-flash-low` | 8 835 in, 4 out (the stream's one call; `result.usage` showed the conversation total, 34 107 in) |
| `--input-format stream-json` with a late first message; `CTRL_BREAK_EVENT` stop (hooks) | Windows | `gemini-3.8-flash-low` | 8 248 in, 1 out; 8 275 in, 55 out |
| C8 marker skills and rules, three runs (skills) | Windows | `gemini-3.8-flash-low` | 8 842 in, 63 out; 8 584 in, 22 out; 8 588 in, 22 out |
| `--mode plan` asked to write `.ralphy/plan.md` (skills) | Windows | `gemini-3.8-flash-low` | 19 075 in, 370 out |
| `--sandbox`: file writes, then a network call (hooks) | Windows | `gemini-3.8-flash-low` | 29 399 in, 183 out; 19 468 in, 117 out |
| `-p /skills`, `/hooks`, `/permissions`, `/config`, `/context`; `--settings` refusals; `--remote-control` and plain consoles with no prompt; `remote-control start`/`stop`; `mic-serve`; WSL logged-out hook load (C8, C9, hooks) | Windows, WSL | none | 0 |

## 0. Executive summary

One line per question; the evidence is in the section named.

- **C1 invocation:** headless print mode with the prompt on stdin and no `-p`; full autonomy is `--dangerously-skip-permissions`; no cwd flag, no PTY needed (§1).
- **C2 stream:** `--output-format stream-json`, NDJSON `init` / `step_update` / one terminal `result`; the final message is `result.response`; no progress field (§2).
- **C3 completion:** the `RALPHY_DONE_EXIT` last line held 5 of 5; exit 0/1/2/3 plus a JSON `AGY_ERROR:` stderr line; exit 0 is not success alone; all six hook events fire, but no hook fires before the first prompt (§3, _Hooks and integrations_).
- **C4 models:** one sign-in, 13 ids from Google, Anthropic and OpenAI, effort as id suffix or `--effort`, unknown id refused before any call; the stream does not name the answering model (§4).
- **C5 auth:** login is `agy` with no arguments; logged out (WSL) exits 1 after a fixed 60 s wait with `Authentication required…`; the Windows logged-out exit code is not observed (§5).
- **C6 usage:** per-call, incremental counts (input without cache, output with thinking, cache read; no cache creation); the model per call is only in the SQLite store; the id is the first stream line (§6).
- **C7 limits:** no limit reached; free reset times from `-p /quota --output-format json`; limit strings read from the binary (§7).
- **C8 skills:** skills from `.agents/skills` and the owned root, rules from `AGENTS.md` and `GEMINI.md`, nothing from `.claude` or `.codex`; `--gemini_dir` replaces the global layer; native plan mode rejected (§8).
- **C9 blast radius:** no MCP server, but built-in skills and the `swarm` plugin are always on; a detached updater outlives each run; remote control is off by default (§9).
- **C10 platforms:** one binary per OS, off `PATH`, explicit probe list; UTF-8 on Windows pipes (§10).
- **C11 state:** `--gemini_dir=<absolute path>` moves all state; the Windows credential stays in the keyring; the self-updater rewrites the shared binary (§11).
- **Hooks:** six events, workspace and global `hooks.json`; a pre-tool hook can veto and fails closed; shell form only.
- **Resume (#758):** `--conversation <id>` reopens a conversation in a console and in `-p`; the id cannot be chosen, but the first hook event carries it.
- **Vendor shape:** one vendor with many models.

Open risks for the ADR:

- **Guard hook in shell form on Windows.** agy runs a hook with `cmd /c` and Go's quoting, so a quoted program path fails; a path with a space is unproven, and ADR-0040 C12 rule 4 refuses a shell form for an unattended session unless the operator allows it.
- **A non-UTF-8 hook error ends the run.** A hook that fails with cmd's localized message ended the whole run with exit 3 (`invalid UTF-8`).
- **No readiness proof before the prompt.** `SessionStart` comes with the first prompt; a console gives no event before it (C12 rule 5 needs another proof, such as the `init` line in headless mode).
- **The repository's own `.agents/hooks.json` runs** as the operator in headless mode, with no trust question.
- **Self-update has no opt-out**, and it rewrites the binary that every root and the operator share (§11).
- **Built-in subagent skills cannot be turned off** in the main CLI (`native_swarm`, `run-workflow`), so "no delegation" is a charter rule only.
- **WSL is covered only logged out.** Hooks, usage and the execute path were probed on Windows; a signed-in WSL probe needs the operator's real `~/.gemini`.
- **Windows logged-out signature and a real limit** (C5, C7) are not observed.
- **On resume, `result.usage` is the conversation total**, not the run's: sum the step records.
- **First console launch in a new root** shows the Terms of Service with data collection pre-checked; a Ralphy-owned root shows it once per root.
- **No price rows** exist for any of the 13 ids (parity 2).

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
  integrations_: all six events fired in `-p`, `Stop` with
  `terminationReason:"NO_TOOL_CALL"` at the end of a turn (🔬), but not after a
  `CTRL_BREAK_EVENT` stop, so `Stop` is a completion hint, not the sentinel.
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

Raw log: `docs/spike/live/antigravity-782-skills.log`. Host: Windows, agy
1.5.6. A marker `SKILL.md` was planted in every candidate root, and a
`MARKER-` word in every candidate instruction file. `agy -p /skills` lists the
skills for free (0 tokens); one Flash turn then asked the model which marker
skills, rule words and commands it had.

### Which roots it reads 🔬

| Root | Skills | Instructions |
| --- | --- | --- |
| Repo `.agents/skills`, `.agent/skills`, `_agents/skills` | read | — |
| Repo `.claude/skills`, `.codex/skills`, `.gemini/skills`, `.cursor/skills` | not read | — |
| Repo `AGENTS.md`, `GEMINI.md` (§9) | — | read |
| Repo `CLAUDE.md` (§9), `.agents/rules/r.md` (no front matter) | — | not read |
| Global `~/.gemini/skills`, `~/.gemini/config/skills`, `~/.gemini/antigravity-cli/skills` | read | — |
| Global `~/.gemini/GEMINI.md`, `~/.gemini/AGENTS.md` | — | read |
| `~/.claude/skills`, `~/.codex/skills`, `~/.agents/skills`, `~/.claude/CLAUDE.md` | not read | not read |
| Workflows `.agents/workflows/*.md`, `~/.gemini/config/global_workflows/*.md`, `~/.claude/commands/*.md` | not offered as commands | — |

- 🔬 **`--gemini_dir` replaces the whole global layer.** With
  `--gemini_dir=<gd>`, the global roots are `<gd>/skills`, `<gd>/config/skills`,
  `<gd>/antigravity-cli/skills`, `<gd>/GEMINI.md` and `<gd>/config/AGENTS.md`;
  with a home that held other markers, none of the home's `~/.gemini` skills or
  rules were read. So Ralphy materializes its skills into the root it owns and
  the operator's own `~/.gemini` library does not leak in. The repo layer
  (`.agents/` and `AGENTS.md`, `GEMINI.md`) is still read.
- 🔬 **No cross-vendor leakage.** Unlike Cursor, agy does not read
  `.claude/skills` or `.codex/skills`; the bundled `migrate-agent-settings`
  skill copies them only when asked.
- 🔬 **Built-in skills are always on.** Every run lists 6 built-in skills and
  the `swarm` plugin (`agy-customizations`, `antigravity-guide`, `automation`,
  `migrate-agent-settings`, `migrate-workflows`, `run-workflow`, `swarm`). The
  switch `--include-default-customizations` exists only on `agy acp`; the main
  CLI refuses it (exit 2).
- 🔎 Skills load by progressive disclosure: only the name and description are
  in the prompt, and the body is read when the model activates the skill
  (bundled `agy-customizations` docs). A named skill is therefore discovered
  by name, as the Gemini overlay's `skill-invocation` slot already says.
- 🔬 **Native plan mode does not fit.** `--mode plan
  --dangerously-skip-permissions`, asked to write `.ralphy/plan.md`, wrote
  nothing in the repo: the plan went to the vendor-private
  `<root>/brain/<id>/plan.md` (with `plan.md.metadata.json`) and the reply asked
  for approval. Ralphy's plan pass must run in the ordinary mode and write
  `.ralphy/plan.md` itself; the Gemini overlay's `mode-rules` slot states this
  already, and fits agy as written.
- **Overlay slots** for `overlay.antigravity.md` (from the Gemini overlay and
  the facts above): `execution-model` — one piped turn on stdin;
  `self-review-step` and `self-review-guidance` — the review runs in the turn,
  because subagents are on by default (`native_swarm`, `run-workflow`, C9) and
  no flag to turn them off was found; `skill-invocation` — skills by name from
  the owned root; `mode-rules` — "native plan mode is not in use, write
  `.ralphy/plan.md` yourself"; `ledger-example` as Gemini's;
  `planning-mode-intro` and `stages-section` empty (no staged charter yet).

**C8 answer:** agy reads skills from the repo's `.agents/skills`, `.agent/skills` and `_agents/skills` and from the global `skills`, `config/skills` and `antigravity-cli/skills` folders of its root, and reads `AGENTS.md` and `GEMINI.md` from the repo and the root, never `.claude`, `.codex`, `.cursor` or `CLAUDE.md` (🔬 marker skills, `-p /skills` and a Flash turn); `--gemini_dir=<owned root>` replaces the whole global layer, so Ralphy can materialize its skills there and the operator's `~/.gemini` library is not read, while 6 built-in skills and the `swarm` plugin are always listed with no CLI switch to drop them; native plan mode (`--mode plan`) writes its plan to the vendor-private `brain/<id>/plan.md` and asks for approval, so it is rejected and the plan pass runs in the ordinary mode and writes `.ralphy/plan.md` itself (🔬); the overlay fills `execution-model`, `self-review-step`, `self-review-guidance`, `ledger-example`, `skill-invocation` and `mode-rules`, and leaves `planning-mode-intro` and `stages-section` empty.

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

Raw logs: `docs/spike/live/antigravity-782-hooks.log`,
`antigravity-782-remote.log`, `antigravity-782-resume.log`. Host: Windows,
agy 1.5.6, isolated root `--gemini_dir=<gd>`, Flash. The event list is the one
of the bundled `agy-customizations/docs/hooks.md`; the changelog names no
other. Each event was registered twice, in `<repo>/.agents/hooks.json` (tag
`ws`) and in `<gd>/config/hooks.json` (tag `gl`), with a command that appends
one JSON line per call. The interactive turn ran in a ConPTY (`pywinpty`,
140×40, the screen read with `pyte`) with no `-p`.

| Event | Registered in | Fired in -p | Fired interactive | Command |
| --- | --- | --- | --- | --- |
| `SessionStart` | `<repo>/.agents/hooks.json` and `<gd>/config/hooks.json` | 🔬 yes, both files, once per new conversation, when the first prompt arrives (with a stream-json message written at +10 s, it fired at +10.2 s) | 🔬 yes, both, 0.3 s after the first prompt was submitted; not at launch, not after the trust answer, and not on a resumed conversation | shell form `<python> -I <hook.py> <tag> <log>`, run by `cmd /c` on Windows (🔎 `sh -c` on Unix) |
| `PreInvocation` | both | 🔬 yes, before each model call (2 per tool turn) | 🔬 yes, before each model call | same |
| `PreToolUse` (matcher `*`) | both | 🔬 yes, before `run_command`; the answer decides: `{"decision":"allow"}` runs the tool, `{}` or a hook that fails **denies** it | 🔬 yes, before `run_command` | same; stdout must be `{"decision":"allow"}` |
| `PostToolUse` (matcher `*`) | both | 🔬 yes, after the tool, not after a denied one | 🔬 yes | same |
| `PostInvocation` | both | 🔬 yes, after each model call | 🔬 yes | same |
| `Stop` | both | 🔬 yes, `terminationReason:"NO_TOOL_CALL"` at the end of the turn; not after a `CTRL_BREAK_EVENT` stop | 🔬 yes, `NO_TOOL_CALL`, also on the resumed conversation | same |
| Every event, WSL | `~/.gemini/config/hooks.json` of a temp `HOME` | not observable — the temp root is logged out; the file loaded (`loaded 1 named hooks`) but nothing fired before sign-in, and a signed-in probe needs the operator's real `~/.gemini` (H10) | not observable — same reason | shell form `echo <Event> >> <file>` |

### What the hook probes show

- 🔬 **Payload.** Every call gets JSON on stdin with `conversationId`,
  `workspacePaths`, `transcriptPath`, `artifactDirectoryPath`, `modelName`
  (the real id, `gemini-3.8-flash-low`), `lastUserInput`, `hookEventName` and
  `cwd`; `PreToolUse` adds `toolCall` and `stepIdx`, `Stop` adds
  `terminationReason`, `fullyIdle`, `continuationActive`, `executionNum`,
  `error`. The hook process's environment has
  `ANTIGRAVITY_CONVERSATION_ID`, and its working directory is the folder of
  its `hooks.json`.
- 🔬 **Shell form only, and Windows quoting breaks a quoted path.** A handler
  has only `type: "command"` and a `command` string (🔎 docs: `cmd /c` on
  Windows, `sh -c` on Unix); there is no exec form with `args`, and
  `--settings` refuses a `hooks` key (`unknown setting "hooks"`, exit 1). With
  the program path in double quotes (ADR-0040 C12 rule 3), agy passed it to
  `cmd /c` with Go's argv escaping, so cmd saw `\"…\"` and every hook failed
  (`'\"…python.exe\"' não é reconhecido…`). The same path without quotes ran.
  A program path with a space is therefore not proven to work on Windows.
- 🔬 **A pre-tool hook fails closed.** A `PreToolUse` hook that could not start
  (exit 1) denied the tool (`JSON hook … failed` as a `TOOL_ERROR`), and a hook
  that printed `{}` denied it too (`tool call denied by pre-tool hook:`). This
  is the opposite of Claude Code (M1 of the Claude hook probes, where a guard
  that cannot start lets the tool run). Other events that fail are logged and
  the turn goes on. One trap: cmd's error text on this Portuguese-locale host
  was not UTF-8, and agy then ended the whole run with exit 3 and
  `proto: field google.protobuf.Value.string_value contains invalid UTF-8`.
- 🔬 **Order and merge.** The two files' handlers run one after another (about
  0.1 s each; the hooks block the loop), in an order that changed between runs.
  A workspace hook that answered `allow` did not override a global hook that
  answered `deny`: the tool was denied (H6). After a `deny`, the next
  `PreToolUse` handler did not run.
- 🔬 **A repository's own hooks run.** `<repo>/.agents/hooks.json` ran in
  headless mode with no trust question; in a console it ran after the folder
  trust answer. So a repository Ralphy works on can run commands as the
  operator through agy, as it can through a Claude Code project settings file.
- 🔬 **No hook proves readiness before the prompt** (ADR-0040 C12 rule 5).
  `SessionStart` fires when the conversation is created, which is when the
  first prompt arrives, in `-p`, in `--input-format stream-json` and in a
  console. In headless mode the readiness signal is the `init` line (at +2.3 s,
  before any prompt); a console gives no hook event before its first prompt.

### Veto of a tool call (parity 18) 🔬

With `--dangerously-skip-permissions` (`permission_mode:"always-proceed"`), a
`PreToolUse` hook that printed
`{"decision":"deny","reason":"blocked-by-ralphy-probe"}` for `echo VETOME >
vetoed.txt` stopped it: the tool step ended `ERROR` with `tool call denied by
pre-tool hook: blocked-by-ralphy-probe`, `vetoed.txt` was not created, and the
model replied `REFUSED: blocked-by-ralphy-probe`. The run exited 0 with
`status:"SUCCESS"`; the veto is not in `denied_actions`, only in the tool step.

### `stream-json` input 🔬

`--input-format stream-json --output-format stream-json`: the `init` line (with
the conversation id) arrived at +2.3 s with no input; a user message written at
+10 s ran a turn and gave a `result` at +13.3 s, and the process kept running
until stdin closed (exit 0). So one process takes several turns, one NDJSON
line each, and Stage 4's note holds: under this input format, exit 0 does not
mean success.

### Cooperative stop (parity 19) 🔬

`CTRL_BREAK_EVENT` to the process group 8 s into a `ping -n 40` turn: within
10 ms, stderr `error: interrupted`, one `result` line with `status:"ERROR"`,
`error:"interrupted"`, `finished_reason:"interrupted"` and the usage so far,
exit **1**; no `PING.EXE` was left, and no `PostToolUse` or `Stop` hook ran. In
a console, Ctrl+C twice exits and prints `Resume with -c (or command below):
agy --conversation=<id>`. On WSL, `timeout` (SIGTERM) during the logged-out
wait still printed the JSON `result` envelope (H10).

### MCP servers, plugins and agents in a fresh root 🔬

`agy mcp list`: `No MCP servers configured.`; `agy plugin list`: `No plugins
installed.`; `agy agents`: empty. But `-p /skills` lists 6 built-in skills
and the built-in `swarm` plugin (C8, C9). `agy acp` (the Agent Client Protocol
over stdio) has switches the main CLI lacks: `--include-default-customizations`,
`--disable-telemetry`, `--login` (📖 `agy acp --help`).

### Remote control and the microphone server

| Command | What it opened 🔬 | Reading under ADR-0072 |
| --- | --- | --- |
| `--remote-control` on a console | printed `Remote control on for this session`, the machine's host name, and a take-over link `https://antigravity.google.com/r/<id>`; sockets of its PID: outbound to Google `:443` only (the rows in state Listen carried a remote Google `:443` address, a socket mid-connect); none without the flag | Opens the session to the operator's Google account (D1: inside the operator zone, like Claude's `remote_control`), but the link on screen is a take-over; Ralphy never passes it unless the operator asks |
| `remote-control start --session` | wrote `HKCU\…\Run\AntigravityCliDaemon = agy.exe --gemini_dir=<gd> remote-control serve` (the logon task was refused: `Acesso negado`) and left a detached `remote-control serve` process, outbound `:443` only; `remote-control stop` removed both | A persistent autostart outside Ralphy's tree; Ralphy must never run it |
| `mic-serve` | `127.0.0.1:4713` listening (default `--addr`; the help says keep it on loopback) | Loopback, but a child must never run it; a voice channel is out of Ralphy's scope |

### `--sandbox` and `--mode`

- 🔬 `--sandbox` with `--dangerously-skip-permissions`: the CLI log says
  `Print mode: enabling terminal sandbox for this session`. A write inside the
  repo and a write under `%TEMP%` outside it both ran; `curl
  https://example.com` printed `000` and exited 1, and the model said it had no
  network. So the sandbox blocks the network, which would also block the
  `gh` and `git fetch` calls the charters run (ADR-0072 D6a); it is no default
  for Ralphy.
- 🔬 `--mode plan` is rejected (C8). 📖 `--mode accept-edits` and `--mode auto`
  exist; 🔎 Auto mode needs Windows 11 24H2 and a personal account, and has a
  second model review commands that leave the sandbox. Not run: the unattended
  path is `--dangerously-skip-permissions` (C1).
- 🔬 First interactive launch in a new root shows a colour-scheme screen, then
  _Terms of Service & Data Use_ with the data-collection box **already
  checked**, then the folder trust question (default `Yes, I trust this
  folder`). The probe unchecked the box. Headless runs skip all three.

### Resume (#758 and parity 21) 🔬

- An interactive turn, ended with Ctrl+C twice, printed
  `agy --conversation=6be64d5a-…`. A new console with `--conversation=<that
  id>` reopened it: the old turn was on screen, and asked which command it had
  run, the model answered `echo hook-probe`. The hooks saw the same
  `conversationId`, and `SessionStart` did not fire again.
- The same id in a headless run (`--conversation <id>`, prompt on stdin)
  continued the same conversation too (`init.conversation_id` unchanged, the
  answer right). But its `result.usage` was the **conversation's** total
  (34 107 input, `num_turns` 3, 70 s) while its one model call used 8 835: on
  resume, sum the step records, not `result.usage`.
- A chosen id cannot be given (Stage 4: `--conversation <unknown>` warns and
  starts a new UUID). The id is known without a launch gate: the first stream
  line in `-p`, and in a console the `conversationId` of every hook payload
  (the hook's environment carries it too), plus `cache/last_conversations.json`
  per workspace.

**Resume answer (#758):** The id cannot be chosen before launch (`--conversation <unknown id>` warns and starts a new CLI-made UUID), but `--conversation <id>` reopens an existing conversation both in a new console and in a headless run (🔬 the model recalled the earlier turn, and the hooks saw the same id); for a console, the new id reaches Ralphy in the `conversationId` of the first hook event (the first prompt's `SessionStart`), so no list-and-guess launch gate is needed if the console carries Ralphy's hooks.

### Vendor shape

agy routes one sign-in to 13 model ids from three providers (Google, Anthropic,
OpenAI open-weight), groups them into two quota buckets, picks the effort by
id suffix or `--effort`, and refuses an unknown id before any model call
(C4, C7). The list depends on the plan (`gemini-4-argon-trial`) and changes
with CLI updates.

**Vendor shape:** one vendor with many models — `agy models` lists 13 ids from Google, Anthropic and OpenAI behind one sign-in and one quota read-out, the list is plan-scoped and changes with self-updates, so Ralphy reads the list (free, deterministic) instead of pinning a set, and keys prices by base id with the effort suffix removed.

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
| 12 | A | Overlay slots and plan charters | `overlay.antigravity.md`, including the `stagedplan` charter | 🔬 Needs its own `overlay.antigravity.md`: `execution-model` (one piped turn on stdin), `self-review-step` and `self-review-guidance` (review in the turn), `ledger-example`, `skill-invocation` (skills by name from the owned `--gemini_dir` root) and `mode-rules` (native plan mode writes to `brain/<id>/plan.md`, so write `.ralphy/plan.md` yourself); `planning-mode-intro` and `stages-section` empty. The `stagedplan` charter is Claude-only today; nothing seen blocks it (its skill is found by name), left to the adapter issue (C8) |
| 13 | A | Run artifacts under `.ralphy/` | Whether the agent can read gitignored files. Gemini could not (the inlining in `crates/ralphy-agent-gemini/src/context.rs`) | 🔬 Yes: with `.ralphy/` in `.gitignore`, `read_file` read `.ralphy/note.md` and the reply quoted its marker; no inlining is needed |
| 14 | A | Images (ADR-0025) | Whether headless mode accepts an attachment | 🔬 No attachment channel headless: the NDJSON input refuses an `image` block (`only "text"`, exit 1); but the agent read a PNG by path with `read_file` and named its color, so an image saved in the repo (for example under `.ralphy/`) and named in the prompt works |
| 15 | A | Autonomy revocation | Whether an administrator or a policy can silently turn off auto-approve, as detected in `crates/ralphy-agent-gemini/src/revocation.rs` | 🔬 Partly: a tool the CLI will not approve shows as `denied_actions` in `result`, with exit 0, an empty response and the stderr line `… auto-denied`, and `init.permission_mode` names the mode the run got (`always-proceed` with `--dangerously-skip-permissions`); 🔎 the changelog says an Enterprise administrator's review rule now makes such headless runs deny those commands. The administrator path itself was not observed (one personal account) |
| 16 | A | Wiring of ADR-0040: five tiers, three agent enums, `config.rs`, the daemon roster and keyboard digit | `ralphy config set` works for every axis | Not observable in a spike (it is code). The CLI offers every axis `ralphy config set` sets: the agent binary (explicit probe list, C10), `--model` (13 ids), `--effort`, and the owned root `--gemini_dir`; no blocker was found for the five tiers |
| 17 | B | Agent state by hooks (ADR-0059) | The hook events of section 2 | 🔬 Yes on Windows: all six events (`SessionStart`, `PreInvocation`, `PreToolUse`, `PostToolUse`, `PostInvocation`, `Stop`) fired in `-p` and in a console, with the conversation id; `SessionStart` comes only with the first prompt, and no event reports a permission wait. WSL not observed (logged-out root). See _Hooks and integrations_ |
| 18 | B | Guard on every tool call (ADR-0072) | A hook or a policy that can veto | 🔬 Yes: a `PreToolUse` hook's `deny` stopped a command under `--dangerously-skip-permissions`; a hook that fails or prints `{}` also denies (fails closed); a global deny beats a workspace allow. Shell form only (`cmd /c`), and a quoted program path failed on Windows |
| 19 | B | Cooperative stop (ADR-0054) | Which signal makes the agent end cleanly | 🔬 `CTRL_BREAK_EVENT`: exit 1 within 10 ms, one `result` with `finished_reason:"interrupted"` and usage, the running command killed, no `Stop` hook; a console exits on Ctrl+C twice and prints its resume command; WSL SIGTERM still printed the `result` envelope |
| 20 | B | Degraded API state | A banner or a record for a failing API, and a restart path (compare `crates/ralphy-agent-claude/src/api_watch.rs`) | Not observed (no API failure happened). 🔎 The CLI retries `429` and `5xx` itself and a console's status line shows `Model unavailable (attempt 2/10), retrying in 12s`; a final failure is exit 3 with an `AGY_ERROR:` line (C3, C7). No stream record during retries is known, so a degraded state would be read from the console screen, as for Claude |
| 21 | B | Resume with context | `--continue` and `--conversation <id>` for a retry, and whether the id can be chosen first | 🔬 `--conversation <id>` reopens a conversation in a console and in `-p` (the model recalled the earlier turn); the id cannot be chosen first, but comes from the `init` line or the first hook payload; 📖 `-c` continues the latest one in the workspace. After a resume, `result.usage` is the conversation total, so sum the step records |
| 22 | B | Context window and compaction | Whether the CLI compacts on its own, and whether the used share of the window is readable | 🔎 Yes, the CLI compacts on its own and has `/compact`, and `/config` has a `contextWindow` key; the used share is in the interactive `/context` panel (refused in `-p`, exit 2) and in the custom status line JSON (`context_window_size`); the `-p` stream gives none. Compaction not observed |
| 23 | B | Account rotation on a limit (ADR-0060) | Whether more than one account can be used, and how one is chosen | 🔬 Windows: no — the credential is the fixed keyring target `gemini:antigravity`, the same for every root, so one account per OS user; Linux: yes — the token file is inside the root, so one account per `--gemini_dir` root (an isolated root is logged out). 📖 `GEMINI_API_KEY` or `AGY_ADC_AUTH` are other sign-ins |
| 24 | B | Complexity routing (CONTEXT.md) | Whether the planner can pick the model, with the multi-vendor catalog | 🔬 Yes: `--model` takes any of 13 ids from three providers per run, and `--effort` the level; a wrong pair is refused before any model call. The price table has no row yet (row 2) |
| 25 | B | Review role (ADR-0077) | Whether `agy` can act as reviewer | 🔬 Likely yes: a headless turn reads the repo with its tools and writes no file unless asked (plan probe, C1); 📖 `--json-schema` forces a structured final answer. A review itself was not run |
| 26 | C | Interactive console on a PTY, screen snapshot, image paste (ADR-0079, ADR-0055) | The console runs, and the screen can be read | 🔬 Yes: the console runs in a ConPTY (140×40) and a VT emulator read its screen (prompt, `● Ran (echo hook-probe)`, the reply). Text typed, a 0.5 s pause, then Enter submitted the prompt. A new root first shows a colour screen, the Terms of Service with data collection pre-checked, and the folder trust question. 🔎 Image paste via the terminal's paste key or `alt+v` on Windows |
| 27 | C | Notes on the stage and review notes on a diff (ADR-0064, ADR-0061) | What the console needs from the agent | 🔬 What the console needs is there: text written into the PTY reaches the prompt, and the hooks (row 17) tell when a turn starts and ends; no other channel into a console exists |
| 28 | C | Supervised session and peers | `SessionSpec` carries both `args` and `env`, so a launch is as contained as a run | 🔬 Yes: the console took the same argv as a run (`--gemini_dir=<owned root>`, `--model`, `--dangerously-skip-permissions`) and the owned root's hooks fired; the auth is the OS keyring on Windows, and on Linux the root's token file, so the env needs no secret |
| 29 | C | Doorbell and letters (ADR-0076) | A documented way into a running session | 🔬 A headless session takes more turns as NDJSON lines on stdin (`--input-format stream-json`, the process waits for EOF); 📖 `agy acp` (Agent Client Protocol over stdio) can steer a running turn; a console has only its PTY; `--remote-control` is a take-over through Google's site, not a way in for Ralphy |
