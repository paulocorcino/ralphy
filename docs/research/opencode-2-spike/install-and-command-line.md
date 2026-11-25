# OpenCode 2: install, version and command line

Spike of #1010, questions Q1 and Q2. Plan stage 3. Markers are in the
[README](README.md). Raw output is in `docs/spike/live/opencode-1010-install-*.log`.

Versions probed: OpenCode 1.19.0 (scratch install) and OpenCode 2.3.2 (the
operator's install), on Windows 11 and in WSL Ubuntu-22.04. macOS is documented
only.

## Q1: where each version installs and how Ralphy finds it

### Installers

| Version | Way to install                                         | Package or release                                   | Folder it writes                                       | Shim                                                 |
| ------- | ------------------------------------------------------ | ---------------------------------------------------- | ------------------------------------------------------ | ---------------------------------------------------- |
| 1.x     | `curl -fsSL https://opencode.ai/install \| bash` 🔎    | GitHub release archive, asset `opencode-<os>-<arch>` | `~/.opencode/bin/opencode`                             | none                                                 |
| 1.x     | `npm install -g opencode-ai` 🔬                        | npm `opencode-ai` (1.19.6 is the latest 1.x)         | npm global folder                                      | `opencode`                                           |
| 2.x     | `curl -fsSL https://opencode.ai/v2/install \| bash` 🔎 | npm tarball of `@opencode/cli-<target>`              | `~/.opencode/bin/opencode` (`opencode.exe` on Windows) | `opencode2` next to it                               |
| 2.x     | `npm install -g @opencode/cli` 🔬                      | npm `@opencode/cli` (2.4.1 is the latest)            | npm global folder                                      | `opencode` and `opencode2`, both run the same binary |

- The 1.x script refuses a 2.x version: it prints "this installer only installs
  OpenCode 1" and names the 2.x script (`install-script-v1.log`) 🔎.
- The 2.x script writes `~/.opencode/bin` and both `opencode` and `opencode2`
  there. `opencode2` is a two-line shell script (`opencode2.cmd` on Windows) that
  runs the `opencode` next to it (`install-script-v2.log`) 🔎. It is called a
  "legacy shim" in the script.
- Both scripts append `export PATH=$HOME/.opencode/bin:$PATH` to the shell
  config file of the user (`.bashrc`, `.zshrc`, ...) 🔎. The Windows install of
  the operator did not use a script: it is `npm install -g @opencode/cli`, and
  `%APPDATA%\npm` holds `opencode.cmd`, `opencode2.cmd` and their `.ps1` and
  extension-less files 🔬.
- A third binary exists on the Windows host: the desktop app folder
  `%LOCALAPPDATA%\opencode\opencode-cli.exe` is version 1.14.24. It is on no
  `PATH` 🔬 (`install-win-desktop.log`).
- One name, two products. On the operator's machines `opencode` is 2.x, and
  `opencode2` is the same 2.x binary. A user with the 1.x installer and the 2.x
  installer both has `~/.opencode/bin/opencode` rewritten by whichever ran last 🔎
  (both scripts write the same path).

### Version output

| Host    | Version     | Binary path                                                                                      | Command               | Output                 | Marker |
| ------- | ----------- | ------------------------------------------------------------------------------------------------ | --------------------- | ---------------------- | ------ |
| Windows | 1.19.0      | `%TEMP%\ralphy-1010\v1\node_modules\opencode-ai\bin\opencode.exe` (scratch, npm)                 | `opencode --version`  | `1.19.0`               | 🔬     |
| Windows | 2.3.2       | `%APPDATA%\npm\node_modules\@opencode\cli\bin\opencode.exe`, run by `%APPDATA%\npm\opencode.cmd` | `opencode --version`  | `opencode v2.3.2`      | 🔬     |
| Windows | 2.3.2       | the same, through `opencode2.cmd`                                                                | `opencode2 --version` | `opencode v2.3.2`      | 🔬     |
| WSL     | 1.19.0      | `/tmp/ralphy-1010/v1/node_modules/opencode-ai/bin/opencode.exe` (scratch, npm, an ELF file)      | `opencode --version`  | `1.19.0`               | 🔬     |
| WSL     | 2.3.2       | `~/.opencode/bin/opencode` (ELF, 253 MB)                                                         | `opencode --version`  | `opencode v2.3.2`      | 🔬     |
| WSL     | 2.3.2       | `~/.opencode/bin/opencode2` (shell script, 47 bytes)                                             | `opencode2 --version` | `opencode v2.3.2`      | 🔬     |
| macOS   | 1.x and 2.x | `~/.opencode/bin/opencode` (script) or the npm global folder                                     | `opencode --version`  | not run: no macOS host | 📖     |

- The two outputs have different shapes: 1.x prints the bare number, 2.x prints
  `opencode v<number>`. A check that parses `--version` must take the number
  from both forms (`install-win-v1.log`, `install-win-v2.log`, `install-wsl.log`).
- The version is in the output only. The binary file name is `opencode` in both.

### How Ralphy finds the binary today and on each host

- `resolve_program("opencode")` searches `PATH`, then `~/.local/bin`, then nvm.
  It knows nothing of `~/.opencode/bin`.
- Windows 🔬: `where opencode` returns `%APPDATA%\npm\opencode` and
  `opencode.cmd`. The npm folder is on `PATH`, so the search finds the 2.x shim.
- WSL 🔬: `~/.opencode/bin` is added to `PATH` only by an interactive part of
  `~/.bashrc`. A login shell that is not interactive (`bash -l`, as Ralphy
  spawns it) does not have it: `which -a opencode` finds only the Windows npm
  shims under `/mnt/c/...`. A search by `PATH` finds no Linux `opencode`, or the
  wrong one (a Windows `.cmd` shim from `/mnt/c`). So the second folder of the
  locator (`~/.opencode/bin`) is required on WSL (`install-wsl.log`).
- macOS 📖: the installer writes `~/.opencode/bin` and edits `.zshrc` (it creates
  the file when it is missing). A process that does not read `.zshrc` has the
  same gap as WSL.
- Linux npm in WSL 🔬: `which npm` returns the Windows npm (`/mnt/c/nodejs/npm`).
  A Linux npm is only under nvm (`~/.nvm/versions/node/<v>/bin`).

**Q1 answer:** OpenCode 2 installs to `~/.opencode/bin/opencode` with an
`opencode2` shim next to it (2.x script, WSL), or as npm `@opencode/cli`, which
installs `opencode` and `opencode2` in the npm global folder (Windows). `opencode`
is the 2.x binary on both operator hosts. The `PATH` of a non-interactive WSL
shell does not include `~/.opencode/bin`, so the locator needs that folder as its
second step, as the plan says. macOS is 📖 only, with the same folder.

**Changes:**

- #1008: the locator order (`PATH`, `~/.local/bin`, nvm, `~/.opencode/bin`) is
  confirmed. Add the npm global folder to the doctor text, because on Windows
  the 2.x binary is only in `%APPDATA%\npm`.
- #1008: a version check reads `1.19.0` and `opencode v2.3.2`; it must find the
  number in both forms.
- #1009: a path chosen by `PATH` may be a 1.x or a 2.x binary. The adapter must
  ask `--version` and not assume it from the folder.

## Q2: the command line of `opencode run`

Probes: `opencode run --help` of both versions on both hosts
(`install-help-win-v1.log`, `install-help-win-v2.log`, `install-help-wsl.log`);
a refusal test with no usable model, `run <flags> -m nosuch/nosuch x`, which
stops at model lookup and sends nothing to a model (`install-flags-*.log`); two
short runs on a free model for the prompt on stdin (`install-stdin-win-*.log`);
and `debug config` for the environment variable (`install-config-win-*.log`).
2.x probes used `--standalone` and a scratch `XDG_*` tree, so the operator's
database and service were not touched. The Windows and WSL results are the same
in every row.

The 1.x parser is yargs in strict mode: an unknown flag prints the help and exits with code 1. The 2.x parser is a new one with the same exit: an unknown flag prints the
help of `run` and exits 1.

### Command line

| What `command.rs` passes                 | 1.19.0                                                                                                | 2.3.2                                                                                                                                                                                            | Marker |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------ |
| `run`                                    | `opencode run [message..]`                                                                            | `opencode run [flags] [<message...>]`; with no `--server` or `--standalone` it uses the background service                                                                                       | 🔬     |
| `--format json`                          | Accepted; choices `default`, `json`. Events carry `part.type`                                         | Accepted; choices `default`, `json`. The shape of the events is the subject of the stream stage                                                                                                  | 🔬     |
| `--dangerously-skip-permissions`         | Not in the help; accepted (the run goes on to model lookup)                                           | Not in the help; accepted the same way. The documented form is `--auto`                                                                                                                          | 🔬     |
| `--print-logs`                           | Accepted; logs go to stderr                                                                           | Accepted; the help says "server logs require `--standalone`"                                                                                                                                     | 🔬     |
| `--log-level ERROR`                      | Accepted; choices `DEBUG`, `INFO`, `WARN`, `ERROR`; lowercase `error` is refused                      | Refused: the help prints and the exit is 1. Choices are lowercase `all, trace, debug, info, warn, warning, error, fatal, none`; `error` is accepted                                              | 🔬     |
| `-m <provider/model>`                    | Accepted, `provider/model`                                                                            | Accepted, `provider/model#variant`                                                                                                                                                               | 🔬     |
| `--variant <v>`                          | Accepted (`--variant high` reaches model lookup)                                                      | Refused: the help prints and the exit is 1. The variant goes after `#` in `-m`; `-m nosuch/nosuch#high` is accepted                                                                              | 🔬     |
| the prompt on stdin                      | `echo "..." \| opencode run ...` is read as the message; the free model answered `OK` (24 058 tokens) | The same; the free model answered `OK` (15 762 tokens)                                                                                                                                           | 🔬     |
| `OPENCODE_CONFIG_CONTENT`                | Honored. `debug config` shows the JSON merged with source `OPENCODE_CONFIG_CONTENT`                   | Honored. `debug config` shows a normalized document: `permission.bash` becomes `permissions: [{action: "shell", resource, effect}]`, `skills.paths` becomes `skills`, `plugin` becomes `plugins` | 🔬     |
| the working folder (`current_dir(root)`) | The session `directory` in the database is the folder of the process. `--dir` is accepted             | The session `directory` in `session_v2` is the folder of the process. `--dir` is refused; there is no flag for the folder                                                                        | 🔬     |

Other observations:

- **A 2.x run needs a server.** With no flag, `run` connects to the managed
  background service. The service listens on a fixed port, 28894, per machine:
  a second `opencode` with a different `XDG_*` tree that wants its own service
  fails with "Managed service port 28894 on 127.0.0.1 is already in use"
  (`install-config-win-v2.log`). `--standalone` starts a private server. Whether
  Ralphy should use the service or a private server is for the server stage.
- **Errors differ.** For a model that does not exist, 1.x ends with the opaque
  `{"name":"UnknownError", ... "ref":"err_..."}` and the real cause only in the
  `--print-logs` text (`ProviderModelNotFoundError`). 2.x says
  `Model unavailable: nosuch/nosuch` and in `--format json` an error event
  `{"type":"error", ..., "error":{"type":"provider.no-route","message":...}}`.
- **The level is not applied to server logs.** With `--standalone --print-logs
  --log-level error`, 2.x still printed `level=INFO` lines
  (`install-flags-win-v2.log`, `install-flags-wsl-v2.log`). The quiet stderr that
  the 1.x flags give (`ERROR` only) is not reproduced; the limit detector that
  reads stderr gets more lines.
- **Help gives `--auto` in both versions.** 1.19.0 already has `--auto`
  ("auto-approve permissions that are not explicitly denied") next to the old
  hidden flag. The explicit deny map still works under the old flag in 2.x
  (see `state-and-reads.md`, Q7).
- The new 2.x `run` flags `--archived` ("create the session archived so it stays
  out of session lists (useful for scripted or CI runs)") and `--title` exist
  and have no 1.x twin; `--worktree`, `--file`, `--agent`, `--thinking` are the
  same or new. None is used today.

**Q2 answer:** `opencode run`, `--format json`, `-m`, `--print-logs`, the prompt
on stdin, `OPENCODE_CONFIG_CONTENT` and the process folder work on 2.3.2 as on
1.19.0, and 2.x still accepts the hidden `--dangerously-skip-permissions`. Two
flags that Ralphy passes break: `--variant` (2.x refuses it; use `-m
provider/model#variant`) and `--log-level ERROR` (2.x takes only lowercase). The
quiet-stderr effect of `--log-level` was not seen on 2.x.

**Changes:**

- #1009: for 2.x, build `-m provider/model#variant` and never `--variant`.
- #1009: for 2.x, pass `--log-level error` (lowercase); for 1.x keep `ERROR`.
  The case is a per-version fact: it belongs in one function with the version.
- #1009: prefer `--auto` on 2.x and keep `--dangerously-skip-permissions` only
  for 1.x. The old flag works on 2.3.2 but is in no help text, so a later 2.x
  can remove it with no notice.
- #1009: do not rely on `--log-level` for a quiet stderr on 2.x. The limit
  detector must tolerate INFO lines.
- New issue under #1011: the spike found no flag for the working folder in 2.x
  `run`; confirm in the server stage that a run through the background service
  uses the folder of the client, and not the folder of the service.

### Live-probe cost

| Probe                                                              | Host         | Version       | Model                               | Tokens                                                                 |
| ------------------------------------------------------------------ | ------------ | ------------- | ----------------------------------- | ---------------------------------------------------------------------- |
| help, version, install scripts, flag refusals (`-m nosuch/nosuch`) | Windows, WSL | 1.19.0, 2.3.2 | none (stops at model lookup)        | none                                                                   |
| `debug config`, `debug paths`                                      | Windows      | 1.19.0, 2.3.2 | none                                | none                                                                   |
| the prompt on stdin                                                | Windows      | 1.19.0        | `opencode/deepseek-v4.1-flash-free` | 24 058 total (cost 0)                                                  |
| the prompt on stdin                                                | Windows      | 2.3.2         | `opencode/deepseek-v4.1-flash-free` | 15 762 total, from input 13 967 + cache read 1 792 + output 3 (cost 0) |
