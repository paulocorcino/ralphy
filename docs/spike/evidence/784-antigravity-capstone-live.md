# Live capstone: `ralphy run --agent antigravity` (#784)

The results of the runbook
[784-antigravity-capstone-runbook.md](./784-antigravity-capstone-runbook.md).
One `- <topic>: ...` line per finding. The raw logs are
`docs/spike/live/antigravity-784-capstone-*.log` (redacted: no OAuth value,
token or email; `<home>` is the user folder). The decisions are in
[ADR-0086](../../adr/0086-antigravity-adapter.md); its `## Measured in #784`
section has one line per open item.

Windows host: Windows 11 Pro 10.0.26300, `agy` 1.5.8 (1.5.7 at the logout
measure), owned root `<home>\.ralphy\antigravity-root`, date 2026-11-13.
Binary: `target/debug/ralphy.exe` built from `feat/784-capstone` at main
`6492c8e5` (`cargo build --locked -p ralphy-cli --bin ralphy`). Model:
`gemini-3.8-flash-low` for every call Ralphy chose, except where a line says
otherwise. Lab: `paulocorcino/FinCal` in fresh temporary clones.

WSL host: Ubuntu 22.04.5 LTS on WSL 2 (kernel 6.6.87.2) on the same machine,
`agy` 1.5.8, owned root `<home>/.ralphy/antigravity-root` (the operator signed
in there once), date 2026-11-13. Binary: `<home>/p1lint/target/debug/ralphy`,
a debug build of `feat/784-capstone` at `a439c946`.

## Windows

