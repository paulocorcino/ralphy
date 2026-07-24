# Live smoke — Copilot reached from the workbench (#238)

Host: Windows 11 (10.0.26200). `copilot.exe` from WinGet Links, self-reported
**v1.0.71** in the TUI. Binary: `./target/debug/ralphy.exe` built from this
branch at commit `ef6bf5b`.

Isolation: the daemon ran on port **7357** with `RALPHY_DAEMON_DIR` pointed at a
scratch dir, so the operator's own daemon store (which has require-login on) was
never touched. Two repos were registered in that scratch registry: this repo, and
`C:\tmp\ralphy-238-scratch` — a throwaway git repo whose `origin` deliberately
names a **nonexistent** GitHub repo, so the dispatched run proves the argv path
without a real agent ever engaging an issue.

## Commands

```bash
RALPHY_DAEMON_DIR=C:/tmp/ralphy-238-daemon ./target/debug/ralphy.exe daemon --port 7357
RALPHY_DAEMON_DIR=C:/tmp/ralphy-238-daemon ./target/debug/ralphy.exe daemon add C:/Dev/ralphy
RALPHY_DAEMON_DIR=C:/tmp/ralphy-238-daemon ./target/debug/ralphy.exe daemon add C:/tmp/ralphy-238-scratch
```

The three probes were Python `websockets` / Playwright clients speaking the
daemon's own wire codec (`protocol.rs`), i.e. exactly what the browser sends.

## 1. An interactive Copilot console opens

`GET /ws/session?agent=copilot&repo=paulocorcino%2Fralphy`. The PTY opened,
emitted `ESC[6n`, and drew once the probe answered the DSR as a terminal would:

```
  ╭─╮╭─╮
  ╰─╯╰─╯  Copilot v1.0.71 uses AI.
  █ ▘▝ █  Check for mistakes.
   ▔▔▔▔  ● Tip: /autopilot  ● No copilot-instructions.md found. Run /init to generate.
C:\Dev\ralphy [⎇ feat/copilot]
❯
● Loading: 1 instruction, 3 skills!  ◉ Session: 0 AIC used
```

The daemon's child process list confirms the real binary, not a stand-in:

```
ProcessId : 14612
Name      : copilot.exe
cmd       : C:\Users\PICHAU\AppData\Local\Microsoft\WinGet\Links\copilot.EXE
```

Note `console=1` is the *plain shell* flag (`console_spec`), not the agent — the
first probe passed it and correctly got `pwsh`. The agent path is `agent=` alone.

## 2. A dispatched run reaches `--agent copilot` on argv and spawns

`/ws/command`, verb `run`, payload `{repo, agent: "copilot", branchMode: "new"}`.
The spawned child's command line, read back from WMI while it lived:

```
"C:\Dev\ralphy\target\debug\ralphy.exe" run --if-idle --agent copilot --branch-mode new
```

The frames the daemon sent back:

```
{"id":1,"verb":"run","payload":{"pid":47928,"status":"spawned"}}
{"id":1,"verb":"run","payload":{"chunk":"Error: ","status":"output"}}
{"id":1,"verb":"run","payload":{"chunk":"`gh issue list --label ready-for-agent` failed: GraphQL: Could not resolve to a Repository with the name 'paulocorcino/ralphy-238-no-such-repo'. (repository)\n","status":"output"}}
{"id":1,"verb":"run","payload":{"code":1,"status":"exited"}}
```

`--agent copilot` was accepted by clap and the run died only at the queue fetch
against the deliberately unresolvable repo — which is the point: the argv path is
proven end to end with nothing left behind. Before this slice the same dispatch
never spawned at all; `Agent::from_query("copilot")` returned `None` and the reply
was `{"status":"error","message":"invalid run options"}`.

## 3. The UI offers Copilot in both sites

Browser-driven (headless Chromium against `http://127.0.0.1:7357`):

- **Console menu** — lists `claude/codex/opencode/kimi/copilot` with the
  accelerators `Alt+Shift+1..5`, plus `console` on `Alt+Shift+0`.
- **`Alt+Shift+5`** — with the project open, opened session
  `{"id":4,"repo":"paulocorcino/ralphy","agent":"copilot","kind":"agent"}` and the
  live Copilot TUI rendered in the xterm.js pane. (The accelerator is inert with
  no project selected, by design — `!c.openSlug` returns early.)
- **Run modal** — the Agent segment reads
  `claude · codex · opencode · kimi · copilot`, and the plan-agent segment behind
  "Plan with a different agent" carries the same five.

## 4. `/api/usage` covers Copilot

`GET /api/usage` over the live store, records per agent:

```
claude 1484 · codex 171 · opencode 110 · kimi 74 · copilot 26
```

with real rows, e.g. `{"agent":"copilot","model":"gpt-5-mini",
"session_id":"03013a25-…","tokens":{"input":13583,"output":306,…}}`. This needed
no change in this slice — the `copilot_db` state plumbing landed with the usage
scan — but it is the acceptance criterion, so it was checked rather than assumed.

## Teardown

Sessions closed via `POST /api/sessions/close`, the daemon stopped, and both
`C:\tmp\ralphy-238-daemon` and `C:\tmp\ralphy-238-scratch` removed. Nothing was
written to the operator's real daemon store or to any GitHub repo.
