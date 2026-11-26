# Live acceptance: OpenCode 1.x and 2.x on Windows and WSL (#1009, #1041, #1042)

Date: 2026-11-26 (UTC). Binary: `ralphy` built from `feat/1011-opencode-v2` at `0fc87ee7`
(`cargo build -p ralphy-cli --locked`; on WSL from a clone of the same commit).
Hosts: Windows 11 and WSL Ubuntu-22.04. Lab: `<user>/FinCal`, one temporary issue per run
(#134 to #137), closed after the check. Each run worked in its own clone under
`<root>/repos/`, never in `C:\Dev\FinCal`, with `--branch-mode current` (no push, no pull
request). `<root>` is `%TEMP%\ralphy-1009-e2e\` on Windows and `/tmp/ralphy-1009-e2e/` on WSL.

- OpenCode 1.x: a scratch `opencode-ai@1.19.0` first on `PATH`, with `XDG_*` under
  `<root>/xdg-v1/`. Model `opencode/deepseek-v4.1-flash-free`.
- OpenCode 2.x: the operator's 2.3.2 install and state. The free models were over their
  usage limit on 2.x, so the 2.x runs used the operator's provider:
  `anthropic/claude-haiku-5-5`. The 2.x runs were started from `<root>/elsewhere` (an
  empty folder that is no git tree) with `PWD` set to it, and `--repo` naming the clone.

The command line of each run is the first line of its log.

| Cell | Host | Version | Result | Log |
| --- | --- | --- | --- | --- |
| run | windows | 1.19.0 | Done | docs/spike/live/opencode-1009-e2e-run-win-v1.log |
| run | windows | 2.3.2 | Done | docs/spike/live/opencode-1009-e2e-run-win-v2.log |
| run | wsl | 1.19.0 | Done | docs/spike/live/opencode-1009-e2e-run-wsl-v1.log |
| run | wsl | 2.3.2 | Done | docs/spike/live/opencode-1009-e2e-run-wsl-v2.log |
| pwd | windows | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-pwd-win-v2.log |
| pwd | wsl | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-pwd-wsl-v2.log |
| state | windows | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-state-win-v2.log |
| state | wsl | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-state-wsl-v2.log |
| doctor | windows | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-doctor-win.log |
| doctor | wsl | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-doctor-wsl.log |
| tokens | windows | 2.3.2 | pass | docs/spike/live/opencode-1009-e2e-ledger-win.log |

What each row rests on:

- **run**: the run ends with `issues_json=[{"number":N,"status":"done"}]` and one commit
  that adds `smoke/e2e-<cell>.txt` in the clone. 🔬
- **pwd**: after the 2.x run, `<root>/elsewhere` is still empty and is no git tree, and
  the commit is in the 2.x clone only. 🔬
- **state**: the run dir's `agent-status.jsonl` starts with `plugin.loaded` and ends with
  `session.idle`, and the run dir holds `ralphy-state-plugin-v2/`. 🔬
- **doctor**: `ralphy doctor` prints `[ok  ] opencode: 2.3.2`. 🔬
- **tokens**: the ledger lines of #135 (plan and execute, model `claude-haiku-5-5`) have
  tokens above zero. The WSL 2.x lines (#137) are above zero too
  (`opencode-1009-e2e-ledger-wsl.log`). 🔬

The first WSL 1.x run of #136 ended `skipped`, not `Done`: the free model wrote a
`## Verify` line with backslash-escaped quotes, which the verify gate refuses
(`opencode-1009-e2e-run-wsl-v1-first.log`). The adapter part of that run ended `Done`.
A second run of the same issue, in a clone reset to `origin/master`, ended `Done`.

Not checked here: the 1.x ledger lines (#134 and #136) have zero tokens and model
`unknown`. With `XDG_*` under `<root>/xdg-v1/`, the reader did not find the 1.x session.
No check of this plan covers it; it is listed in the stage report.
