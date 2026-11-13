# The Antigravity adapter replaces the Gemini adapter, driven from a root Ralphy owns

Status: accepted
Kind: vendor
Protects: extensibility, observability and cost; constraints security, portability

Replaces ADR-0043 when it is accepted and the Gemini adapter is removed (see
the section _ADR-0043_ below). Applies the
[ADR-0040](./0040-agent-adapter-onboarding-contract.md) onboarding contract,
and proposes one change to its C12 rule 4 for this vendor (D9).

## Context

Google replaced the Gemini CLI with the Antigravity CLI, a binary named `agy`
(#729). Ralphy's Gemini adapter (ADR-0043) drives a CLI that no longer gets
new models. The spike #782 answered the ADR-0040 questions C1 to C11 for
`agy` 1.4.9 and 1.5.6 on Windows 11 and WSL Ubuntu, on 2026-11-12, observed
its hooks, and filled the parity table of #782. Every fact below comes from
[docs/research/antigravity-cli-adapter-spike.md](../research/antigravity-cli-adapter-spike.md)
(the spike) and its raw logs `docs/spike/live/antigravity-782-*.log`. This
ADR records the decisions; the spike records the observations and marks each
one as observed, read from the binary, or documented only.

What shapes the decisions:

- `agy` is one sign-in in front of 13 model ids from Google, Anthropic and
  OpenAI, with the effort in the id (spike §4).
- It has a usable headless contract: the prompt on stdin, an NDJSON stream
  with one terminal `result` line, and a structured error line (spike §1–§3).
- Its hooks work, and a pre-tool hook fails closed, but a hook is a shell
  string only, and no hook event comes before the first prompt (spike,
  _Hooks and integrations_).
- Some of its behavior cannot be turned off: a background updater, built-in
  skills, and the reading of the repository's own `.agents/` files and
  `AGENTS.md` (spike §8, §9, §11).
- On Windows the sign-in lives in Windows Credential Manager, not in the
  state root, so no root Ralphy owns isolates it (spike §5).

Every decision below is a recommendation. The operator accepts or changes
each one; the adapter is built after that, in #784.

## Decision

### D1 — Antigravity is one vendor with many models

**Decision.** Ralphy adds one vendor, `antigravity`, in one crate,
`ralphy-agent-antigravity`, selected per run with `--agent antigravity`. The
13 model ids are models of that one vendor. Ralphy reads the list from
`agy models` (free, no model call) and does not pin it in code. The spike
recommends this shape; this ADR keeps it as a recommendation for the
operator to accept.

Rests on: [spike, _Vendor shape_ and §4](../research/antigravity-cli-adapter-spike.md).
One sign-in, one quota read-out, one stream format and one store serve all
13 ids; the list depends on the plan and changes with each self-update.

Rejected: one Ralphy vendor per provider (for example "Claude through
Antigravity"). The provider changes nothing in how Ralphy drives the CLI,
so each extra vendor would repeat every place of the ADR-0040 wiring
inventory for no new behavior.

### D2 — A run is one headless process; the prompt goes on stdin

**Decision.** The adapter spawns `agy` with the workspace as its working
directory, stdin a pipe (never a terminal), and every flag stated, including
the default ones (ADR-0040 C11):

`--gemini_dir=<owned root> --output-format stream-json --input-format stream-json --dangerously-skip-permissions`
plus `--model <id>` and `--effort <level>` when set (D4).

The prompt is written to stdin as NDJSON lines
(`{"event":"user","message":{"role":"user","content":"…"}}`), after the
proof turn of D9, and stdin is then closed. `-p` is never passed. `--mode`
and `--sandbox` are never passed (D14, D15). Ralphy's own budgets
(ADR-0038) stay the deadline; `--print-timeout` is not passed.

Rests on: [spike §1, §2 and _`stream-json` input_](../research/antigravity-cli-adapter-spike.md).
Piped stdin carried 72 KB of UTF-8 whole; `-p <text>` drops stdin with no
error; without `--dangerously-skip-permissions` a shell command is
auto-denied and the run still exits 0. There is no cwd flag. The NDJSON
input keeps one process for several turns, which D9 needs.

Rejected: the prompt in argv with `-p` (the Windows argv limit of about
32 KB, and `-p` takes the next flag as the prompt). Rejected: plain stdin
with no `--input-format`, because it allows only one turn, so no proof turn
(D9). Open for #784: a prompt of 72 KB as one NDJSON line was not measured;
plain stdin of that size was.

### D3 — Completion: the sentinel decides, the exit code alone never does

**Decision.** The adapter fills `CompletionSignals` (ADR-0023) from the
stream, as the spike maps them:

| Signal | Source |
| --- | --- |
| `done` / `blocked` | the last line of `result.response` is the sentinel |
| `timed_out` | `finished_reason:"print_timeout"`, or Ralphy's own deadline |
| `exited_ok` | exit 0 **and** `result.status:"SUCCESS"` **and** no `finished_reason` |
| `errored` | `status:"ERROR"`, an `AGY_ERROR:` stderr line, a non-empty `denied_actions`, or a stream with no `result` line |
| `limit` | D6 |
| `committed` | Ralphy's HEAD check, as for every vendor |

The `Stop` hook is not a completion signal. Progress comes from the HEAD
diff; the stream has no field for files changed.

Rests on: [spike §2, §3 and parity row 8](../research/antigravity-cli-adapter-spike.md).
The sentinel held in 5 of 5 runs. Exit 0 also comes with an auto-denied tool
and, under `--input-format stream-json`, with a print timeout. Every run
observed, errors included, ended with exactly one `result` line.

Rejected: the `Stop` hook as the completion signal: it does not fire after
an interrupt, so a missing `Stop` cannot tell "not done" from "stopped".

### D4 — Models and effort: an optional id, the effort as a separate flag

**Decision.** The settings section `antigravity` has `plan_model`,
`exec_model` and the effort axis, each `Option`, set with `ralphy config set`.
When a model is unset, `--model` is omitted and `agy` uses its default
(today `gemini-4-argon-medium`, which depends on the plan). Ralphy passes a
base id (`claude-sonnet-5-5`) with `--effort` from its effort ladder
(ADR-0044); a model id that already ends in `-low`, `-medium` or `-high` is
passed as it is, with no `--effort`. An unknown id, an unknown effort, or a
pair that conflicts is refused by `agy` with exit 1 before any model call;
the adapter reports that as a setting error, not as an outcome of the issue.

Rests on: [spike §4 and parity rows 10 and 24](../research/antigravity-cli-adapter-spike.md).
A run with `--model` wrote nothing back into the root (§11), so omitting the
flag does not inherit an old value. No listed model has `xhigh` or `max`;
`gpt-oss-120b` has `medium` only.

Rejected: a model list pinned in code (it is plan-scoped and changes with
each self-update). Rejected: always passing the default model explicitly
(the default differs by plan, and Ralphy cannot know it without a call).

### D5 — Usage is the sum of the per-call records of the stream

**Decision.** The adapter sums the `usage` of every `step_update` with
`step_type:"agent_response"` and `state:"DONE"`. It never reads
`result.usage`. The mapping into `Usage` (ADR-0008):

| `agy` field | Ralphy count |
| --- | --- |
| `input_tokens` | input (it excludes cache reads) |
| `cache_read_tokens` | cache read |
| `output_tokens` | output (it includes `thinking_tokens`) |
| none | cache creation: `agy` reports none; the count is 0 and the gap is stated, never estimated |

The model of a run is the `--model` id Ralphy passed. When none was passed,
the model is recovered at read time (ADR-0053) from the store:
`<root>/conversations/<id>.db`, table `gen_metadata`, opened read-only. The
conversation id is the first stream line (`init.conversation_id`). The
interactive usage scan (ADR-0033) reads the same SQLite files under the
owned root, skips a conversation with no usage, and never fails on a
missing store. The `gen_metadata` rows are protobuf with no published
schema: a row the parser cannot read gives "model unknown", never a guess.

Rests on: [spike §6, the resume section, and parity rows 1, 3, 4 and 5](../research/antigravity-cli-adapter-spike.md).
Per-call records are incremental (8 325 + 8 451 + 8 579 = 25 355 =
`result.usage`). On a resumed conversation, `result.usage` is the total of
the whole conversation (34 107 input), while the run's one call used 8 835.
The stream never names the model when `--model` is omitted.

Rejected: `result.usage` (wrong on every resume). Rejected: the transcript
`transcript_full.jsonl` as the only source (no thinking count, no model).

### D6 — Limits: the structured error line, and the reset from the quota read-out

**Decision.** A limit is classified from the `AGY_ERROR:` JSON line and from
stderr, by the patterns `RESOURCE_EXHAUSTED`, `quota`, `429`, `rate limit`,
`exhausted`, `credits balance is too low`. The reset time comes from
`agy -p /quota --output-format json`, run after the failure in the same
root (no model call): the `reset_time` of the bucket of that model's group
whose `remaining_fraction` is 0. With no bucket at 0, the outcome is
`Limit(None)` with the synthetic cadence (ADR-0003, ADR-0030). Ralphy adds no
retry: the CLI already retries per-minute 429 and 5xx errors. Ralphy never
turns on the `UseG1Credits` setting.

Rests on: [spike §7 and parity row 7](../research/antigravity-cli-adapter-spike.md).
No limit was reached in the spike; the signals are read from the binary and
its changelog. The quota read-out is observed and costs 0 tokens.

Open for #784: a real limit is observed once, and the classifier's patterns
are checked against it. Until then the classifier rests on strings read from
the binary.

### D7 — Auth: the login command is `agy`; the logged-out run is stopped early

**Decision.** The auth error message tells the operator to run
`agy --gemini_dir=<owned root>` with no other argument and sign in (D8 says
why the root is named). The preflight (`agent_logged_in`) runs
`agy --gemini_dir=<owned root> models`, which makes no model call: exit 1
with `Please sign in to view available models` means logged out. In a run,
the adapter matches the first stderr line
`Authentication required. Please visit the URL to log in:` and ends the
child at once, instead of waiting the fixed 60 seconds; the JSON
`result.error` `authentication failed or timed out` is the second marker.

The child gets an explicit environment allowlist, as for Gemini (ADR-0043
D7). It does not pass `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `AGY_ADC_AUTH` or
the other `GOOGLE_*` variables: the first version supports the interactive
sign-in only. An API key or Application Default Credentials sign-in is a
later decision under ADR-0072.

The **Windows logged-out signature is inferred, not measured**: the four
stderr strings are in `agy.exe`, but the logged-out state on Windows needs a
`/logout` of the operator's own session. The operator chose to measure it in
the live capstone of #784.

Rests on: [spike §5 and parity row 6](../research/antigravity-cli-adapter-spike.md).
WSL logged out: exit 1 after a fixed 60 s wait, whatever `--print-timeout`
says. A terminal on stdin makes the wait read the terminal.

Rejected: waiting for the exit code (60 seconds lost per logged-out run).
Rejected: passing `GEMINI_API_KEY` through (alone it has no effect, the
documentation says; it also needs a `modelProvider` setting in the root).

### D8 — The isolation root is one owned root per OS user

**Decision.** Every run, one-shot and console passes
`--gemini_dir=<Ralphy's data folder>/antigravity-root` (an absolute path;
`agy` refuses a relative one). Ralphy materializes its skills into that root,
writes its hooks file there (D9), and never touches the operator's
`~/.gemini`. One root serves every workspace of that OS user:

- On Windows the sign-in is in Windows Credential Manager (target
  `gemini:antigravity`) whatever the root, so the root cannot isolate the
  account anyway.
- On Linux the sign-in is a token file inside the root, so the operator
  signs in once for Ralphy's root, with the command of D7.
- A console started in a new root shows a colour screen, the Terms of
  Service with the data-collection box already checked, and the folder
  trust question. Ralphy does not answer them for the operator; with one
  root, the operator sees them once. Headless runs skip all three.

Rests on: [spike §5, §8 and §11, and parity rows 23 and 28](../research/antigravity-cli-adapter-spike.md).
`--gemini_dir` moves all state and replaces the whole global layer of skills
and rules; a headless run changed no setting and no trusted folder.

Rejected: one root per workspace, as Gemini does (ADR-0043 D4): on Linux
each repository would need its own sign-in, and on Windows it isolates
nothing more. Rejected: the operator's own `~/.gemini`: its skills,
`GEMINI.md`, extensions and hooks would enter every run, and it is shared
with the old Gemini CLI. Account rotation (ADR-0060) is not offered: one
account per OS user on Windows.

Open for #784: two runs at once in one root (the store and
`cache/last_conversations.json` are shared) are not measured.

