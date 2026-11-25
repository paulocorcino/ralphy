# OpenCode 2 spike

Facts about OpenCode 2 that the ADR and the two issues #1008 and #1009 stand on.
This is the spike of #1010, part of #1011. Each file answers some questions from
the probes of the machine, with the raw output in `docs/spike/live/`. Nothing here
is an instruction for the code: the decision is in the ADR.

## Platform table

| Host             | Version                             | Binary                                                                                                                   | Marker |
| ---------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ------ |
| Windows 11       | 2.3.2 (operator)                    | `%APPDATA%\npm\node_modules\@opencode\cli\bin\opencode.exe`, started by `%APPDATA%\npm\opencode.cmd` and `opencode2.cmd` | 🔬     |
| Windows 11       | 1.19.0 (scratch)                    | `%TEMP%\ralphy-1010\v1\node_modules\opencode-windows-x64\bin\opencode.exe`                                               | 🔬     |
| Windows 11       | 1.14.24 (desktop app, on no `PATH`) | `%LOCALAPPDATA%\opencode\opencode-cli.exe`                                                                               | 🔬     |
| WSL Ubuntu-22.04 | 2.3.2 (operator)                    | `~/.opencode/bin/opencode`, and the `opencode2` shell script next to it                                                  | 🔬     |
| WSL Ubuntu-22.04 | 1.19.0 (scratch)                    | `/tmp/ralphy-1010/v1/node_modules/opencode-linux-x64/bin/opencode`                                                       | 🔬     |
| macOS            | not probed                          | `~/.opencode/bin/opencode` by the installer scripts                                                                      | 📖     |

The latest releases on the day of the probes: 2.4.1 (`@opencode/cli`) and 1.19.6
(`opencode-ai`). The operator's 2.3.2 was not updated.

## Evidence markers

Every fact has the tool, the version and the OS, and one marker:

- 🔬 observed: a command ran; the output is quoted or is in a raw log.
- 🔎 read from the binary, a script or a file on disk.
- 📖 documented only: a page of the vendor, not run. macOS is always this one.

## Scratch rules

- Scratch roots: `%TEMP%\ralphy-1010\` on Windows and `/tmp/ralphy-1010/` in WSL.
  No scratch file is written under `/mnt/c/` from WSL.
- OpenCode 1.x is a scratch npm install (`opencode-ai@1.19.0`). Every 1.x command
  runs with `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and
  `XDG_CACHE_HOME` under `<root>/xdg-v1/`, so 1.x never opens the 2.x database.
- OpenCode 2.x is the operator's install, called by absolute path. It is never
  updated, reinstalled or logged out. A probe of 2.x that is not about the
  operator's state uses `--standalone` and a scratch `<root>/xdg-v2/` tree.
- The database of the operator is read through a copy of its three files.
- Probes run in clones of the lab repo under `<root>/repos/`, never in the lab
  repo itself.
- Raw logs hold `<redacted>` in place of tokens, keys, account ids and e-mails,
  and `<home>` and `<user>` in place of the home folder and the user name.

## Spike files

- [Install, version and command line](install-and-command-line.md): Q1 and Q2.
- [State on disk and what Ralphy reads](state-and-reads.md): Q6 and Q7.
- `stream-and-outcomes.md`: added by the stream stage.
- `server-and-console.md`: added by the server stage.

## Summary

_Stage 6_.

## Change list

_Stage 6_.
