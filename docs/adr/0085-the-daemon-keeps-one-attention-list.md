# The daemon keeps one attention list, and every agent state comes from one kind of state source

Status: proposed
Kind: structural
Protects: observability and cost, consistency of the workbench; constraint security

Amends ADR-0059 (`blocked` leaves the agent state; one kind of state source;
new fold rules; the vendor table), ADR-0076 D9 and D13 (the attention list
moves here and squads become one more source of it), and ADR-0083 D3 (a
`waiting` agent blocks hibernation).

## Context

An operator runs several agents at once, in consoles and in **runs**, on more
than one machine. What they need to know is short: which agent waits for
them now, and, when they are away from the desk, a message that says so.
Today:

- Only Claude has an **agent state** (ADR-0059). Codex, OpenCode, Kimi,
  Copilot, Cursor and Gemini have none on the run path or in a console
  (`state_source` in `crates/ralphy-daemon/src/agent_state.rs`).
- The run writes its agent state into the **Run snapshot** (`phase.agent`),
  but the workbench does not show it. The Telegram card pushes a run's
  `waiting`; nothing pushes a console's `waiting`, and the daemon cannot send
  a Telegram message at all: the client lives in `crates/ralphy-cli/src/telegram/`.
- **Attention request** is in CONTEXT.md, and ADR-0076 D13 decides a list of
  them, but inside the squads track. An operator with one agent and no fence
  needs the list too.
- `blocked` is in the ADR-0059 vocabulary, but no fold produces it. A blocked
  issue is an outcome of the run (`Outcome::Blocked`, the `Blocked` issue
  status), not something the agent process is doing.
- ADR-0076 D9 plans a second kind of state source, an event stream from an
  `opencode serve` server. `StateSource::Stream` exists in the daemon and
  nothing returns it.

Measured on 2026-10-28 with the real vendor CLIs (Windows 11; WSL
Ubuntu-22.04), with a recorder as the hook command and, where noted, the real
`ralphy hook status`. The reports are local notes of the track; the facts
that decide a rule are below.

