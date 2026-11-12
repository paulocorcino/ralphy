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

## 0. Executive summary

Filled by Stage 5.

## 1. C1 — Invocation and the headless contract

Filled by Stage 4 or 5.

**C1 answer:** _Stage 4/5_

## 2. C2 — The output stream

Filled by Stage 4 or 5.

**C2 answer:** _Stage 4/5_

## 3. C3 — Completion and the sentinel

Filled by Stage 4 or 5.

**C3 answer:** _Stage 4/5_

## 4. C4 — Models

Filled by Stage 4 or 5.

**C4 answer:** _Stage 4/5_

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

Filled by Stage 4 or 5.

**C6 answer:** _Stage 4/5_

## 7. C7 — Limits

Filled by Stage 4 or 5.

**C7 answer:** _Stage 4/5_

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

Filled by Stage 4.

## Feature parity

What Ralphy has for Claude Code and for Gemini today, and what `agy` must show.
**A** is required to replace Gemini. **B** is what Claude has and Gemini may
lack. **C** is optional and may be left to a later issue. The table is copied
from #782; the answer column is filled by Stages 4 and 5.

| # | Tier | Feature | What `agy` must prove | Answer |
| --- | --- | --- | --- | --- |
| 1 | A | Tokens per call: input, output, cache read, cache creation (ADR-0008) | The four counts are separate. Per-call records are cumulative or incremental, with a test that fails the wrong choice | |
| 2 | A | Priced usage (ADR-0034) | Every id is in the price table, with its provider | |
| 3 | A | Model per turn, `Usage::fold_usage` | Which model answered each turn | |
| 4 | A | Model recovery (ADR-0053) | The model can be read back from the store | |
| 5 | A | Usage scan and interactive usage (ADR-0033) | A read-only scan of the store that never errors on a missing store. If there are no local counts, the gap is stated and no number is invented | |
| 6 | A | Auth preflight (ADR-0013) | The login command, and the exit code and stderr marker when logged out | |
| 7 | A | Limits and synthetic reset (ADR-0003, ADR-0030) | How a limit shows up, and a reset hint or `Limit(None)` | |
| 8 | A | Outcome classifier signals (ADR-0023) | Which `CompletionSignals` each stream fact fills | |
| 9 | A | Budgets: per issue, idle, run deadline (ADR-0038) | Whether `--print-timeout` helps, and whether the output stops when the agent hangs | |
| 10 | A | Effort ladder (ADR-0044) | The map from `low`..`max` to `--effort` and the id suffix | |
| 11 | A | The five roles and the four one-shots | `plan`, `execute`, `diagnose_repo`, `draft_issues`, `triage_issues`, `consolidate_knowledge` | |
| 12 | A | Overlay slots and plan charters | `overlay.antigravity.md`, including the `stagedplan` charter | |
| 13 | A | Run artifacts under `.ralphy/` | Whether the agent can read gitignored files. Gemini could not (the inlining in `crates/ralphy-agent-gemini/src/context.rs`) | |
| 14 | A | Images (ADR-0025) | Whether headless mode accepts an attachment | |
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
