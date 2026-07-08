# Kimi CLI adapter — spike findings

Living notes from hands-on probing of the **Kimi CLI** (`kimi`) to de-risk a
`ralphy-agent-kimi` adapter. Goal: confirm the command surface, output formats,
auth/limit signatures, and token-capture path **before** writing the adapter, so
the build is mechanical (clone the Codex adapter) with no surprises.

Test project: `C:\Dev\FinCal` (throwaway repo created for this purpose).
Status legend: ✅ confirmed on machine · ⚠️ needs a logged-in run · ❓ unknown.

---

## 1. Environment

- **Install:** `uv tool install --python 3.13 kimi-cli` (PyPI package `kimi-cli`,
  Python 3.12–3.14, 3.13 recommended). Official one-liner also exists:
  `Invoke-RestMethod https://code.kimi.com/install.ps1 | Invoke-Expression`.
- **Executables installed:** `kimi` and `kimi-cli` (same binary).
- **Version probed:** `kimi, version 1.48.0`. `kimi info` → agent spec `1`,
  wire protocol `1.10`, python `3.13.13`.
- **On this machine** the binary landed in `~/.local/bin` (not on PATH by
  default in this shell — the adapter must resolve it, see §7). ✅
- It is a **Python/Typer** app (matters for arg parsing quirks, §3).

---

## 2. Command surface (v1.48.0) — the flags the adapter needs

Kimi CLI's headless surface is **near-identical to Claude Code and Codex**, which
is why the adapter is mostly a clone.

