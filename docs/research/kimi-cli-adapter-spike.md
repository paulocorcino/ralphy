# Kimi CLI adapter — spike findings

Living notes from hands-on probing of the **Kimi CLI** (`kimi`) to de-risk a
`ralphy-agent-kimi` adapter. Goal: confirm the command surface, output formats,
auth/limit signatures, and token-capture path **before** writing the adapter, so
the build is mechanical (clone the Codex adapter) with no surprises.

Test project: `C:\Dev\FinCal` (throwaway repo created for this purpose).
Status legend: ✅ confirmed on machine · ⚠️ needs a logged-in run · ❓ unknown.

> **Spike complete (2026-07-08). No open questions.** Both phases done: all ⚠️
> items resolved in §11 (logged-in battery), and the last ❓ (429/limit) resolved
> in §11.9 by reading the installed source — `--print` exits **75** on
> 429/5xx/timeout → `Outcome::Limit`. **Verdict in §12: clone the Codex adapter.**
> Sections 1–10 are the logged-out phase (kept for the record); §11–12 supersede
> the ⚠️/❓ notes above.

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

---

## 11. Logged-in findings — RESOLVED ✅ (2026-07-08)

Ran the full battery authenticated (OAuth → Kimi Code) in `C:\Dev\FinCal`.

**The invocation the adapter should use** (works everywhere; avoids the Typer
arg-split and the Windows encoding trap):

```
kimi --work-dir <ws> --print --input-format text --output-format stream-json -y -m kimi-code/kimi-for-coding
     < <prompt on stdin>
```

### 11.1 stdout stream-json = coarse OpenAI-role JSONL
One JSON object per line, discriminated by top-level **`role`** (there is **no**
top-level `type`):
- `{"role":"assistant","content":[{"type":"think","think":…},{"type":"text","text":…}], "tool_calls":[{"type":"function","id":"tool_…","function":{"name":"WriteFile","arguments":"{…}"}}]}`
- `{"role":"tool","content":…,"tool_call_id":"tool_…"}`
- **Final assistant text** = the last `role:"assistant"` line whose `content[]`
  has a `type:"text"` part **and no `tool_calls` key**. That absence is the
  "turn finished" marker. The stream then just ends — **no explicit done/usage
  envelope on stdout.**

### 11.2 Token usage — lives ONLY in `wire.jsonl`, per-step 🎯
Not on stdout. In the session's `wire.jsonl`, in `StatusUpdate` events:

```json
{"type":"StatusUpdate","payload":{
   "context_tokens":14248,"max_context_tokens":262144,
   "token_usage":{"input_other":4776,"output":37,"input_cache_read":9472,"input_cache_creation":0},
   "message_id":"chatcmpl-…","plan_mode":false}}
```

- Path: `wire.jsonl` line → `message.payload.token_usage.{input_other, output, input_cache_read, input_cache_creation}`.
- **Per-step** (one `StatusUpdate` per LLM call). A turn total = sum of
  StatusUpdates between `TurnBegin` and `TurnEnd`.
- `wire.jsonl` event vocabulary: `metadata`, `TurnBegin{user_input}`,
  `StepBegin{n}`, `ContentPart{think|text|…}`, `StatusUpdate{token_usage,…}`,
  `TurnEnd`.
- **Field → `Usage` mapping** (ADR-0008): `input_other`→`input`,
  `output`→`output`, `input_cache_read`→`cache_read`,
  `input_cache_creation`→`cache_creation`. Model id
  `kimi-code/kimi-for-coding`.

**Capture strategy decided:** snapshot-diff the session dir's `wire.jsonl`
(Codex/Claude "appeared-over-grew" pattern via `session_files`), summing
`StatusUpdate.token_usage` across the run. The session id is recoverable from
**stderr** (`To resume this session: kimi -r <id>`) and from
`~/.kimi/kimi.json` → `work_dirs[].last_session_id`. Session dir:
`~/.kimi/sessions/<workdir-hash>/<session-id>/` (`wire.jsonl` + `context.jsonl`
+ `state.json`). Stdout stream-json is used only for the **final text / sentinel
/ tool-call** view, not tokens.

### 11.3 Auth on disk (refines §4/§6) ✅
- Auth is **not** in `config.toml` (`api_key=""`). It's OAuth, file-stored:
  `[providers."managed:kimi-code".oauth]` → `storage="file"`, `key="oauth/kimi-code"`.
- Token file: **`~/.kimi/credentials/kimi-code.json`** (`access_token`,
  `refresh_token`, `expires_at`, `scope`, `token_type`, `expires_in`).
- Provider block: `[providers."managed:kimi-code"]` `type="kimi"`,
  `base_url="https://api.kimi.com/coding/v1"`.
