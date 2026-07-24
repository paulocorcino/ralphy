# Live evidence — `ralphy run --agent copilot` (#229)

The tracer bullet, exercised end to end against the authorized lab repo
`C:\Dev\FinCal` (`paulocorcino/FinCal`) on 2026-07-20. Copilot CLI **1.0.71**,
Windows 11 Pro 26200.

## The command

```
./target/debug/ralphy.exe run --repo C:/Dev/FinCal --agent copilot \
    --only-issue 108 --base-branch origin/master \
    --max-minutes-per-issue 12 --verbose
```

Run dir: `C:\Dev\FinCal\.ralphy\runs\20260720-084338\` (`copilot.log`, 4 038 776 bytes).

## What it proves

| Claim | Evidence |
|---|---|
| The plan phase drives Copilot with the Tier 2 charter | `plan written number=108 open_steps=16` — 16 steps parsed out of a plan Copilot wrote from `prompt.plan.copilot.md` |
| The stream is JSON lines | every line of `copilot.log` is one `{"type","data","id","timestamp","parentId","ephemeral"?}` object |
| No PTY is allocated | the crate has no `ralphy-pty` edge (`crates/ralphy-agent-copilot/Cargo.toml`); the whole run went through `HeadlessCall` |
| `--disable-builtin-mcps` takes effect | `session.mcp_servers_loaded` reports `github-mcp-server` with `"status":"disabled"` (3 records) — the bundled GitHub MCP server, which holds the operator's token and can open PRs, never loads |
| The charter arrives on stdin | no `-p` in the argv, and the agent executed the 16-step plan it was handed |
| The run commits real work | `committed=true`; `git log afk/run-20260720-084338` shows `a022715b`, `524cf3f4`, `3e5d687a`, all `(#108)` |
| The outcome is CLASSIFIED, not hung | `Timeout` at the 12-minute wall, reaped by the runner rather than left running |

## The head of the stream

```json
{"type":"session.mcp_servers_loaded","data":{"servers":[{"name":"github-mcp-server","status":"disabled","source":"builtin","transport":"http"}]},"id":"68575549-…","timestamp":"2026-07-20T11:44:00.546Z","parentId":"d8c9bc36-…","ephemeral":true}
{"type":"session.skills_loaded","data":{"skills":[{"name":"domain-modeling",…,"source":"project",…}]},…}
```

## The tail of the stream

The child was reaped at the wall, so there is **no `result` envelope** — the last
records are a `tool.execution_complete` and the `assistant.turn_start` of turn 54:

```json
{"type":"assistant.turn_end","data":{"turnId":"53","model":"claude-sonnet-5"},"id":"1648bab2-…","timestamp":"2026-07-20T11:55:54.101Z",…}
{"type":"assistant.turn_start","data":{"turnId":"54","model":"claude-sonnet-5","interactionId":"2aa1c0bb-…"},"id":"9a518fa3-…","timestamp":"2026-07-20T11:55:54.102Z",…}
```

This is exactly the case the parser must survive: no terminal envelope, no final
tool-less `assistant.message`, a truncated tail. `copilot_final_text` returned no
sentinel and `classify_copilot_outcome` let `timed_out` win.

## The final status lines

```
2026-07-20 08:55:57  INFO ralphy_agent_copilot: copilot execution ended outcome=Timeout exited_cleanly=false timed_out=true exit_code=None committed=true
2026-07-20 08:55:57  INFO ralphy_core::emit: non-green — stopping run number=108 outcome=Timeout
2026-07-20 08:55:57  WARN ralphy::run::report: knowledge consolidation failed — notes kept loose for retry error=the copilot adapter does not support one-shot consolidate yet (tasks.rs is a later slice, ADR-0040 Tier 1)
2026-07-20 08:55:57  INFO ralphy_core::emit: run finished outcome="non_green" issues_total=1 issues_blocked=1 duration_s=738
```

`Timeout` is the CORRECT classification: the issue is a full transfer feature and
12 minutes was a deliberately short budget for this probe, not a defect. The
consolidate bail is the honest one-shot stub degrading to a warning rather than
crashing the close.

## Known gaps this run confirms

- **Zero tokens reported** (`up=0 cr=0 cw=0 out=0`) — `usage.rs` is the D10 slice.
- **An inherited `GH_TOKEN` breaks the vendor CLI outright**, not just widens its
  reach: with a classic PAT in the environment `copilot -p …` refuses to start
  ("Replace the token in GH_TOKEN with a fine-grained PAT"). The adapter's D8
  `env_remove` of `COPILOT_GITHUB_TOKEN`/`GH_TOKEN`/`GITHUB_TOKEN` is therefore
  load-bearing for CORRECTNESS on this host, not only for blast radius — and the
  same scrub is why `ralphy init`'s Copilot login probe can succeed here.