- Windows: issue #131 closed by ralphy run --agent antigravity (`--issues 131
  --plan-model gemini-3.8-flash-low --exec-model gemini-3.8-flash-low`),
  run `20261113-024834`, exit 0 in 223 s: plan 54 s (4 open steps), execute
  52 s (`outcome=Done exit_code=Some(0) committed=true`), the verify gate
  passed (2 commands), "green — issue closed", knowledge consolidated in
  97 s. The delivery commit is on a local branch `afk/run-20261113-024834`
  of the temp clone; nothing was pushed. Log:
  `antigravity-784-capstone-win-run.log`.
- Version line: each session writes `antigravity: agy version 1.5.8` to the
  run log before it starts (plan, execute, consolidation).
- Proof turn: every session's status file starts with `SessionStart`, about
  3 s after the launch (consolidation of run `20261113-024834`: launch
  02:50:39, `SessionStart` 02:50:42.085), then the task. No session was
  refused.
- Guard decisions: in the status files, each `PreToolUse` (`run_command`,
  `read_file`, `write_file`) is followed by its `PostToolUse`, so the guard
  allowed every tool call of these runs; no deny happened, because no run
  asked for a denied command. The guard writes no line of its own to the
  status file; an allow is read from the `PostToolUse` that follows.
- Status file: `agent-status.jsonl` in the run folder is reset by each new
  session, so after a run it holds only the last session (the
  consolidation). The plan and execute lines are gone. The stream log
  `antigravity.log` is also overwritten per session: it holds only the
  execute session.
- Usage: stream sum equals store sum for each of the five Windows sessions
  measured: the sum of the `step_update` `agent_response` `DONE` usage
  against the sum of the `gen_metadata` rows of
  `<root>/antigravity-cli/conversations/<id>.db` (input, output, cache read,
  and the number of calls). The execute session of #131: 11 calls,
  59 790 in, 1 736 out, 122 610 cache read in both, and the run log's
  `up=59790 cr=122610 out=1736` is the same. Log:
  `antigravity-784-capstone-win-usage-store.log`.
- Consolidation model: the knowledge consolidation at the end of a green
  `ralphy run` passes no `--model` (`consolidate_defaults` gives `None` for
  `antigravity`), so `agy` used its default, `Gemini 4 Argon (Medium)` by the
  store label (33 978 in, 875 out, 104 042 cache read), not the run's Flash
  model. `ralphy run` has no flag to set it. The run summary then logs
  `some tokens had no model attribution — not priced (shown as +?)`.
- Pricing at the end of a run: the run summary logs `unknown model — add
  gemini-3.8-flash-low to pricing.toml to price it`. The price table has a
  `gemini-3.8-flash` row, and `ralphy_pricing::price_key("antigravity", ..)`
  strips the effort suffix, but the run summary prices the raw id, so the
  Flash tokens of the run are not priced.
- Windows hooks: SessionStart seen from the hooks file Ralphy wrote in
  `<root>/config/hooks.json`, in every session; all six events fired
  (`SessionStart`, `PreInvocation`, `PreToolUse`, `PostToolUse`,
  `PostInvocation`, `Stop`).
- Program path with a space: `ralphy.exe` copied to
  `<tmp>\agy784\ralphy space\ralphy.exe` and run as `ralphy consolidate
  --agent antigravity --model gemini-3.8-flash-low`: Ralphy wrote the hooks
  with the 8.3 short path `<tmp>\agy784\RALPHY~1\ralphy.exe hook guard
  --vendor antigravity`, unquoted; `SessionStart` came, and 15 guarded tool
  calls (`run_command`, `read_file`) ran, each `PreToolUse` with its
  `PostToolUse`; exit 0 in 54 s. Log: `antigravity-784-capstone-win-space.log`.
- Skills: `swarm` is still listed by `agy -p /skills` in the owned root,
  although `config/config.json` holds `{"plugins":{"swarm":{"enabled":false}}}`.
  The list is `agy-customizations`, `antigravity-guide`, `automation`,
  `migrate-agent-settings`, `migrate-workflows`, `reviewer`, `run-workflow`,
  `setup-pocock`, `staged-plan`, `swarm`. So the setting does not take
  `swarm` out of the skill list; whether the plugin's tools still run was
  not checked. Log: `antigravity-784-capstone-win-skills-quota.log`.
- Git Bash trap: `agy -p /skills` without `MSYS_NO_PATHCONV=1` sends
  `C:/Program Files/Git/skills` as a prompt, and the model answers it. Two
  such calls ran on the default model by mistake (the first `/quota` used
  15 070 input tokens). Ralphy itself spawns `agy` with no shell, so a run
  is not affected.
- Limit: observed on Windows, `agy` 1.5.8, 2026-11-13, measured by the
  parent agent with the operator's approval to spend the most expensive
  model. Five calls of `claude-opus-5-5-high` (each about 26 000 to 33 000
  input and 20 000 to 22 000 output tokens, about 3.5 min) took the
  `Claude and GPT models` 5 h bucket from 1.0 to 0 and its weekly bucket
  from 1.0 to 0.46; the `Gemini Models` group stayed at about 0.97 to 0.98.
  The sixth call: exit 3 after 4.5 s, JSON `status: "ERROR"`,
  `finished_reason: "error"`, `error: "Individual quota reached. Please
  upgrade your subscription to increase your limits. Resets in 4h41m14s."`,
  usage all 0, and on stderr `error: Individual quota reached. ...` then
  `AGY_ERROR: {"short_error":"RESOURCE_EXHAUSTED (code 429): ...",
  "status":"RESOURCE_EXHAUSTED","error_code":429,"code_kind":"http",
  "retryable":true,...}`. Then `ralphy run --agent antigravity --only-issue
  110 --plan-model claude-opus-5-5-high` classified it as a limit with a
  reset: `ralphy.log` has `waiting for usage-limit reset remaining_min=280`,
  the `reset_time` of the 5 h bucket read from `/quota`, and the run waited.
  The parent stopped it after 10 min; nothing was posted on GitHub. The
  reset comes from `/quota`: the stderr `Resets in ...` text is not parsed.
  The limit is per model group: no `Gemini Models` bucket was at 0. Log:
  `antigravity-784-capstone-win-limit.log`.
- Two runs at once in one root: two `ralphy consolidate --agent antigravity
  --model gemini-3.8-flash-low` started in the same second in two clones,
  both with the owned root. Both exit 0 (56 s and 126 s), each with its own
  conversation id, its own store file, its own status file with one
  `SessionStart`, and stream sum equal to store sum for each.
  `cache/last_conversations.json` is keyed by the working folder and holds
  both. The pair was two consolidation sessions, not two full `ralphy run`s,
  so that no call used the default model (see _Consolidation model_). Logs:
  `antigravity-784-capstone-win-concurrent.log`.
- Prompt of 72 KB: one NDJSON line of 74 936 bytes on stdin
  (`--input-format stream-json`, isolated root, Flash) was read whole: exit 0
  in 7 s, 27 573 input tokens, and the answer `01010` is the number of the
  last of the 1 010 lines in the prompt. Log:
  `antigravity-784-capstone-win-big-prompt.log`.
- Updater: `agy` starts `--bg-updater` at most once per 15 minutes per root
  (`Last check was less than 15 minutes ago, skipping update (fast path)`).
  In a Ralphy session after the window (03:10:33), the updater (pid 3224)
  had the session's `agy` as its parent, so the Windows tree kill, which
  walks the table by parent pid, reaches it while it lives. It ended by
  itself after 5 s with `Already on the latest version.`, 57 s before the
  session ended, so no kill met a live updater. An updater that is
  replacing the binary needs a new release (and `agy update` is not run), so
  the kill of an updater in the middle of a replace was not measured.
  `agy --version` ran after the session (1.5.8, exit 0). Log:
  `antigravity-784-capstone-win-updater.log`.