### D9 — Hooks and the guard: a pre-tool guard in shell form, proved by a first turn

**Decision.** Ralphy writes `<owned root>/config/hooks.json` with its guard
on `PreToolUse` (matcher `*`) and its state hook on the six events. Each
command is the shell form, because `agy` has no other: the program is
`ralphy` itself (ADR-0040 C12 rule 1), **not quoted**, because `agy` passes
a quoted path to `cmd /c` with Go's escaping and the hook then fails. When
the program path has a space or a shell character, the adapter uses the
Windows short (8.3) form of the path; when there is none, the session is
refused with a message that names the path.

This ADR proposes to amend ADR-0040 C12 rule 4 for this vendor: an
unattended `agy` session may use the shell form, because:

1. A `PreToolUse` hook that fails, or answers anything but
   `{"decision":"allow"}`, **denies** the tool (fails closed). A guard that
   cannot start stops the work; it never lets a tool through.
2. The proof of C12 rule 5 is a **proof turn**. No hook event comes before
   the first prompt, so the adapter writes a first, fixed NDJSON message
   with no task (D2), waits up to 30 seconds for that session's
   `SessionStart` line in its status file, and only then writes the task.
   Without the line, the session ends as a fatal error of the run, as the
   ADR-0072 amendment of 2026-10-28 decides, and the operator's setting that
   allows a run without the proof applies as it does for Claude.