- Model block: `[models."kimi-code/kimi-for-coding"]` `max_context_size=262144`,
  `capabilities=["video_in","image_in","thinking"]`, `display_name="K2.7 Code"`.
- **Auth-error detection stays behavioral** (exit 1 + stdout `LLM not set`, see
  §11.5) rather than inspecting the credentials file — simpler and matches how
  the other adapters do it.

### 11.4 Completion / sentinel — Codex-style works ✅
- Real task (`--print -y`, "create KIMI_SPIKE.txt with `done`"): **exit 0**,
  file created & verified. Tool `WriteFile` (`{"path":…,"content":…}`); tool
  result wrapped in `<system>…</system>`.
- **Custom sentinel honored verbatim:** asked for `RALPHY_DONE` on the last
  line → final assistant `text` ended with exactly `RALPHY_DONE`. So the
  Codex-style *sentinel-in-final-message + exit code + HEAD-diff* completion
  detection is viable. Feed the shared `PROMPT_EXECUTE` charter (with
  `DONE_SENTINEL`/`blocked_reason`) and scrape the final assistant text.

### 11.5 Exit-code table ✅
| Case | Exit | Signal |
|------|------|--------|
| success | **0** | normal JSONL; resume hint on stderr |
| refusal (won't/can't do it) | **0** | refusal is in assistant `text` — **not** distinguishable by exit code; must judge from content (→ `blocked_reason` sentinel in charter) |
| **rate-limit 429 / 5xx / timeout** | **75** | `EX_TEMPFAIL`; provider error text on stdout → **`Outcome::Limit`** (§11.9) |
| invalid flag | **2** | stderr Typer usage box (`No such option '…'`) |
| permanent (auth `LLM not set`, invalid config, max steps) | **1** | plain text on stdout |

→ `classify_kimi_outcome` maps: **75 → Limit**, **1 → auth/permanent bail**,
**0 → Done or Blocked** decided by sentinel + HEAD-diff, **2 → usage error**.

### 11.6 Model flag ✅
- `-m kimi-code/kimi-for-coding` (full `provider-key/model`) → **accepted**.
- `-m kimi-for-coding` (short) → **rejected** (exit 1, `LLM not set`).
- Use the **full config-key form** `kimi-code/kimi-for-coding` as the default.

### 11.7 `--plan` mode — don't use it ✅
- `--plan` is `--print`-compatible (exit 0) but **explores heavily and is slow
  (>120s)**, and signals completion via an `ExitPlanMode` tool call, writing to
  `~/.kimi/plans/` + `state.json` (`plan_mode`, `plan_session_id`, `plan_slug`).
- **Decision:** do NOT use native `--plan`. Like the other adapters, run a normal
  headless `--print` with a **plan charter** that instructs the model to write
  `.ralphy/plan.md` itself. Plan success = `.ralphy/plan.md` appears on disk
  (same contract as Claude/Codex). Needs a `prompt.plan.kimi.md` (no tier line).

### 11.9 Rate-limit / 429 — RESOLVED from source ✅ (the last ❓)
Read straight from the installed source (v1.48.0) instead of guessing:

- **`--print` mode has dedicated exit codes** (`kimi_cli/cli/__init__.py:53-56`):
  ```python
  class ExitCode:
      SUCCESS   = 0
      FAILURE   = 1
      RETRYABLE = 75   # EX_TEMPFAIL from sysexits.h
  ```
- **429 (rate limit) and 5xx/timeout → exit 75** (`ui/print/__init__.py:438-449`,
  `_classify_provider_error`): `_RETRYABLE_STATUS_CODES = {429,500,502,503,504}`
  → `ExitCode.RETRYABLE`; connection/timeout/empty-response → `RETRYABLE` too;
  any other API status → `FAILURE (1)`. Confirmed by CHANGELOG: *"print mode now
  exits with code 1 for permanent failures (auth errors, invalid config) and
  code 75 for retryable failures (429 rate limit, 5xx server errors, connection
  timeouts)."*
- The provider error text is `print(str(e))`'d to **stdout** before exit
  (`ui/print/__init__.py:418-421`).
- **No structured reset/Retry-After at the chat level.** `retry_after` exists
  only for OAuth token refresh (`auth/oauth.py`), not for chat 429s. In-run
  retries are `kosong` backoff (banner shows attempt/wait), but the final
  give-up just returns exit 75. The 429 body *may* contain a human reset hint in
  the printed text, but it isn't guaranteed or structured.

