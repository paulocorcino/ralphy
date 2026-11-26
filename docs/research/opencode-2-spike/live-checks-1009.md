# OpenCode 2: live checks for #1009

The live probes that the code of #1009 needs before it ships: a standalone run on the
operator's own state (ADR-0094 D6d), `opencode models --standalone` (D6e), the deny map
under `--auto` (D3c), and the fixture databases of the usage readers (D4h). Markers:
🔬 observed, 🔎 read from the binary or its files, 📖 documented only.

Tools: OpenCode 2.3.2 (the operator's install: Windows `%APPDATA%\npm\opencode.cmd`, WSL
`~/.opencode/bin/opencode`), OpenCode 1.19.0 (a scratch npm install), Windows 11 and WSL
Ubuntu-22.04, 2026-11-25. Every run was started by a Python script in the way the adapter
starts it: the prompt on stdin, the process folder and `PWD` set to a scratch clone of
FinCal, `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` removed.

The operator's processes, never touched and alive at the end: Windows
`opencode.exe serve --service` PID 44232 and its `opencode-pty.exe daemon` PID 4484; WSL
PID 2216799 and its `opencode-pty daemon` PID 3980080.

## The model of the probes

The plan named `opencode/deepseek-v4.1-flash-free`. On the operator's login it failed with
`provider.free-limit` (HTTP 429) in the second step, after the run had written the file 🔬
(`docs/spike/live/opencode-1009-d6d-win-freelimit-standalone.log`). The operator's paid
provider, `zai-coding-plan/glm-5.3`, failed with `provider.quota` ("Insufficient balance")
🔬 (`docs/spike/live/opencode-1009-d6d-win-zaiquota-standalone.log`), and
`opencode/mimo-v2.6-flash-free` made the commit and then hit `provider.free-limit` 🔬
(`docs/spike/live/opencode-1009-d6d-win-mimo-standalone.log`). These are limits of the
accounts, not of `--standalone`: in each of them the session is in the operator's database,
no lock line is on stderr, and the service still answered. Every probe below used
`opencode/big-pickle`, a free model that ran to the end.

## D6d: a standalone run on the operator's state, with the service running

Command: `opencode run --standalone --auto --format json --print-logs --log-level error
-m opencode/big-pickle`, the operator's own `XDG_*`, prompt "Create the file probe.txt with
the word ok, then commit it.". `--auto` was added to the plan's command line, because a
2.x run that Ralphy starts passes it (D3c) and a headless run with no permission flag can
wait for an answer. After the run: a copy of the operator's three database files, a
process table, and a second default-mode run ("Reply with the word ok.").

| Host    | Exit | Commit in the clone     | Session in the operator's `session_v2` | Lock line | Service after                            | Log                                                |
| ------- | ---- | ----------------------- | -------------------------------------- | --------- | ---------------------------------------- | -------------------------------------------------- |
| Windows | 0    | `Add probe.txt`         | `ses_dedbcc34affeF8M5zZbf2LsepM`       | none      | PID 44232 alive, the second run exit 0   | `docs/spike/live/opencode-1009-d6d-win-checks.log` |
| WSL     | 0    | `Add probe.txt with ok` | `ses_dedb2be85ffel38Ek8kfqHZ9dV`       | none      | PID 2216799 alive, the second run exit 0 | `docs/spike/live/opencode-1009-d6d-wsl-checks.log` |

All 🔬. The standalone server and the service wrote one database at the same time with no
`SQLITE_BUSY` and no lock line.

**D6d answer (Windows):** pass — a 2.3.2 `--standalone` run on the operator's state exited 0, committed in its clone, wrote its session to the operator's database, and the service PID 44232 still answered a second run (`docs/spike/live/opencode-1009-d6d-win-standalone.log`, `docs/spike/live/opencode-1009-d6d-win-checks.log`, `docs/spike/live/opencode-1009-d6d-win-default-after.log`).

**D6d answer (WSL):** pass — the same on WSL: exit 0, the commit, the session in the operator's database, no lock line, and the service PID 2216799 answered a second run (`docs/spike/live/opencode-1009-d6d-wsl-standalone.log`, `docs/spike/live/opencode-1009-d6d-wsl-checks.log`, `docs/spike/live/opencode-1009-d6d-wsl-default-after.log`).

## D6e: `opencode models --standalone`

| Host    | Exit | Lines | New process after | Log                                                      |
| ------- | ---- | ----- | ----------------- | -------------------------------------------------------- |
| Windows | 0    | 36    | none              | `docs/spike/live/opencode-1009-d6e-win-models.log`       |
| WSL     | 1    | 0     | none              | `docs/spike/live/opencode-1009-d6e-wsl-models-first.log` |
| WSL     | 0    | 32    | none              | `docs/spike/live/opencode-1009-d6e-wsl-models.log`       |

All 🔬. The first WSL call failed with `ServiceUnavailableError: Location plugins did not
finish activating`; the next call, a minute later, passed. A plain `opencode models` on WSL
exited 0 with the same list (`docs/spike/live/opencode-1009-d6e-wsl-models-default.log`).
The flag is accepted: `models` does not refuse it, and no service process started.