- Upload A/B: two fresh git repositories (3 and 201 files), each in a fresh
  root, one Flash run each with two tool calls (`run_command` `git
  ls-files`, `read_file`). Both answered right (3 and 201 files). `agy`'s own
  log names only `loadCodeAssist`, `fetchAvailableModels` and
  `streamGenerateContent` on `daily-cloudcode-pa.googleapis.com`, the same
  in A and B; no upload, sync, index or embedding line, and no line that
  grows with the repository. This is the log only, not a network capture;
  no opt-out switch exists to compare against. Log:
  `antigravity-784-capstone-win-upload-ab.log`.
- Remote control flag: each fresh root logs `[RemoteControl] Mendel flags
  changed (remote-control-setting-enabled=true,
  remote-control-proxy-server-url="jetski-webchannel.googleapis.com:443")`.
  The server turns the remote-control setting on by a flag; Ralphy still
  passes no `--remote-control`, and no take-over link was printed.
- Safety monitor: `agy`'s log of a Ralphy session has `Monitor
  sir_flash_lite FINISHED: triggered=false, reason=...`: a vendor monitor
  reviews what the agent does. Its calls are not in the stream usage.
- Models: `ralphy models --agent antigravity` lists 18 ids under `agy`
  1.5.8. New since 1.5.6: `gemini-4-hydrogen-low`, `-medium`, `-high`,
  `-xhigh` and `gemini-pro-orchestrator-g1-pro` ("Orchestrator (Argon)").
  The price table has no row for them, and `ralphy_pricing::price_key` does
  not strip the `-xhigh` suffix. Log: `antigravity-784-capstone-win-models.log`.
- Doctor: `ralphy doctor` prints `[warn] antigravity hooks: not checked: the
  check gave no result`, signed in and signed out alike.
- Windows logged out: exit 1, stopped after 1 s (`ralphy models --agent
  antigravity`), and a `ralphy run --agent antigravity --only-issue 108`
  stopped about 1.5 s after planning started, exit 1, against the fixed
  60 s of `agy`. Measured by the parent agent on 2026-11-13 02:33-02:38,
  `agy` 1.5.7, while the operator was signed out with `/logout`:
  - `agy models`: exit 1, `Error: Please sign in to view available models.
    Launch the CLI without arguments to sign in.`
  - `agy -p "Reply OK" --model gemini-3.8-flash-low --output-format
    stream-json`: stderr `Authentication required. Please visit the URL to
    log in:` at once, then `Waiting for authentication (timeout 60s)...`;
    exit 1 after 60.6 s; one `result` line with `status:"ERROR"`,
    `error:"authentication failed or timed out"`. With `--print-timeout
    20s` the wait is still 61.0 s. The same as on WSL in the spike.
  - `ralphy models --agent antigravity`: exit 1 in 1.0 s, `Antigravity is
    not signed in for Ralphy. Run agy --gemini_dir="<home>\.ralphy\antigravity-root"
    with no other argument, sign in, then run Ralphy again.`
  - `ralphy run`: the queue built, planning started at 02:38:35, and the
    run ended with `Error: the prompt was held and not sent`, caused by the
    same sign-in message; exit 1 at 21.2 s of wall time, most of it the
    clone and queue. Nothing was written on GitHub.
  - `ralphy doctor` does not report the signed-out state (exit 0, the
    version line and the hooks warn above).
  - The operator signed back in; the runs above (02:48 on) all worked.
  Logs: `antigravity-784-capstone-win-logout.log`,
  `antigravity-784-capstone-win-logout-run.log`.
- Cooperative stop on Windows: not a gap. Phase A Stage 3 shipped the
  Windows half (`CTRL_BREAK_EVENT`), proved in a process with a console and
  one without; its tree-kill fallback was not taken.
- Vendor terms: <https://antigravity.google/terms> ("Google Antigravity -
  Terms of Service", no date on the page), read 2026-11-13. Data use
  (section 5): "We use Interactions to evaluate, develop, and improve Google
  and Alphabet research, products, services and machine learning
  technologies"; "If you don't want your Interactions used in this way,
  navigate to settings to change your preference"; with telemetry off,
  Google still uses the data to provide the service. Automation
  (section 6): "You must not abuse, harm, interfere with, or disrupt the
  Service. This includes, but is not limited to, using the Service in
  connection with products not provided by us. Using third party software,
  tools, or services to access the Service (e.g. using OpenClaw with
  Antigravity OAuth) is a breach of this Agreement", which "may be grounds
  for suspension or termination of your Antigravity and/or Gemini CLI
  accounts". Ralphy runs Google's own `agy` binary and never reads or
  reuses its OAuth token, but "in connection with products not provided by
  us" can be read to cover a tool that drives `agy`. This is a decision for
  the operator, not a code change.

## WSL

- WSL: issue #132 closed by ralphy run --agent antigravity (`--issues 132
  --base-branch master --branch-mode new --plan-model gemini-3.8-flash-low
  --exec-model gemini-3.8-flash-low --no-telegram`), run `20261113-032212`,
  exit 0 in 208 s: plan 42 s (3 steps), execute 62 s (`[ok] #132 done`),
  knowledge consolidated in 92 s. The issue is closed as completed with
  Ralphy's comments; the delivery commit `00447c4` is on the local branch
  `afk/run-20261113-032212` of the temp clone, and nothing was pushed. Each
  session wrote `antigravity: agy version 1.5.8` first. Log:
  `antigravity-784-capstone-wsl-run.log`.
