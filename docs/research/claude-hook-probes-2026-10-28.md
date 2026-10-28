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

## Not measured

- **M3: does `SessionStart` fire before the folder trust question is
  answered?** An interactive `claude.exe` started under pywinpty 3.0.5 drew
  nothing on the pseudo-console in 15 s, so the probe saw neither the
  question nor a hook. To measure from the workbench: open a Claude console
  in a folder Claude Code has never seen, and before answering the question
  read `~/.ralphy/sessions/<id>.agent-status.jsonl`.

## Side facts

- `ralphy hook status` exits 0 and prints `{}` when it cannot append to the
  status file; the error goes to stderr only. This is ADR-0059 §3 by design
  ("It never fails the agent"), so a hook that runs but writes nothing is
  visible only as an absence.
- With `--include-hook-events`, `claude -p` reports each hook as a
  `hook_started` and a `hook_response` line with `exit_code`, `outcome`,
  `stdout` and `stderr`. That is enough for a probe to see why a hook failed.