3. A hook error text that is not UTF-8 (`cmd` on a Portuguese Windows) made
   `agy` end the whole run with exit 3 and `invalid UTF-8`. The adapter
   classifies exit 3 with `invalid UTF-8` in stderr as a fault of the
   machine: a fatal error of the run whose message names the hook command,
   not an outcome of the issue. The checks of the program path above are
   there so that `cmd` never prints that error.

The state source (ADR-0040 Amendment 4, ADR-0059) moves from `none` to
`hooks` only with #784's mapping table and its fixture. `SessionStart` does
not fire again on a resumed conversation.

Rests on: [spike, _Hooks and integrations_, §3 and parity rows 17 and 18](../research/antigravity-cli-adapter-spike.md).
All six events fired in `-p` and in a console on Windows; a global `deny`
won over a workspace `allow`; `--settings` refuses a `hooks` key.

Rejected: no proof (the gap C12 rule 5 closes). Rejected: a time window after
the real prompt (the first tool call can run before the window ends).
Rejected: a `chcp` prefix in the command (not measured, and it makes the
command a composite shell line). The proof turn costs one small model call
per session (about 8 000 input tokens in the probes).

Open for #784: hooks seen firing from the file as Ralphy writes it, on
Windows and on Linux (ADR-0040 Amendment 4, sharpened C3); a program path
with a space on Windows; the proof turn itself.