- Usage on WSL: stream sum equals store sum for the execute session (14
  calls, 80 970 in, 1 537 out, 160 803 cache read) and for the
  consolidation (11 calls, 34 935 in, 733 out, 102 164 cache read), and the
  run log's `up=80970 cr=160803 out=1537` is the same. The plan session's
  stream was overwritten by the execute session; its store (10 rows,
  53 449 in, 1 042 out, 123 233 cache read) equals the run log's `plan
  written up=53449 cr=123233 out=1042`. The consolidation again used
  `Gemini 4 Argon (Medium)` (no `--model`). Log:
  `antigravity-784-capstone-wsl-usage-store.log`.
- Linux hooks: SessionStart seen from the hooks file Ralphy wrote in
  `<root>/config/hooks.json`, whose commands are the single-quoted program
  path, for example `'<home>/p1lint/target/debug/ralphy' hook status
  antigravity`. `agy` runs each hook as `sh -c '<path>' hook ...`, with the
  session's `agy` as the parent (sampled: three `hook status`, one `hook
  guard --vendor antigravity`). `SessionStart` came 7.5 s after the launch of
  the execute session (03:23:01.05 to 03:23:08.56) and 7.8 s after the
  consolidation's; all six events fired, and each `PreToolUse` has its
  `PostToolUse` (10 of 10 in the consolidation), so the guard allowed every
  tool call. Log: `antigravity-784-capstone-wsl-hooks.log`.
- Process groups on Linux: Ralphy starts each `agy` session in its own
  process group (pgid = the `agy` pid) inside Ralphy's session.
- Updater on Linux: at 03:32:45, after the 15-minute window, the session's
  `agy` (pid 98034, pgid 98034) started `agy --bg-updater` (pid 98251) in a
  new session and group of its own (sid = pgid = 98251); it ran `agy
  --version` and ended by itself after 3.6 s. It is in neither the run's
  group nor the `agy` group, so the Unix tree kill, which sends `SIGKILL` to
  the group `agy` leads, does not reach it. Log:
  `antigravity-784-capstone-wsl-updater.log`.
- Stop by `kill -TERM <ralphy pid>`: not a stop of the agent. A run on the
  throwaway issue #133, `SIGTERM` to the `ralphy` pid 15 s into planning:
  `ralphy` exited 143 within 3 ms (`ralphy run` has no `SIGTERM` handler);
  `agy`, in its own group, got no signal, was moved under `/init`, finished
  its planning turn (it wrote `.ralphy/plan.md`; 9 calls, 51 108 in), with
  its hooks still firing, and ended by itself 27 s later. No run recorded
  that usage and nothing was posted on #133. Log:
  `antigravity-784-capstone-wsl-stop.log`.
- Stop by `ralphy stop` (the operator's stop): a third run on #133,
  `ralphy stop --repo .` 18 s into planning. The run logged `operator stop
  — reaping the headless child`, the adapter sent `SIGTERM` to the `agy`
  group, and `agy` wrote one `result` line with `status:"ERROR"`,
  `error:"interrupted"`, `finished_reason:"interrupted"` and the usage so
  far (38 369 in, 160 out, 16 423 cache read), and `error: interrupted` on
  stderr. `agy` was gone 1.3 s after `ralphy stop` returned; `ralphy` exit
  0 with `Stopped: you stopped the run during #133. Branch handed back with
  the work in place; #133 stays open.` The run summary records `up=0 cr=0
  out=0`: the usage of the interrupted `result` is not counted. Log:
  `antigravity-784-capstone-wsl-stop.log`.