**D6e answer:** accepted — `opencode models --standalone` exited 0 on Windows and on WSL (a first WSL call exited 1 with "Location plugins did not finish activating", the retry exited 0), and no new service process appeared (`docs/spike/live/opencode-1009-d6e-win-models.log`, `docs/spike/live/opencode-1009-d6e-wsl-models.log`).

## D3c: the deny map under `--auto`

The config was built as `opencode_config` in `crates/ralphy-agent-opencode/src/command.rs`
builds it: `{"permission":{"bash":{...}}}` with `"<words>"` and `"<words> *"` set to `deny`
for each of the nine entries of `DENIED_FORGE_WRITES`, passed as `OPENCODE_CONFIG_CONTENT`.
Windows, 2.3.2, a scratch `XDG_*` tree.

`opencode debug config --standalone` exited 0 and listed the 18 rules under
`permissions` as `{"action": "shell", "resource": "<words>", "effect": "deny"}` 🔬: the
1.x key `permission.bash` is read as the 2.x `shell` action.

**D3c config answer:** pass — `opencode debug config` of 2.3.2 on Windows lists all 18 forge-write rules (9 commands, each bare and as `<words> *`) as `action: shell`, `effect: deny` (`docs/spike/live/opencode-1009-d3c-win-debug-config.log`).

Then `opencode run --standalone --auto ...`, prompt "Run the shell command
`git push --dry-run origin HEAD` and tell me its output.". The model called the tool
twice; each call ended `status: error`, `error: "Permission denied: shell"`, tool `shell`
🔬. The run exited 0.

**D3c refusal answer:** pass — under `--standalone --auto` both `git push --dry-run origin HEAD` calls were refused with "Permission denied: shell", tool name `shell` (`docs/spike/live/opencode-1009-d3c-win-refusal.log`).

## Fixture sessions

Each database was written by the real version on Windows, prompt "Reply with the word ok.",
model `opencode/big-pickle`, in a fresh clone; then a copy was dumped with Python `sqlite3`
`iterdump()`: the schema of every table, and the rows of `session`, `message`,
`session_v2` and `session_message` only. The scratch root is `<scratch>`. Runs:
`docs/spike/live/opencode-1009-fixture-win-v1.log`,
`docs/spike/live/opencode-1009-fixture-win-v2-fresh.log`,
`docs/spike/live/opencode-1009-fixture-win-v2-upgraded.log`.

The sums are over the assistant rows of each table (1.x `message` with `role = 'assistant'`;
2.x `session_message` with `type = 'assistant'`), computed with SQL on the loaded dump.

| File              | Table             | Session id                       | Version                   | input | output | reasoning | cache.read | cache.write | Model        | Session folder           |
| ----------------- | ----------------- | -------------------------------- | ------------------------- | ----- | ------ | --------- | ---------- | ----------- | ------------ | ------------------------ |
| `v1.sql`          | `message`         | `ses_dedb89c6affe2Bf0J714lbuYfg` | 1.19.0                    | 23840 | 3      | 14        | 0          | 0           | `big-pickle` | `<scratch>/repos/fx-v1`  |
| `v2-fresh.sql`    | `session_message` | `ses_dedb6c267ffe6qnJHwBc9ot2DX` | 2.3.2                     | 10217 | 12     | 2         | 4096       | 0           | `big-pickle` | `<scratch>/repos/fx-v2f` |
| `v2-upgraded.sql` | `message`         | `ses_dedb89c6affe2Bf0J714lbuYfg` | 1.19.0                    | 23840 | 3      | 14        | 0          | 0           | `big-pickle` | `<scratch>/repos/fx-v1`  |
| `v2-upgraded.sql` | `session_message` | `ses_dedb89c6affe2Bf0J714lbuYfg` | 1.19.0, migrated by 2.3.2 | 23840 | 3      | 14        | 0          | 0           | `big-pickle` | `<scratch>/repos/fx-v1`  |
| `v2-upgraded.sql` | `session_message` | `ses_dedb7a25dffeoEMhN9M3UVV92v` | 2.3.2                     | 13981 | 3      | 13        | 1536       | 0           | `big-pickle` | `<scratch>/repos/fx-v2u` |

Row counts: `v1.sql` 1 `session`, 2 `message`; `v2-fresh.sql` 1 `session_v2`, 3
`session_message`; `v2-upgraded.sql` 1 `session`, 2 `message`, 2 `session_v2`, 5
`session_message` 🔬.

**The 2.x migration copies the old session into the new tables** 🔬: in `v2-upgraded.sql`
the 1.x session `ses_dedb89c6...` is in `message` and also in `session_message` (and
`session_v2`), with the same tokens. This corrects Q6 of `state-and-reads.md` ("no session
is in both"), which was read from the operator's database. A reader that sums both sets of
tables counts a migrated session twice; it must count each session id once.

**A 1.19.0 database already has a `session_message` table** 🔬, with the same columns as
2.3.2 (`id, session_id, type, seq, time_created, time_updated, data`) and no row
(`v1.sql`; Q6 of `state-and-reads.md` lists it as "0 rows"). A reader that picks the
table by "does `session_message` exist" takes a 1.x database for a 2.x one; it must read
the rows of both tables, or test for `session_v2`, which 1.19.0 does not have.
