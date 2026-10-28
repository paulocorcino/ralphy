# Claude Code hook probes, 2026-10-28

Measured on Windows 11, with Claude Code 2.1.312 (the installed CLI on
Windows and in WSL) unless a row says otherwise. Each probe ran
`claude -p --model haiku` in an empty temporary folder outside any
repository, with `< /dev/null` (without it, `claude -p` waits 3 s for stdin)
and with `RALPHY_STATUS_FILE` pointing at a file in that folder. Older CLIs
were installed with `npm install --prefix <tmp> @anthropic-ai/claude-code@<v>`
and run with `DISABLE_AUTOUPDATER=1`. The folder and the installs were
deleted after the probes.

These probes ground the 2026-10-28 amendments of ADR-0059, ADR-0072 and
ADR-0040.

## M1 — a guard hook that cannot start lets the tool run

| Hook command on `PreToolUse`, matcher `Bash` | Hook result | Bash tool |
|---|---|---|
| `"\\?\C:\Users\…\ralphy.exe" hook guard` | Git Bash: `\?\C:\Users\…\ralphy.exe: No such file or directory`, exit 127 | **ran** (`probe.txt` was written) |
| `echo blocked-by-probe >&2; exit 2` | exit 2 | blocked; the model reported the hook's message |

The prompt asked the model to run `echo PROBE > probe.txt`, with only that
command in `--allowedTools`. A hook that exits with any code other than 0
and 2 is a non-blocking error: Claude Code runs the tool.