**Adapter rule (final):** `classify_kimi_outcome` treats **exit 75 as
`limit: Some(None)`** → `Outcome::Limit(None)` (default wait/resume, ADR-0003).
This is cleaner than Claude/Codex (structured exit code, no text scraping).
Optional later polish: scan the stdout error text for a timestamp and, if
present, upgrade to `Outcome::Limit(Some(reset))` — same shape as PR #145, but
**not required** for a correct first version. Exit 1 stays the permanent-fail
bail (auth `LLM not set`, invalid config, max steps).

### 11.8 Windows gotchas (must bake into the adapter) ⚠️→✅
- **Always** drive with `--output-format stream-json`. The default rich/TUI
  renderer writes box-drawing/emoji and **crashes on a cp1252-redirected stdout**
  (`'charmap' codec can't encode…`, exit 1).
- **Do not** set `PYTHONIOENCODING=utf-8` with redirected/no-console stdio — it
  flips kimi into trying to start the Textual TUI (`No Windows console found`).
  stream-json is ASCII-safe; that's the whole mitigation.
- Prompt via **stdin** (`--input-format text`), never as a split argv (Typer
  treats a stray word as a subcommand). Rust `Command::arg(prompt)` as a single
  element also works, but stdin is the belt-and-suspenders choice.

---

## 12. Final verdict & closed de-para

**Build `crates/ralphy-agent-kimi` by cloning `crates/ralphy-agent-codex`.**
Headless-only, no PTY, uses the shared scaffold (`run_plan_session` /
`run_exec_session`), `run_headless_logged`, and the `classify` ladder unchanged.

Concrete adapter shape:

| Concern | Kimi mechanism | Source of truth |
|---------|----------------|-----------------|
| Headless plan/exec | `kimi --print --input-format text --output-format stream-json -y -m kimi-code/kimi-for-coding` (prompt on stdin) | §11 |
| `plan()` | normal `--print` + `prompt.plan.kimi.md` → model writes `.ralphy/plan.md`; success = file on disk (not native `--plan`) | §11.7 |
| `execute()` | `run_exec_session` loop + `PROMPT_EXECUTE` charter; sentinel in final assistant `text` + exit + HEAD-diff | §11.4 |
| Completion classify | `classify_kimi_outcome` → `CompletionSignals` → shared `classify`; exit **75**→Limit, **1**→auth bail, **0**→Done/Blocked via sentinel + HEAD-diff, **2**→usage error | §11.4/11.5/11.9 |
| Limit detection | **exit code 75** (`EX_TEMPFAIL`, 429/5xx/timeout) → `Outcome::Limit(None)`; no scraping | §11.9 |
| Permission bypass | `-y` (`--yolo`) | §2 |
| Model | `-m kimi-code/kimi-for-coding` (full form); **no tier routing** | §11.6 |
| Tokens (ADR-0008) | snapshot-diff `wire.jsonl` `StatusUpdate.payload.token_usage`, sum per run; map 4 fields → `Usage` | §11.2 |
| Session dir | `~/.kimi/sessions/<workdir-hash>/<session-id>/`; id from stderr resume hint or `kimi.json` | §11.2 |
| Auth-error bail | exit 1 + stdout `LLM not set` → `KIMI_AUTH_ERROR_MSG` telling user to run `kimi login` | §11.5 |
| Binary resolve | `resolve_program("kimi")`, also probe `~/.local/bin` | §7 |
| Skills | `--skills-dir` (repeatable) or `--agent-file`; materialize like Codex | §2 |
| Config injection | `--config '<toml>'` inline if needed (else rely on `~/.kimi/config.toml`) | §6 |
| `ACCEPTS_IMAGES` | model has `image_in` cap → defensible `true`; start `false` (safe) and revisit for triage | §11.3 |
| Force stream-json + stdin | **mandatory** on Windows (encoding + Typer traps) | §11.8 |

**No open questions remain.** The last one (429/limit behavior) was resolved by
reading the installed source, not by triggering a real limit: `--print` exits
**75** (`EX_TEMPFAIL`) on 429/5xx/timeout → `Outcome::Limit(None)`; exit 1 =
permanent bail (§11.9). A structured reset timestamp is **not** exposed at the
chat level, so `None` (default wait, ADR-0003) is correct; an optional text-scan
upgrade later mirrors PR #145 but isn't needed for a correct v1.

**Wiring checklist** (unchanged from the pre-spike analysis): two enums
(`cli.rs` `CliAgent`, `init/gate.rs` `Agent`), `build_agent` in `wiring.rs`,
the three one-shot dispatch matches (`init/run.rs`, `init/issues.rs`,
`triage.rs`), `models.rs` (`agent_slug`/`plan_action`), Cargo workspace + CLI
dep, and a `prompt.plan.kimi.md`. Zero changes to `ralphy-core`.