| Flag | Meaning | Adapter use |
|------|---------|-------------|
| `--print` | non-interactive mode; auto-dismiss AskUserQuestion, auto-approve tools **for this run** | **the headless switch** (like `claude -p`) |
| `-p, --prompt TEXT` | user prompt, doesn't enter interactive mode | pass the charter |
| `--output-format [text\|stream-json]` | must be used with `--print` | `stream-json` for event/token parsing |
| `--input-format [text\|stream-json]` | must be used with `--print`; prompt piped via stdin | alternative to `-p` (avoids arg-splitting, §3) |
| `--final-message-only` | print only the final assistant message | clean sentinel capture |
| `--quiet` | alias for `--print --output-format text --final-message-only` | — |
| `-m, --model TEXT` | LLM model, overrides `default_model` in config | model select (single id `kimi-for-coding`) |
| `-y, --yolo` (aliases `--yes`, `--dangerous...`) | auto-approve all actions | permission bypass |
| `--afk` | away-from-keyboard: auto-approve + auto-dismiss AskUserQuestion | belt-and-suspenders with `--print` |
| `--plan` | **start in plan mode (native)** | candidate for `plan()` phase |
| `-w, --work-dir DIRECTORY` | working directory for the agent | point at the run workspace |
| `--add-dir DIRECTORY` | add extra dir to workspace scope (repeatable) | — |
| `-S, -r, --session/--resume [ID]` | resume a session (with/without id) | **not used** — ralphy never `--resume` (ADR-0003) |
| `-C, --continue` | continue previous session for the work dir | **not used** |
| `--config TEXT` | config TOML/JSON **string** to load | inject config inline (like OpenCode's `OPENCODE_CONFIG_CONTENT`) |
| `--config-file FILE` | config file to load (default `~/.kimi/config.toml`) | alt |
| `--agent [default\|okabe]` | builtin agent spec | keep `default` |
| `--agent-file FILE` | custom agent spec file | possible skills/charter hook |
| `--skills-dir DIRECTORY` | custom skills dirs (repeatable), overrides discovery | **skills materialization hook** |
| `--mcp-config[-file]` | MCP config (JSON string / file, repeatable) | — |
| `--max-steps-per-turn INT` | cap steps in a turn (default 1000 from config) | safety cap |
| `--max-retries-per-step INT` | retries per step (default 3) | — |
| `--max-ralph-iterations INT` | **native "Ralph mode"** extra iterations; -1 = unlimited | **ignore** — ralphy drives its own loop |
| `--thinking / --no-thinking` | thinking mode | optional |
| `--verbose`, `--debug` | logging | debug only |

**Subcommands:** `login`, `logout`, `term`, `acp`, `info`, `export`, `mcp`,
`plugin`, `vis`, `web`.

- `kimi login [--json]` / `kimi logout [--json]` — **auth is a subcommand, not
  just a TUI `/login`.** `--json` emits OAuth events as JSON lines (scriptable,
  though ralphy will likely just detect the auth error and tell the user to run
  `kimi login`, per ADR-0013).
- `kimi export [SESSION_ID] [-o PATH] [-y]` — exports a session **as a ZIP**.
  Defaults to the previous session. Not needed for token capture (raw files are
  simpler, §5), but useful for debugging.
- `kimi info` — version/protocol, always exit 0.

---

## 3. Typer arg-parsing quirk (bit us during the spike) ⚠️

Kimi is a Typer app. A **multi-word prompt passed positionally through
`Start-Process -ArgumentList`** got re-split and the second word was interpreted
as a subcommand:

```
kimi ... --prompt 'Say hello and nothing else.'   # via Start-Process ArgumentList
=> Error: No such command 'hello'.   (exit 2)
```

Implication for the adapter: **do not rely on shell arg-joining.** Either

1. spawn with a real argv array where the whole prompt is one element (Rust
   `Command::arg(prompt)` does this correctly — the PowerShell mangling was a
   `Start-Process` artifact, not a Rust one), or
2. prefer **`--print --input-format text` with the prompt piped on stdin**
   (mirrors how Claude's plan charter goes in on stdin). This side-steps the
   whole class of quoting problems and is the recommended path.

`exit 2` = Typer usage/parse error; `exit 1` = runtime error (§4). Distinct
codes — useful signal.

---

## 4. Logged-out behavior — the auth-error signature ✅

Ran headless in `C:\Dev\FinCal` **while logged out**:

```
kimi --work-dir C:\Dev\FinCal --print --output-format stream-json --prompt hello
```

Result:
- **Exit code: `1`**
- **stdout:** `LLM not set`  ← plain text, **NOT** JSON, even though
  `--output-format stream-json` was requested. The failure happens *before* the
  model loop starts, so no stream-json envelope is emitted.
- **stderr:** `To resume this session: kimi -r <session-id>`  ← printed on
  **every** run (see §5).

So the logged-out / no-model detector (`KIMI_AUTH_ERROR_MSG`, analogous to
`CLAUDE_AUTH_ERROR_MSG`) keys on: **exit 1 + stdout contains `LLM not set`**.
Actionable message should tell the user to run `kimi login`.

> Caveat: `LLM not set` literally means "no `default_model` resolved". Logged out
> this is the symptom, but a logged-in user with an empty `default_model` and no
> `-m` could hit the same string. The adapter always passes `-m`, so post-login
> this should only appear on a genuine auth gap. **Confirm the real
> post-login-but-token-expired message during the logged-in phase (⚠️).**

First headless run also **materialized `~/.kimi/`** from nothing (see §6).

---

## 5. Session storage & token-capture path

Every run writes a session tree:

```
~/.kimi/sessions/<workdir-hash>/<session-id>/
    ├── context.jsonl   # full message log (system prompt, turns)
    └── wire.jsonl      # wire-protocol event stream
```

- `<workdir-hash>` observed `2a8e98a033a3fd28d98d7550370cf2bb` for
  `C:\Dev\FinCal` — a hash of the work-dir path (per-workspace bucketing).
- `<session-id>` is a UUID, e.g. `718e658e-8597-4c1e-842b-b41f1296dd34`.
- **`wire.jsonl` (logged-out run) contained:**
  ```json
  {"type": "metadata", "protocol_version": "1.10"}
  {"timestamp": ..., "message": {"type": "TurnBegin", "payload": {"user_input": "hello"}}}
  {"timestamp": ..., "message": {"type": "TurnEnd", "payload": {}}}
  ```
  Logged out there are no usage fields (loop never ran). **A logged-in run should
  carry token usage in `TurnEnd`/assistant events — this is the token-capture
  target (⚠️ confirm shape).**

**Two viable capture strategies (decide after the logged-in run):**
1. **Parse `--output-format stream-json` on stdout** (like OpenCode's
   `events.rs`) — preferred if usage rides the stream. No filesystem coupling.
2. **Snapshot-diff `wire.jsonl`** in the session dir (like Codex's rollout-file /
   Claude's transcript approach, "appeared-over-grew" via `session_files`) — the
   session id is recoverable two ways: the **stderr `To resume this session:
   kimi -r <id>` line**, and `~/.kimi/kimi.json` → `work_dirs[].last_session_id`.

Strategy 1 is cleaner (no path/hash reverse-engineering) **if** stream-json
includes usage.

---

## 6. Config file — `~/.kimi/config.toml` ✅

Auto-created on first run. TOML (JSON also accepted; legacy JSON auto-migrates).
Full default (logged out) captured:

```toml
default_model = ""            # empty until login/config; adapter overrides via -m
default_thinking = false
default_yolo = false
skip_afk_prompt_injection = false
default_plan_mode = false
default_editor = ""
theme = "dark"
show_thinking_stream = true
hooks = []
merge_all_available_skills = true
extra_skill_dirs = []
telemetry = true

[models]                      # populated after login/provider setup
[providers]                   # type / base_url / api_key / custom_headers / env
[loop_control]
max_steps_per_turn = 1000
max_retries_per_step = 3
max_ralph_iterations = 0
reserved_context_size = 50000
compaction_trigger_ratio = 0.85

[background]                  # concurrent bg-task runtime (agent_task_timeout_s=900, ...)
[notifications]
[services]
[mcp.client]
tool_call_timeout_ms = 60000
```

Other state files in `~/.kimi/`: `kimi.json` (work-dir registry +
`last_session_id`), `device_id` (32 chars), `logs/kimi.log`, `telemetry/`.

Adapter notes:
- The adapter can pass config inline via `--config '<toml>'` (no need to mutate
  the user's global file) — mirrors OpenCode's injected-config pattern.
- After login, `[providers]`/`[models]`/`default_model` get populated; **capture
  the exact diff during the logged-in phase (⚠️)** to know what "authenticated"
  looks like on disk (for a possible richer auth check).
- Model id per docs: `kimi-for-coding` (single stable id; backend remaps to
  newer models). **No complexity/tier routing needed** — follow OpenCode's D8
  (drop tiers).

---

## 7. Binary resolution ⚠️

On this machine `kimi` is in `~/.local/bin`, which was **not** on PATH in a fresh
shell. The adapter's `command.rs` must resolve it via
`ralphy_adapter_support::resolve_program("kimi")`, and the resolver / preflight
should also probe `~/.local/bin` (uv tool install location) in addition to PATH.
Confirm `resolve_program` already covers `~/.local/bin` or add it.

---

## 8. Rate limits (from docs, not yet observed) ❓

- Rolling **5-hour window**, ~300–1200 requests, up to 30 concurrent.
- **Weekly quota**, resets 7 days from subscription date, no carryover.
- Shared across all devices/API keys on the account.
- **Unknown:** exact CLI output on a 429/limit and whether it includes a
  parseable reset timestamp. If not, fall back to `Outcome::Limit(None)`
  (default wait) — no regression. This is the same shape of work as the Codex
  reset-hint handling (PR #145). **Capture a real limit message if one occurs
  during heavy logged-in testing (⚠️).**

---

## 9. De-para confirmations vs. the original analysis

Everything in the pre-spike de-para held up. Concrete confirmations:

- ✅ `plan()` → `kimi --print --output-format stream-json` (+ `--plan` for native
  plan mode, TBD which we use) — 1:1 with `claude -p`.
- ✅ `execute()` → headless `kimi --print ... -y` loop via
  `run_headless_logged` + HEAD-diff (Codex pattern).
- ✅ Permission bypass = `-y/--yolo` (+ `--afk`).
- ✅ Model select = `-m kimi-for-coding`, no tiers.
- ✅ Sentinel completion via `--final-message-only` + exit code + HEAD-diff
  (no Stop-hook / PTY needed — headless-only like Codex).
- ✅ Auth-error signature = exit 1 + `LLM not set` → tell user `kimi login`.
- ✅ Config = `~/.kimi/config.toml` (TOML), inline via `--config`.
- ⚠️ Token capture = stream-json stdout (preferred) or `wire.jsonl` snapshot.
- ⚠️ Limit reset hint = unknown format → safe `None` fallback.

**Template to clone remains Codex** (`crates/ralphy-agent-codex`): headless,
uses shared scaffold + `classify` ladder, sentinel-in-final-message + exit +
HEAD-diff. Add `prompt.plan.kimi.md` (no tier line).

---

## 10. Open questions for the logged-in phase (the "leave me working" list)

Run once authenticated (`kimi login`), then unattended:

1. **stream-json shape** of a *successful* `--print` run — capture full envelope
   sequence (message types, where assistant text lands, where DONE/BLOCKED
   sentinel text would surface).
2. **Token usage location** — is per-turn usage in the stdout stream, in
   `wire.jsonl`, or only via `kimi export`? Grab a real sample.
3. **`config.toml` post-login diff** — `[providers]`, `[models]`, `default_model`.
4. **`--plan` mode output** — does plan mode still produce a file / final message
   we can turn into `.ralphy/plan.md`, or is it purely a permission gate?
5. **Real prompt on stdin** (`--input-format text`) end-to-end, confirming the
   arg-splitting workaround (§3).
6. **A genuine limit/429 message** if heavy testing triggers one — exact text +
   any reset timestamp.
7. **Exit codes** for: success, blocked/refused, tool-approval-needed-but-afk,
   mid-run auth expiry.
8. Whether `-m kimi-for-coding` is accepted verbatim or needs a `[models]` entry.