### D10 — A repository's own `.agents/hooks.json` is an accepted, limited risk

**Decision.** `agy` runs a repository's own `.agents/hooks.json` in headless
mode with no trust question. Ralphy accepts this, as it accepts a Claude
Code project settings file, and limits it: Ralphy's guard is in the global
layer, and a global `deny` wins over a workspace `allow`. At launch, when
the workspace has `.agents/hooks.json` (or `.agent/` or `_agents/`, from the
working directory up to the repository root), the adapter writes a warning
with the file's path to the run's log and events. The repository's
`AGENTS.md`, `GEMINI.md` and `.agents/skills` are read too, with no switch;
the charter says what wins.

Rests on: [spike, _Hooks and integrations_, §8 and §9](../research/antigravity-cli-adapter-spike.md).

Rejected: refusing to run when a repository has such a file. A repository
that the agent works on can already give the agent instructions through
`AGENTS.md`, and the agent runs as the operator (ADR-0072 D6); the refusal
would stop repositories that use `agy` themselves and close no new path.

### D11 — The background updater cannot be turned off; Ralphy records the version

**Decision.** Ralphy does not try to stop the updater. At the start of each
run the adapter reads `agy --version` (that command starts no updater) and
writes the version into the run's events, so a change of behavior can be
traced to a version. The stream parser ignores unknown fields.

Rests on: [spike §9 and §11](../research/antigravity-cli-adapter-spike.md).
Every launch except `--version` starts a detached `--bg-updater`, which
outlived its parent by minutes and replaced the shared binary (1.4.9 to
1.5.6 during the spike). No flag, setting or variable turns it off.