The first command is the one Ralphy wrote before commit `d66214e2`, on a
machine where the daemon was started by `ralphy daemon install`: the
autostart entry, and therefore `current_exe()`, carried the `\\?\` prefix
that `std::fs::canonicalize` returns on Windows. A run the daemon starts
inherits that path. So, on such a machine, a run started from the workbench
had a guard hook that did not start. This is deduced from the code and M1;
no log of a past run was checked.

## M2 — the exec form needs Claude Code 2.1.139

The exec form is a hook entry with `"command": "<exe>"` and
`"args": ["hook", "status"]`: Claude Code starts the program directly, with
no shell. Hooks on `SessionStart`, `UserPromptSubmit` and `Stop`, the
command `C:\Dev\ralphy\target\debug\ralphy.exe`:

| CLI | Status lines written | What happened |
|---|---|---|
| 2.1.312 | 3 of 3 | the program ran with `hook status` |
| 2.1.139 | 3 of 3 | the program ran with `hook status` |
| 2.1.138 | 0 of 3 | `args` was ignored and `command` went to Git Bash unquoted: `C:Devralphytargetdebugralphy.exe: command not found`, exit 127 |

So on an older CLI the exec form does not run the program without
arguments: the hook fails. On a guard, that is M1.

The same three hooks in the shell form, with the path quoted and written
with forward slashes (`"C:/Dev/ralphy/target/debug/ralphy.exe" hook
status`), wrote 3 of 3 lines on 2.1.312.

## M4 — `disableAllHooks` turns off the hooks Ralphy passes

A project `.claude/settings.json` with `{"disableAllHooks": true}`, and the
M2 hooks passed by `--settings`: **no hook started** (no `hook_started` line
in `--output-format stream-json --verbose --include-hook-events`), neither
Ralphy's nor a plugin's. The key was not tested in the user's own
`~/.claude/settings.json`; the project file is below `--settings` in Claude
Code's order of settings, so the user file is expected to act the same.

## M3 — `SessionStart` fires only after the folder trust question is answered

Measured on 2026-10-28, Windows 11, Claude Code 2.1.312. A throwaway Rust
program drove `claude.exe` through `ralphy_pty::PtySession` (ConPTY,
40 rows × 140 columns) and answered the cursor-position query. The folder
was a new `git init` folder under `%TEMP%` with no entry in
`~/.claude.json`. Command:

```
claude.exe --dangerously-skip-permissions --settings <s.json> --model haiku
```

with `RALPHY_STATUS_FILE` set and no positional prompt. `s.json` has
`SessionStart`, `UserPromptSubmit` and `Stop` hooks in the shell form
(`echo "<Event> $(date +%s%3N)" >> "$RALPHY_STATUS_FILE"`).

| When | Status file |
|---|---|
| 15 s after the spawn, the question "Is this a project you created or one you trust?" on screen | empty (no file) |
| after the answer "Yes, I trust this folder" (`ESC [ B`, then `\r`) | `SessionStart` written within 10 s |

So no hook runs while the trust question waits. The question's default
choice is "No, exit": a bare `\r` chose it, and no hook ran at all. Claude
Code then wrote the folder to `~/.claude.json` with
`hasTrustDialogAccepted`; the probe removed that entry afterwards.

## M5 — a headless session holds its prompt only with stream-json input

Measured by the planner, 2026-10-28, Windows 11, Claude Code 2.1.312,
`claude -p --model haiku` with the `SessionStart` and `UserPromptSubmit`
hooks of M3, and the flags named in each bullet:

- Prompt on plain stdin, written 8 s after the spawn: `SessionStart` fired at
  +3.9 s, AFTER claude gave up on stdin ("Warning: no stdin data received in
  3s, proceeding without it."), then "Error: Input must be provided either
  through stdin or as a prompt argument when using --print", exit 1. So with
  plain stdin, the prompt cannot wait for `SessionStart`.
- `--input-format stream-json --output-format stream-json --verbose`, one
  user message written 10 s after the spawn: `SessionStart` at +0.8 s,
  `UserPromptSubmit` at +10.2 s, result `OK`, exit 0. So a headless session
  can hold its prompt until the first hook event.
- `--input-format stream-json` without `--output-format stream-json`:
  "Error: --input-format=stream-json requires output-format=stream-json.",
  exit 1.
- The user message line is
  `{"type":"user","message":{"role":"user","content":"<prompt>"}}`.

## M6 — an interactive session holds its prompt; a long prompt needs a pause before `\r`

Measured on 2026-10-28, Windows 11, Claude Code 2.1.312, with the program,
command and `s.json` of M3, in a temporary `git init` folder marked trusted
in `~/.claude.json` (`projects["<path with forward slashes>"]
.hasTrustDialogAccepted = true`, the way `ensure_interactive_session_ready`
does; the entry was removed afterwards). The probe waited for `SessionStart`
in the status file, then waited a "settle" time (1500 ms, or 0 ms), then
wrote the prompt. A variant "submits" when `UserPromptSubmit` is written and
the model answers (`Stop` written; for the short prompt, `● OK` on screen).
The environment of the probe carried `CLAUDE_CODE_CHILD_SESSION` from the
Claude Code session that ran it (the TUI showed "Transcript saving is off").

(a) `SessionStart` arrives with no prompt: in every one of the 28 spawns, 1.2 s to
3.2 s after the spawn (3.2 s only in the first spawn; 1.2 s to 1.6 s after).

(b) Which bytes submit the prompt:

| Variant | Prompt | Settle | Submitted |
|---|---|---|---|
| (i) text + `\r` in one write | `Reply with the single word OK.` | 1500 ms | 3 of 3 |
| (i) text + `\r` in one write | same | 0 ms | 3 of 3 |
| (ii) text, pause 300 ms, `\r` | same | 1500 ms | 3 of 3 |
| (iii) `ESC [200~` text `ESC [201~`, pause 300 ms, `\r` | same | 1500 ms | 3 of 3 |
| (iii) the same, pause 0 ms | same | 1500 ms | 1 of 1 |
| (i) text + `\r` in one write | `EXEC_CHARTER` (125 bytes) | 1500 ms / 0 ms | **0 of 3**: the text shows in the input box, `\r` does not submit it, no hook fires in 60 s |
| (ii) text, pause 300 ms, `\r` | `EXEC_CHARTER` | 1500 ms | 3 of 3 |
| (iii) bracketed paste, pause 300 ms, `\r` | `EXEC_CHARTER` | 1500 ms | 3 of 3 |
| (ii) text, pause 100 ms, `\r` | `EXEC_CHARTER` | 0 ms | 3 of 3 (`UserPromptSubmit` 0.2 s to 0.3 s after `\r`) |
| (ii) text, pause 30 ms, `\r` | `EXEC_CHARTER` | 0 ms | 0 of 1 |

So the sequence that submits every time, for the short prompt and for the
real `EXEC_CHARTER`, is **(ii): the prompt text in one write, a pause, then
`\r` in a second write**. The shortest pause that worked 3 of 3 is 100 ms;
30 ms failed. A pause of 300 ms worked 6 of 6. The prompt may be written as
soon as `SessionStart` is in the status file (settle 0 ms). One write of a
long text with `\r` at its end does not submit: the TUI takes the `\r` as
part of the typed text.

## M7 — the shapes of the stream-json output

Measured on 2026-10-28, Windows 11, Claude Code 2.1.312, in an empty
temporary folder. Each command got one user message (the M5 line) on stdin:

```
claude -p --model haiku --input-format stream-json --output-format stream-json --verbose
```

| Case | Prompt | Exit | Fixture |
|---|---|---|---|
| (a) done | two short lines, then `RALPHY_DONE_EXIT` alone on the last line, no tools | 0 | `crates/ralphy-agent-claude/tests/fixtures/stream_json_done.jsonl` |
| (b) blocked | exactly the line `RALPHY_BLOCKED_EXIT missing key`, no tools | 0 | `stream_json_blocked.jsonl` |
| (c) logged out | `Reply OK.`, with `CLAUDE_CONFIG_DIR=<empty temp dir>` and `ANTHROPIC_API_KEY` empty | 1 | `stream_json_logged_out.jsonl` |

stderr was empty in all three cases, so there is no `.stderr.txt`.

- Line order in (a) and (b): `system/hook_started` and
  `system/hook_response` for each `SessionStart` hook (the user's own plugin
  hooks), `system/init`, in (a) `system/thinking_tokens`, then one or more
  `assistant` lines, a `rate_limit_event` line, and the `result` line.
- The text is in an `assistant` line's `message.content[]` items of
  `"type":"text"`, field `text` (an `assistant` line can hold only a
  thinking item and no text item), and again in the `result` line's
  `result` field. A newline in the text is the JSON escape `\n`, so in the
  raw stream the sentinel is never at the start of a line: in (a) the raw
  line ends with `\nRALPHY_DONE_EXIT"`.
- (a) and (b) carry a `rate_limit_event` line even with no limit reached;
  (c) does not.
- Logged out, the text is on **stdout**, not stderr: one `assistant` line
  with `"model":"<synthetic>"` and the text `Not logged in · Please run
  /login`, then a `result` line with `"is_error":true`,
  `"terminal_reason":"api_error"`, the same text in `result`, and exit 1. It
  really was logged out: no model call was made (`total_cost_usd` 0, no
  `modelUsage`).
- Before the fixtures were saved, the user's home folder name was replaced by
  `user`, the temp folder name by `probe`, and the plugin hook's output in
  `hook_response` by `<plugin hook output removed>`. No token or e-mail was
  in the output.

## Side facts

- `ralphy hook status` exits 0 and prints `{}` when it cannot append to the
  status file; the error goes to stderr only. This is ADR-0059 §3 by design
  ("It never fails the agent"), so a hook that runs but writes nothing is
  visible only as an absence.
- With `--include-hook-events`, `claude -p` reports each hook as a
  `hook_started` and a `hook_response` line with `exit_code`, `outcome`,
  `stdout` and `stderr`. That is enough for a probe to see why a hook failed.