| Vendor | Version | Given at launch by | A human is asked | The wait ends | No signal for |
|---|---|---|---|---|---|
| Claude Code | 2.1.312 | `--settings <file>` | `PermissionRequest`; `PreToolUse` of `AskUserQuestion`; `Elicitation` (from the vendor's hooks reference, not measured) | `PostToolUse`; `ElicitationResult` (not measured) | an interrupt (Esc) |
| Codex CLI | 0.160.0 Windows, 0.165.0 WSL | `-c hooks.<Event>=…` and `--dangerously-bypass-hook-trust` (without it no hook runs, and Codex says nothing) | `PermissionRequest` (also under auto-review, with no human); `PreToolUse` of `request_user_input` | `PostToolUse`, only when the tool ends; `Interrupt` | a granted permission |
| OpenCode | 1.19.0 Windows, 1.18.4 WSL | a plugin file named in `OPENCODE_CONFIG_CONTENT`, which the adapter already sets; the user's own plugins still load | `permission.asked`; `question.asked` (TUI only) | `permission.replied`; `question.replied` / `rejected` | — |
| Kimi Code | 0.29.0 | only `[[hooks]]` in `<KIMI_CODE_HOME>/config.toml`; no launch flag | `PermissionRequest`; `PreToolUse` of `AskUserQuestion` | `PermissionResult`; `PostToolUse`; `Interrupt` | — |
| Copilot CLI | 1.0.91 | `--plugin-dir <dir>` with `plugin.json` and `hooks.json` | `Notification` with `permission_prompt` or `elicitation_dialog` (`PermissionRequest` also fires under `--allow-all-tools`, with no dialog) | `PostToolUse`, only when the tool ends | an interrupt; a cancel |
| Cursor CLI | 2026.08.31 | a repo file or the user's file only | nothing: it ignores `PermissionRequest`, and its shell hook fires for every call | — | the wait itself |
| Antigravity `agy` | 1.4.0 | `--add-dir <dir>` with `.agents/hooks.json` | `PreToolUse` of `ask_question`; nothing for a permission prompt | `PostToolUse` | a permission prompt |

Three facts hold for more than one vendor:

- **Stdout.** A permission hook of Claude with empty stdout fails closed, so
  `ralphy hook status` prints `{}`. Kimi adds the `{}` of `UserPromptSubmit`
  to the model's context. `agy` reads `{}` from `PreToolUse` as a refusal
  and blocks every tool. Codex accepts both.
- **Fail closed.** Copilot's `PreToolUse` and `PermissionRequest` deny the
  tool when the hook exits non-zero or its binary is missing. `agy`'s
  `PreToolUse` too.
- **Subagents.** Codex and Kimi give a subagent's events the lead's
  `session_id` (Codex adds `agent_id`); Copilot gives the subagent its own id,
  and a subagent's `Stop` arrives in the middle of the lead's turn.
  `status_line` in `crates/ralphy-cli/src/hook.rs` keeps neither `session_id`
  nor `agent_id`, and keeps `tool_input` only for `AskUserQuestion`.

## Decision

### The agent state

**D1. The agent state has four values: `working`, `waiting`, `done`,
`unknown`.** `blocked` leaves the vocabulary (ADR-0059 §1). A blocked issue
is read from the run's outcome, as an attention request (D8), not from the
agent.

**D2. Every state source is a file that the vendor fills.** The vendor fills
it through a hook command (`ralphy hook status`) or through a plugin that
Ralphy writes and gives at launch (OpenCode). There is no second kind of
source: `StateSource::Stream` is removed, and the OpenCode console does not
run on an `opencode serve` server for its state (ADR-0076 D9).

**D3. Ralphy gives its hooks and plugins at launch, never in the user's
global config of a vendor.** The ways are the ones in the table: a flag, an
environment variable that names a file Ralphy owns, or a folder Ralphy owns.
A vendor whose hooks can only live in the user's own config (Kimi, Cursor)
gets no state source until an amendment decides how.

**D4. Only an event that fires only when a human is asked makes `waiting`.**
Each vendor's mapping table picks that event. `PermissionRequest` of Copilot
does not count, because it fires under `--allow-all-tools` with no dialog;
Copilot's `Notification` does. Codex's `PermissionRequest` counts only when
the console's own launch flags say no auto-review; with auto-review it is
`working`. For Claude, `Notification` stays out (ADR-0059 §4).

**D5. A status hook is never registered on an event that blocks the tool
when the hook fails,** unless the vendor has no other event for the same
state. Claude's `PreToolUse` and `PermissionRequest` are such an exception,
and `{}` keeps them open.

**D6. What `ralphy hook status` prints depends on the vendor.** `{}` for
Claude, nothing for every other vendor. The command takes the vendor as an
argument.

**D7. The fold has these rules, for every vendor:**

1. A line from a session other than the console's or the run's lead session
   (by `session_id`, and by `agent_id` where the vendor uses it) never sets
   the state. A `waiting` of a subagent is a `waiting` of the lead: the
   operator answers it in the same console.
2. A `waiting` keeps the state it replaced. Its end event (`PostToolUse`,
   `ElicitationResult`, a reply) gives that state back.
3. A `PostToolUse` of another tool (another `tool_use_id`) does not end a
   `waiting`.
4. An `Interrupt` event is `done`.
5. A vendor that sends no event when the operator grants a permission
   (Codex, Copilot) ends a permission `waiting` at the first input the
   operator writes to that console after it. The state becomes `working`
   with the detail "approved, the tool runs".
6. Claude and Copilot send nothing on an interrupt. Their state stays
   `working` until ADR-0059 §6 ages it into `unknown`. This is a known limit.
   No terminal output, title or vendor transcript fills it.

**D8. The status line keeps `session_id`, `agent_id`, the tool name, the
tool use id, the question text of every vendor, and the notification type.**

**D9. A source that is not `none` writes a first line when the agent starts.**
A plugin that does not load is silent in every vendor that has one, so the
first line is the only proof (ADR-0059 amendment of 2026-10-28, A2).

### The attention list

**D10. The daemon that hosts a source owns its attention requests.** Each
daemon works out its own list from its consoles and from the run snapshots
of its registered repos, read on a tick of a few seconds. The daemon the
page talks to joins the lists of its peers, as it joins `/api/sessions`. A run in a repo that no
daemon registers is not on any list; it still sends its own Telegram
messages.

**D11. These open an attention request:**

| Source | Ends when |
|---|---|
| A console agent `waiting` | the state leaves `waiting`, or the console closes |
| A run agent `waiting` | the snapshot says so |
| A **silent** console | its first status line arrives, or it closes |
| An issue of a run that is `Blocked`, `Hitl` or `NeedsSplit` | the operator dismisses it, a new run works the same issue in the same repo, or the run's folder is removed |
| An agent the idle watchdog reaped | the same as the line above |
| A console whose "tell me when it is done" mark is on, at its next `working → done` from `Stop` | the operator opens the console, or dismisses it |

A finished run's requests come from `<run folder>/snapshot.json`, its last
snapshot, which the run writes when it ends, before it removes the live one
(amendment, Paulo, 2026-10-28).

A usage-limit wait, a plain `done`, a `done` at a session boundary (resume,
clear, compact) and a turn the operator interrupted open nothing. A stalled
squad and an `ask_human` (ADR-0076) are sources when they are built.

**D12. Only a dismissal and the "tell me when it is done" mark are stored.**
Dismissals are an append-only file in the daemon store, keyed by run and
issue. The mark is on the desk record, like the **console name** (ADR-0066),
and turns off when it fires. A live source (a `waiting`, a silent console)
cannot be dismissed: hiding an agent that still waits is the error the list
exists to prevent.

**D13. A request shows what is in the agent state and nothing more:** the
vendor, the console or the issue, the kind of request, its age, and the
one-line detail. Its only action is "go to the console" or "open the run".
Nothing answers the vendor's prompt from the list. The page shows the
number of open requests at the top and in the tab title.

### Telling the operator

**D14. Telegram is the channel for an operator who is away.** A run keeps
sending its own `waiting` message at once, as today. The daemon sends the
console ones: one message per request, two minutes after it opens, and only
if it is still open and the operator is not present. A silent console sends
nothing. The message links to the console.

The operator is **present** when a page has its tab visible and the
operator's last key, click or touch in it is less than five minutes old.
Each page reports both facts to the daemon it is connected to, and that
daemon passes "present" to its peers on the link it already holds. With no
signal (no page, a peer without the link, a report older than the window)
the operator is not present: one message too many costs less than an agent
that waits all night. The bot client and its config move from `ralphy-cli` to a crate
that both the CLI and the daemon use (`ralphy-telegram`); it depends on no
other Ralphy crate.

**D15. A browser notification is opt-in,** off by default, and fires only
when the tab is hidden. No Web Push: it needs a vendor push service and an
installed app.

**D16. Telegram only sends.** No reply from Telegram reaches an agent.

### The idle console

**D17. A `waiting` agent blocks hibernation, not suspension** (ADR-0083 D3).
A suspended process keeps its prompt and comes back in under a second; a
hibernated one loses it. `unknown` and `none` keep the ADR-0083 rules.

## Consequences

- ADR-0059 loses `blocked` and gains D4–D9 as fold rules. Its §7 rule, that
  a guessed state is worse than an absent one, is unchanged and decides D7.6.
- ADR-0076 D9 has one kind of source; D13 points here; D17 (the server tree)
  no longer applies to state. The squad tracks of #673 consume this list.
- Issue #675 leaves the squads track and is rewritten against this ADR.
- The v1 vendors are Claude, Codex and OpenCode. Kimi waits for D3 (its hooks
  live only in its home folder). Copilot can follow with a plugin folder.
  Cursor and `agy` stay `none`: neither has an event for a permission prompt,
  and a green dot on a waiting agent is the lie D4 forbids.
- The daemon gains an outgoing call to the Telegram API. ARCHITECTURE.md §6
  and §7 change with it.
- The hook event list of Claude is written in the adapter and in the daemon.
  It gets one shared fixture, like the mapping.

## Considered options

- **Answer from the list, or from Telegram.** Each vendor's prompt is a
  different menu in each version, and a wrong key becomes the operator's
  answer to a permission. A Telegram reply is also a new way for a command to
  reach an agent that runs with its permissions skipped, which ADR-0072
  would have to decide. If it comes, it starts as "allow or deny" for one
  permission, by a button, never free text. OpenCode's server would be the
  easiest vendor for it.
- **Show the full question as a card.** It stores each vendor's question
  JSON for something the one-line detail and "go to the console" already
  give.
- **Telegram for every `waiting`, at once.** An operator at the desk gets a
  buzz for each prompt they are looking at. The two-minute delay drops those.
- **Send only when no page is visible.** A desktop tab left open would stop
  the message to the phone. Presence also needs a recent input (D14).
- **An OpenCode server and its event stream.** It adds a port, a password, a
  process tree to end, and a second kind of source. The plugin gets the same
  events.
- **The terminal as a source** (the structured state sequence some vendors
  write, the title, a spinner). Any program in the PTY can write the same
  bytes, and the hook sources cover the v1 vendors.
- **The vendor's global config.** Ralphy would have to find its own entries
  again to remove them, and a user's edit can break them.

## Compliance

- D1: not checked by code: manual, the vocabulary is reviewed in the PR.
- D2: not checked by code: the daemon matches `session::Agent` exhaustively,
  so a new vendor must name its source; nothing checks that the source is a
  file.
- D3: not checked by code: manual, reviewed in the PR of each vendor.
- D4, D7: checked by the shared mapping fixture of each vendor, which pins
  the fold of the adapter and of the daemon.
- D5, D6: not checked by code: needs a live vendor CLI.
- D10: not checked by code: manual.
- D14: not checked by code: the crate graph is reviewed in the PR
  (ARCHITECTURE.md §5).
- D16: not checked by code: manual; no inbound Telegram call exists outside
  `ralphy telegram setup`.