Rejected: a private copy of the binary (242 MB, and which path the updater
rewrites from a copy is not measured). Rejected: blocking the update host
(a change to the machine outside Ralphy's tree).

Open for #784: whether Ralphy's process-tree kill ends an updater that is
replacing the binary, and whether the binary still runs after that.

### D12 — Remote control, `mic-serve` and the other daemons stay off

**Decision.** Ralphy never passes `--remote-control` and never runs
`agy remote-control start`, `remote-control serve`, `mic-serve`, `install`
or `update`. It does not offer `--remote-control` as a setting, in a run or
in a console. The guard's deny-list for `agy` sessions gains those five
commands, because the agent could run them through its shell.

Rests on: [spike, _Remote control and the microphone server_, and §9](../research/antigravity-cli-adapter-spike.md).
`--remote-control` prints a take-over link and the host name on the screen
(and a console screen can be shared through the workbench); `remote-control
start` wrote a `Run` key that starts a daemon at each logon; `mic-serve`
listens on a port.

Rejected: an operator setting for remote control. The operator who wants it
can start `agy` outside Ralphy; inside Ralphy, the link would sit on a
screen the workbench may show to a remote device. No charter runs those
commands, so the deny rule breaks no flow (ADR-0072 D6a).

### D13 — Built-in skills and the `swarm` plugin: turned off where a setting exists

**Decision.** Ralphy writes `{"plugins": {"swarm": {"enabled": false}}}` into
the owned root's `config/config.json` before the first run. The built-in
skills (`native_swarm`, `run-workflow`, `automation`,
`permissioned-github`, `migrate-agent-settings` and others) have no switch
in the main CLI; they stay, and "no delegation, no background task" is a
rule of the charter in `overlay.antigravity.md`. The guard still denies the
forge writes that `permissioned-github` could reach.

Rests on: [spike §8, §9 and _MCP servers, plugins and agents in a fresh root_](../research/antigravity-cli-adapter-spike.md).
The first run writes `swarm` as enabled into that file; the switch
`--include-default-customizations` exists only on `agy acp`. No MCP server
is bundled.

Rejected: driving `agy` through `agy acp` (Agent Client Protocol) only for
that switch: a second protocol, not probed.

Open for #784: whether `swarm` disabled in the owned root is honored (`-p
/skills` no longer lists it).

### D14 — The native plan mode is rejected

**Decision.** `--mode` is never passed. The plan pass runs in the ordinary
mode, with the same flags as execute, and writes `.ralphy/plan.md` itself;
the overlay's `mode-rules` slot says so, as Gemini's does (ADR-0043 D12).

Rests on: [spike §8 and parity row 11](../research/antigravity-cli-adapter-spike.md).
`--mode plan` wrote its plan to the vendor's private `brain/<id>/plan.md`
and asked for approval.

Rejected: `--mode plan` (no file in the repository, and an approval step a
headless run cannot answer).

### D15 — `--sandbox` is not used

**Decision.** Ralphy never passes `--sandbox`. The agent runs as the
operator and is not sandboxed (ADR-0072 D6); the guard of D9 is the layer
on top.

Rests on: [spike, _`--sandbox` and `--mode`_](../research/antigravity-cli-adapter-spike.md).
Under `--sandbox`, `curl` had no network, so `gh` and `git fetch`, which the
charters run, would fail; a write outside the workspace still ran.

Rejected: `--sandbox` as a default or an option, by ADR-0072 D6a: it breaks
what works and does not contain the file system.

### D16 — Images and `.ralphy/` files are read by path

**Decision.** `ACCEPTS_IMAGES` is true. An image is saved under `.ralphy/`
and its path is named in the prompt; the headless input has no attachment
channel. Files under the gitignored `.ralphy/` are read by the agent with
its own tools: the adapter does not inline them, as the Gemini adapter does
(`crates/ralphy-agent-gemini/src/context.rs`).

Rests on: [spike, parity rows 13 and 14](../research/antigravity-cli-adapter-spike.md).
The NDJSON input refuses an `image` block; the agent read a PNG by path and
a gitignored `.ralphy/note.md`.

Rejected: an image block in the input (refused, exit 1).

### D17 — The binary is found from an explicit list; stop is cooperative first

**Decision.** The locator lives in `ralphy-proc-util`, so the daemon can
call it (ADR-0042 D19), and tries `%LOCALAPPDATA%\agy\bin\agy.exe` on
Windows and `~/.local/bin/agy` on Linux and macOS before `PATH`. The
`antigravity.cmd` launcher of the IDE is never taken. To stop a run, the
adapter sends `CTRL_BREAK_EVENT` on Windows and `SIGTERM` on Unix, waits for
the `result` line, then ends the tree with `kill_tree` (ADR-0054).

Rests on: [spike §10 and parity row 19](../research/antigravity-cli-adapter-spike.md).
Neither install path is on a running process's `PATH`. `CTRL_BREAK_EVENT`
gave exit 1 within 10 ms, one `result` with `finished_reason:"interrupted"`
and the usage so far, and killed the running command.

Rejected: `PATH` only (the binary is off `PATH` until a new shell).

Open for #784: `SIGTERM` was seen only on a logged-out WSL run; a signed-in
stop on Linux is checked live.

### D18 — Resume by conversation id; the id comes from the stream or the first hook event

**Decision.** A retry or a hibernated console resumes with
`--conversation <id>`. The id cannot be chosen before launch; a run takes it
from `init.conversation_id`, and a console from the `conversationId` of its
first hook event. So `agy` needs no launch gate, and #758 can place it with
the agents whose console can be hibernated (ADR-0083 D7), when the operator
decides that issue.

Rests on: [spike, _Resume (#758 and parity 21)_](../research/antigravity-cli-adapter-spike.md).
`--conversation <id>` reopened a conversation in a console and in a headless
run; an unknown id starts a new conversation with a warning.

Rejected: a launch gate that lists conversations after the launch (not
needed: the id arrives in the first event).

### D19 — Antigravity replaces Gemini; `--agent gemini` becomes an error

**Decision.** The Antigravity adapter replaces the Gemini adapter. #785
removes the Gemini crate one release after the Antigravity adapter ships.
From that removal, `--agent gemini` is an error for one release, with the
message "the Gemini CLI agent was removed: use --agent antigravity" After
that release it is an unknown value like any other. The section _Moving
from Gemini_ gives the details.

Rests on: [spike, _Vendor shape_, §4 and §11](../research/antigravity-cli-adapter-spike.md).
The model ids differ (`gemini-3.1-pro` against `gemini-4-argon-medium`), the
state root differs, and the sign-in differs, so nothing of a Gemini setting
carries over as it is.

Rejected: `--agent gemini` as an alias of `antigravity` for one release.
The alias would run another binary, with another sign-in and another model
list, under the old name; a `gemini.plan_model` value would be refused by
`agy`. An error that names the new value tells the operator what changed.

## ADR-0043

When this ADR is accepted and #785 removes the Gemini crate, ADR-0043 (and
its companion `0043-gemini-validation.md`) gets the status
`superseded by ADR-0086` and a back-reference line under it. Until then,
ADR-0043 stays accepted: the Gemini adapter still ships.

Done: #785 removed the Gemini crate and ADR-0043 is superseded.

What carries over from ADR-0043, under this ADR's numbers:

| ADR-0043 | Here | Change |
| --- | --- | --- |
| D1 per-run selection | D1, D19 | the value is `antigravity` |
| D2 prompt on stdin | D2 | NDJSON lines, no `-p` |
| D3 completion | D3 | the sentinel decides; exit 0 is not enough |
| D4 owned root per workspace | D8 | one root per OS user |
| D5 autonomy asserted three ways | none | `agy` has no policy engine; `init.permission_mode` and `denied_actions` show the mode (parity row 15) |
| D6 auth on exit 41 | D7 | exit 1 and a stderr line, stopped early |
| D7 environment allowlist | D7 | kept |
| D8 optional model | D4 | kept, plus `--effort` |
| D9 usage from the envelope | D5 | per-call records summed |
| D10 scan is a lower bound | D5 | the SQLite store has every count but cache creation |
| D11 limits | D6 | reset from `/quota` |
| D12 plan mode rejected | D14 | kept |
| D13 skills in the owned root | D8 | kept |
| D14 images | D16 | by path |
| D15 blast radius closed | D12, D13 | what has a switch is off; the rest is a charter rule |
| D16 the WSL `/mnt/c` shim | D17 | an explicit probe list |
| D17 the vendor's terms | none | to review in #784 before the adapter ships |
| D18 `kill_tree` | D17 | cooperative stop first |

## Moving from Gemini

- **`--agent gemini`.** An error for one release after #785, naming
  `--agent antigravity` (D19); not an alias.
- **Settings.** The `gemini` section (`gemini.plan_model`,
  `gemini.exec_model`) is not converted: its ids are not `agy` ids. After
  #785 a stored `gemini` section is ignored, `ralphy config` prints one
  warning that names the `antigravity` keys, and `ralphy config unset` still
  removes it.
- **History.** Run reports, the spend ledger and events already written with
  the agent `gemini` stay as they are and keep their label. The interactive
  scan of the old `.ralphy/gemini-home` roots ends with #785; Ralphy does not
  delete those folders.
- **Workbench.** The keyboard digit `7` of the roster passes to Antigravity.
  A saved console whose agent is `gemini` is refused at launch with the same
  message as the CLI, not dropped without a word.
- **The console root.** `ralphy gemini prepare-root` (ADR-0040 Amendment 3)
  becomes `ralphy antigravity prepare-root`, which writes the owned root of
  D8 (skills, hooks file, `swarm` off) for a console; the daemon still
  refuses to open a console when the hooks file is missing.
- **Prices.** No row exists for the 13 ids. #784 adds rows keyed by the base
  id with the effort suffix removed (`gemini-4-argon`, `gemini-3.8-flash`,
  `claude-opus-5-5`, `claude-sonnet-5-5`, `gpt-oss-120b`), each with its
  provider's list price, and the key function that removes the suffix.
  Until a row exists, the usage is shown as not priced (ADR-0034), never as
  zero. The plan's quota share is a separate reading, not a price.

## Wiring for the adapter

Every place of the ADR-0040 wiring inventory that plan 2 (#784, #785)
changes. Paths are at this commit; grep for the symbol.

**Tier 1 — the crate** `crates/ralphy-agent-antigravity`: `lib.rs`
(`impl Agent`, `ACCEPTS_IMAGES = true`, the three budget builders),
`command.rs` (D2 argv, environment allowlist of D7, stdin not a terminal),
`auth.rs` (D7), `outcome.rs` (D3, D6, the `invalid UTF-8` fault of D9),
`usage.rs` (D5, the SQLite model recovery), `tasks.rs` (the four
one-shots), `skills.rs` (D8), `settings.rs` (D4), `root.rs` (D8: hooks file,
`swarm` off), `catalog.rs` (`agy models`), `effort.rs` (D4).

**Tier 2 — the prompt**: `assets/prompts/plan/overlay.antigravity.md` with
the slots of the spike §8 (`execution-model`, `self-review-step`,
`self-review-guidance`, `ledger-example`, `skill-invocation`, `mode-rules`;
`planning-mode-intro` and `stages-section` empty), the "no delegation" rule
of D13, and `VARIANTS` in `crates/ralphy-core/tests/prompt_assembly.rs`.

**Tier 3 — the registry**: `crates/ralphy-cli/src/cli.rs` (`CliAgent`,
`cli_name`, the error of D19); `init/gate.rs` (`Agent`, `ALL` and its
length, `accepts_images`, the `agent_logged_in` argv of D7);
`run/wiring.rs` `build_agent`; `run.rs`; `config.rs` and `config/view.rs`,
`config/write.rs` (the `antigravity` keys, the ignored `gemini` section);
the four one-shot matches (`init/run.rs`, `init/issues.rs`, `triage.rs`,
`main.rs::consolidate_with_agent`); `main.rs::consolidate_defaults`;
`models.rs` (`agent_slug`, `plan_action` from `agy models`); `pricing.rs`
and `crates/ralphy-pricing/src/floor.rs` (the rows of _Moving from Gemini_);
`runstate/capture.rs` (`EMIT_CALL_SHAPES`, `MIGRATED_EMITTERS`);
`crates/ralphy-cli/src/gemini.rs` becomes the `antigravity` subcommand;
`doctor.rs` and `doctor/probe.rs`; the workspace and CLI `Cargo.toml`.

**Tier 4 — usage scan and daemon**: `crates/ralphy-usage-scan/src/` (a new
`antigravity.rs` over the SQLite store, `recovery.rs`, `lib.rs`);
`crates/ralphy-daemon/src/usage.rs` and `StorePaths`;
`session/spec.rs::Agent` (`from_query`, `program_name` through the locator
of D17); `dispatch.rs` and `dispatch/gemini.rs`; `roster.rs` (digit `7`);
`agent_state.rs` (state source `none` until #784's mapping, D9);
`routes/ws_session/agent_launch.rs` and `gemini_root.rs` (the
prepare-root gate); `attention/telegram.rs` (the label);
`crates/ralphy-proc-util` (the locator of D17); the guard's deny-list (D12);
`assets/ui/wb-spend-ledger.ts` and `assets/ui-demo/wb-seed-agents.js`.

**Tier 5 — the tests that trip**: `prompt_assembly`;
`capture.rs::no_vocabulary_literal_outside_emit`; the `pricing.rs` model-id
assertion; `gate.rs`'s `accepts_images` assertion; `cli.rs`'s `--agent`
round trip; the daemon's `/api/usage` coverage; the Gemini tests that move
or go with #785 (`tests/verbs/gemini_root.rs`,
`tests/sessions/session_ws_gemini.rs`, `usage-scan/src/gemini/tests.rs`).

**The live capstone of #784** measures what the spike could not:

1. The Windows logged-out signature (D7), with the operator's own
   `/logout`, as the operator chose.
2. A real limit, against the patterns of D6.
3. A signed-in run on Linux: usage, hooks, stop (D5, D9, D17); the spike
   covered WSL logged out only.
4. Hooks firing from the file as Ralphy writes it, on Windows and Linux,
   with the proof turn and a program path with a space (D9).
5. `swarm` disabled in the owned root (D13).
6. Two runs at once in one root (D8).
7. A prompt of 72 KB as one NDJSON line (D2).
8. The updater ended by the tree kill (D11).
9. The repository upload A/B on two fresh repositories, with tool calls
   (ADR-0040 C9 sharpened); the spike saw no upload in a one-turn run.
10. The vendor's terms (ADR-0043 D17 asked the same for Gemini).

## Measured in #784

2026-11-13, Windows 11 and Ubuntu 22.04 on WSL 2, `agy` 1.5.8 (1.5.7 at the
logout), Flash only except the limit (item 2). The numbers are in
[784-antigravity-capstone-live.md](../spike/evidence/784-antigravity-capstone-live.md).

1. D7, Windows logged out: measured. `agy models` exits 1 with `Please sign
   in to view available models`; `agy -p` prints `Authentication required`
   at once and waits a fixed 60 s whatever `--print-timeout` says, exit 1,
   `authentication failed or timed out`, as on WSL. `ralphy models` stops in
   1 s and `ralphy run` about 1.5 s after planning starts, exit 1, with the
   sign-in message. `ralphy doctor` does not report the signed-out state.
2. D6, a real limit: observed on Windows. Five `claude-opus-5-5-high` calls
   emptied the 5 h bucket of the `Claude and GPT models` group (weekly to
   0.46; the Gemini group stayed at about 0.97). The sixth call exits 3 with
   `status: "ERROR"`, `Individual quota reached. ... Resets in 4h41m14s.`,
   usage 0, and an `AGY_ERROR` line with `RESOURCE_EXHAUSTED`, code 429,
   `retryable: true`. `ralphy run` classified it as a limit and waited for
   the reset (`remaining_min=280`), taken from `/quota`; the `Resets in`
   text of stderr is not parsed.
3. D5, D9, D17 on Linux: measured. A WSL run closed FinCal #132. Stream sum
   equals store sum in every session on both hosts, and the run's recorded
   usage equals both. The operator's stop (`ralphy stop`) sent `SIGTERM` to
   the `agy` group, and `agy` wrote one `result` with `finished_reason:
   "interrupted"` and the usage so far, and was gone in 1.3 s; the run
   summary does not count that usage. A `SIGTERM` to the `ralphy` process
   itself is not a stop: `ralphy run` exits 143 at once and `agy`, in its
   own group, finishes its turn alone.
4. D9, hooks from Ralphy's file: on Windows, `SessionStart` came about 3 s
   after each launch, all six events fired, and a program path with a space
   was written as the unquoted 8.3 short path and worked. On Linux, `agy`
   runs each hook as `sh -c` on the single-quoted path Ralphy wrote;
   `SessionStart` came about 7.5 s after each launch, and all six events
   fired.
5. D13, `swarm`: not honored for the skill list. With `swarm` disabled in
   `config/config.json`, `agy -p /skills` still lists `swarm`.
6. D8, two sessions at once in one root: both completed, each with its own
   conversation, store and status file; `last_conversations.json` is keyed
   by folder. Measured with two consolidation sessions, not two full runs.
7. D2, 72 KB as one NDJSON line: read whole (74 936 bytes, the right answer).
8. D11, the updater and the tree kill: the updater is a child of the
   session's `agy`, so the Windows tree kill reaches it, but it lived 5 s
   and no kill met it live; a kill during a binary replace needs a new
   release and is not measured. `agy --version` runs after a session. On
   Linux the updater starts in a new session and process group of its own,
   so the Unix group kill does not reach it; it ended by itself in 3.6 s.
9. ADR-0040 C9, upload A/B: two fresh repositories with tool calls; `agy`'s
   log names only `loadCodeAssist`, `fetchAvailableModels` and
   `streamGenerateContent` in both, with no upload,
   sync or index line. Log evidence only, no network capture.
10. Terms: <https://antigravity.google/terms> section 6 calls "using the
    Service in connection with products not provided by us" and "third party
    software ... to access the Service" a breach, with OpenClaw on
    Antigravity OAuth as the example. Ralphy runs Google's own binary and
    never touches its token; whether that is allowed is the operator's call.

Found on the way, for a fix stage: the knowledge consolidation of
`ralphy run` passes no `--model`, so it ran on `agy`'s default (Gemini 4
Argon Medium); the run summary prices the raw id `gemini-3.8-flash-low`
instead of its base id; the status file and `antigravity.log` keep only
the last session of a run; the 1.5.8 ids `gemini-4-hydrogen-*` and
`gemini-pro-orchestrator-g1-pro` have no price row, and `-xhigh` is not
stripped by `price_key`; `ralphy doctor` says `antigravity hooks: not
checked`.

## Consequences

- The proof turn of D9 adds one small model call to every unattended
  session. On the probes it was about 8 000 input tokens; on an expensive
  model it costs more, and it is visible in the run's usage.
- One root per OS user (D8) means the conversation store of every workspace
  is in one place: the usage scan reads one folder, and two runs at once
  share it (open for #784).
- The model of a run with no `--model` depends on the operator's plan and
  on the vendor's latest update; Ralphy learns it only from the store after
  the run (D5).
- ADR-0040 C12 rule 4 gets its first vendor exception (D9). If the operator
  accepts it, ADR-0040 gets an amendment in the same change as the adapter.

## Open risks of the spike, and where each one went

| Risk (spike §0) | Here |
| --- | --- |
| Guard hook in shell form on Windows | D9: unquoted path, short path for a space, C12 rule 4 exception; a path with a space measured in #784 |
| A non-UTF-8 hook error ends the run | D9: a machine fault, fatal for the run; path checks prevent it |
| No readiness proof before the prompt | D9: the proof turn |
| The repository's own `.agents/hooks.json` runs | D10: accepted, limited risk with a warning |
| Self-update has no opt-out | D11: accepted; the version is recorded; tree kill measured in #784 |
| Built-in subagent skills cannot be turned off | D13: `swarm` off by setting, the rest a charter rule |
| WSL is covered only logged out | open: capstone item 3 |
| Windows logged-out signature and a real limit | open: capstone items 1 and 2 (D7, D6) |
| On resume, `result.usage` is the conversation total | D5: sum the step records |
| First console launch shows the Terms of Service | D8: one root, shown once; Ralphy does not answer it |
| No price rows for the 13 ids | _Moving from Gemini_, Prices: rows by base id in #784 |
