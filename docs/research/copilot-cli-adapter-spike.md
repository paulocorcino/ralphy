# GitHub Copilot CLI — adapter spike

Evidence for a prospective `ralphy-agent-copilot`, gathered against
**GitHub Copilot CLI 1.0.71** (winget install, `copilot.exe`) on **Windows 11
Pro 26200**, with the target repo **`C:\Dev\FinCal`** (`paulocorcino/FinCal`,
branch `feat/opencode-v2`).

This document answers the C-questions of
[ADR-0040](../adr/0040-agent-adapter-onboarding-contract.md). It records
**observations**, not decisions; decisions belong in the Copilot adapter ADR.
Every claim below cites a command that was run and its output. Where a
capability is asserted only by `--help` and was not exercised, it is marked
**⚠ unverified**.

Session date: 2026-07-20.

---

## 0. Executive summary

Copilot CLI is the **richest headless surface Ralphy has evaluated**. Three
things it does better than any existing vendor:

1. **`--session-id <uuid>` lets Ralphy mint the session id before spawning.**
   The ADR-0008 D10 snapshot-diff is unnecessary; usage lookup is a primary-key
   read.
2. **A real terminal envelope.** `--output-format json` ends with a
   `{"type":"result", …}` line carrying `sessionId`, `exitCode` and a usage
   block. No other vendor gives an unambiguous "the run is over" record.
3. **A first-class usage table.** `assistant_usage_events` in the session store
   carries per-call input/output/cache-read/cache-write/reasoning tokens **and**
   per-token pricing, keyed by session id.

And three things that are materially worse:

1. **Model selection is a plan entitlement, not a CLI feature.** On a **free**
   account `--model` rejects *every* id — including ones the CLI itself routes
   to — and only `auto` is accepted. On a **paid** account the same flag works,
   as does `--effort`. An adapter that hardcodes a model id works on one and
   hard-fails every run on the other. §4 has both observations and the free,
   deterministic way to enumerate what the current account can actually use.
2. **Blast radius is large and on by default**: a bundled GitHub MCP server
   holding the operator's token, session export to GitHub web/mobile, and a
   `/delegate` verb that opens PRs. This collides head-on with Ralphy's
   never-push/never-open-a-PR ethos.
3. **Billing is denominated in AI credits, not tokens**, and the two are
   reported through different channels.

---

## 1. C1 — Invocation and the headless contract

| Question | Finding |
|---|---|
| Headless one-shot | `copilot -p <text>` — "Execute a prompt in non-interactive mode (exits after completion)" |
| Prompt channel | **Both argv and stdin.** `--help` on a missing prompt: *"Run in an interactive terminal or provide a prompt with `-p` or via standard in."* |
| Argv ceiling | A **23 433-byte** charter was passed on argv successfully (probe P2). Ralphy's `prompt.execute.md` is 23 884 bytes *before* the issue body — so argv is within ~30 % of the Windows ceiling with no margin. **stdin is the only safe channel.** |
| Prompt via stdin | ✅ **VERIFIED** (probe P3). A **24 250-byte** payload piped into `copilot` with **no `-p` at all** arrived intact — the reply echoed both a marker planted on the first line and one on the last (`RALPHY_HEAD_7F3A\|RALPHY_TAIL_9C2B\|RALPHY_DONE_EXIT`), and the usage row recorded `input_tokens=31594`. No truncation, no flag needed. **This is the channel the adapter must use.** |
| Full autonomy | `--allow-all-tools` is *required* for non-interactive mode. `--allow-all` / `--yolo` = `--allow-all-tools --allow-all-paths --allow-all-urls`. |
| PTY required for billing? | **No.** Headless `-p` bills identically; the Claude particularity (ADR-0002) does not apply. |
| Working directory | `-C <directory>`, and the CLI honours the spawned process's cwd (probes ran from `C:\Dev\FinCal` with no `-C` and the session recorded `cwd=C:\Dev\FinCal`). |
| Autonomy extras | `--no-ask-user` disables the `ask_user` tool outright — stronger than relying on `-p` to auto-dismiss. |

Other invocation-shaped flags: `--add-dir`, `--allow-tool` / `--deny-tool`
(pattern grammar `shell(git:*)`, `write(path)`, `<mcp-server>(tool)`,
`url(domain)`; **deny always beats allow, even `--allow-all-tools`**),
`--available-tools` / `--excluded-tools` (visibility, not permission),
`--disallow-temp-dir`, `--secret-env-vars`, `--max-autopilot-continues`,
`--agent <name>`, `--attachment <path>`, `--continue`, `-r/--resume`,
`--share[=path]`, `--share-gist`, `--acp` (Agent Client Protocol server mode).

---

## 2. C2 — The output stream

`--output-format text` (default) or `json` — "JSONL, one JSON object per line".
`--stream on|off`. `--no-color`, `--plain-diff`, `--log-level none`,
`--log-dir <dir>` (default `~/.copilot/logs/`, one `process-<epoch>-<pid>.log`
per run).

**No cp1252 crash was observed** on redirected stdout with `--output-format
json` — unlike Kimi (ADR-0028 D5). ⚠ the default `text` renderer under
redirection was not stress-tested.

### Event envelope

Every line: `{"type", "data", "id", "timestamp", "parentId", "ephemeral"?}`.
Discriminators observed across probes P1 and P2:

```
session.mcp_server_status_changed   session.mcp_servers_loaded
session.skills_loaded               session.tools_updated
session.auto_mode_resolved          session.background_tasks_changed
mcp.tools.list_changed              user.message
assistant.turn_start                assistant.turn_end
assistant.reasoning_delta           assistant.reasoning
assistant.message_start             assistant.message_delta
assistant.message                   assistant.tool_call_delta
tool.execution_start                tool.execution_partial_result
tool.execution_complete             assistant.idle
result
```

`ephemeral: true` marks the delta/streaming records; the non-ephemeral ones are
the durable spine. **A parser that drops `ephemeral` lines loses nothing.**

### The terminal envelope

```json
{"type":"result","timestamp":"2026-07-20T08:57:02.582Z",
 "sessionId":"d911b7f0-7e70-471c-a12c-39a114e4afc1","exitCode":0,
 "usage":{"premiumRequests":0.33,"totalApiDurationMs":9364,"sessionDurationMs":14496,
          "codeChanges":{"linesAdded":0,"linesRemoved":0,"filesModified":[]}}}
```

### Final assistant message

`assistant.message` with `data.toolRequests: []` and non-empty `data.content` —
the same "last tool-call-free assistant turn" shape as Kimi. It also carries
`data.model` (the *actually used* model) and `data.outputTokens`.

### ⚠ `codeChanges` is NOT a progress signal

In probe P2 the agent created a file, staged it and committed it — HEAD advanced
from `0d7b10f1` to `49732d59`, `git show --stat` confirms
`.ralphy-probe/hello.txt | 1 +`. The `result` envelope still reported
`codeChanges: {linesAdded: 0, linesRemoved: 0, filesModified: []}`, because the
work went through the **shell** tool rather than the write tool.

**`codeChanges` counts the vendor's own write-tool activity, not repository
change.** The HEAD-diff `committed` guard remains load-bearing exactly as for
the other four vendors. This is the single most dangerous false friend in
Copilot's stream.

---

## 3. C3 — Completion and the sentinel

**The custom sentinel works.** Probe P1, `-p "Reply with exactly this token on
the last line and nothing else: RALPHY_DONE_EXIT"`:

```json
{"type":"assistant.message","data":{"model":"claude-haiku-4.5",
 "content":"RALPHY_DONE_EXIT","toolRequests":[],"outputTokens":75,…}}
```

Probe P2, after real tool work, the final message was:

```
Done. File created, staged, and committed successfully.

RALPHY_DONE_EXIT
```

`exitCode` appears in **two** places and agreed in every probe: the process exit
status and `result.exitCode`.

Exit codes observed:

| Code | Condition | Channel |
|---|---|---|
| 0 | clean finish | P1, P2, `--model auto` probe |
| 1 | not authenticated | stderr `Error: No authentication information found.` |
| 1 | unknown model | stderr `Error: Model "X" from --model flag is not available.` |
| 1 | bad flag value | stderr `error: option '--max-ai-credits <credits>' argument '1' is invalid…` |

**No semantic exit code equivalent to Kimi's `75 = RETRYABLE` was found.**
⚠ a real quota exhaustion was not induced (see §7).

**Hooks exist**: `.github/hooks/*.json`, plus inline `hooks` in config, plus a
`disableAllHooks` kill switch. This is a potential deterministic completion
mechanism in the Claude Stop-hook family. ⚠ entirely unexercised — the event
names and payload schema are undocumented in `--help`.

---

## 4. C4 — Models

**This section was probed twice: once on a free account, then again after the
operator upgraded to a paid plan.** The delta is the finding.

### 4a. Model pinning is gated by plan entitlement

On the **free** account, `--model` rejected every id — including the two the
CLI itself routes to:

```
$ copilot -p "…" --model claude-haiku-4.5 …
Error: Model "claude-haiku-4.5" from --model flag is not available.   exit=1
$ copilot -p "…" --model gpt-5-mini …
Error: Model "gpt-5-mini" from --model flag is not available.         exit=1
$ copilot -p "say OK" --model auto …                                   exit=0
```

After the **upgrade**, the same flag works, and so does `--effort`:

```
$ copilot -p "Reply with exactly: RALPHY_DONE_EXIT" --model claude-sonnet-5 --effort high …
exit=0   assistant.message.data.model = "claude-sonnet-5"
result.usage.premiumRequests = 1          # vs 0.33 for claude-haiku-4.5
debug log: "model": "capi:claude-sonnet-5:defaultReasoningEffort=high"
           "defaultReasoningEffort": "high"
```

So `--effort` is **confirmed accepted and applied**, encoded into the model
handle rather than sent as a separate parameter.

**Consequence for the adapter: an implementation that hardcodes a default model
id, as the Kimi adapter does, works on a paid plan and hard-fails on every
single run on a free one. Model resolution must be `Option<String>`, omitted
from argv when `None` — the OpenCode D4 shape, not the Kimi D4 shape.**

#### Omitting `--model` selects the operator's *current default*, not auto mode

Probe P3 passed no `--model` and got `claude-sonnet-5` — the account's default —
with **no `session.auto_mode_resolved` event emitted**:

```
[INFO] Using default model: claude-sonnet-5
"model": "capi:claude-sonnet-5:defaultReasoningEffort=medium"
```

This matters because it is the behaviour an operator expects: *"whatever I have
selected is what runs."* Omission is therefore not a degraded fallback — it is
the correct default. (On the free account the default resolved to `auto`, which
is why P1 saw `auto_mode_resolved`; the event's presence still discriminates
auto mode, it just isn't implied by omitting the flag.)

#### ⚠ `request_multiplier` is per-model and not derivable from token prices

Six probes, same trivial prompt, ~14–31 k input tokens each:

| model | `request_multiplier` | `total_nano_aiu` |
|---|---|---|
| `gpt-5.4-mini` | 0.33 | 1 093 650 000 |
| `kimi-k2.7-code` | 1.0 | 1 288 700 000 |
| `claude-sonnet-5` | 1.0 | 2 188 830 000 |
| **`gemini-3.5-flash`** | **14.0** | 2 237 400 000 |

`gemini-3.5-flash` bills **14 premium requests for one call** despite a
*cheaper* per-token rate card than `claude-sonnet-5` (§4d: 150/900 vs 200/1000
nano-AIU). The credit multiplier and the token price are independent axes.
**An adapter must not infer cost from the rate card**, and any "cheap model"
default picked from §4d's table would be wrong.

### 4b. ✅ Free, deterministic model enumeration — via the debug log

ADR-0040 C4 asks for a free way to enumerate. There is one, and it is exact.

A run with an **invalid** `--model` fails *after* the catalog fetch but *before*
any paid call. With `--log-level all --log-dir <dir>`, the catalog lands in the
log verbatim:

```
$ copilot -p "hi" --model "zzz-not-real" --allow-all-tools --log-level all --log-dir $tmp
[DEBUG] [rust:capi_models] fetched models from CAPI /models {"count":46,"models":"[…]"}
[WARNING] Model 'zzz-not-real' from CLI argument is not available. Falling back to next option.
[INFO] Using default model: claude-sonnet-5
[ERROR] Model "zzz-not-real" from --model flag is not available.
exit=1     # zero model calls, zero credits
```

46 entries, each with keys: `id, name, vendor, version, object, preview,
policy, warning_text, capabilities, supported_endpoints, billing,
is_chat_default, is_chat_fallback, model_picker_enabled,
model_picker_category, model_picker_price_category`.

`billing` carries **both the entitlement and the rate card**:

```json
{"restricted_to":["pro_plus","business","enterprise","max"],
 "token_prices":{"batch_size":1000000,
   "default":{"input_price":1000,"output_price":5000,
              "cache_read_price":100,"cache_write_price":1250,
              "max_prompt_tokens":200000},
   "long_context":{…,"max_prompt_tokens":936000}}}
```

**This solves three ADR-0040 questions at once**: enumeration (C4), pricing for
`PriceTable::default` (C4/ADR-0034), and plan detection (below). It is the
single most valuable probe in this spike.

### 4c. Detecting the operator's plan

The plan/SKU is **not** logged in plaintext. Two indirect signals, both free:

- **`[INFO] Using default model: <id>`** in the same failed-model run. Free
  account → `gpt-5-mini`; upgraded account → `claude-sonnet-5`. Cross-referenced
  against `restricted_to`, this pins the tier.
- **The CAPI host**: `api.individual.githubcopilot.com` — the subdomain encodes
  the account class (individual vs business/enterprise).

Tier vocabulary observed in `restricted_to`: `free`, `edu`, `pro`, `pro_plus`,
`individual_trial`, `business`, `enterprise`, `max`. **A model with an empty
`restricted_to` is available to everyone** — on the free account exactly those
(`gpt-5-mini`, `claude-haiku-4.5`) were the auto-mode candidates, which
corroborates the rule.

`is_chat_default: gpt-5-mini` · `is_chat_fallback: gpt-5.3-codex`.

### 4d. The picker-enabled catalog (prices in nano-AIU per 1M tokens)

Only `model_picker_enabled` entries are user-selectable; the rest are internal
(`exec-agent-a/b/c`, `copilot-search-a/b`, `trajectory-compaction`) or legacy
(`gpt-4`, `gpt-3.5-turbo`, embeddings).

| model id | vendor | tiers | in | out | cache rd | cache wr | ctx |
|---|---|---|---|---|---|---|---|
| `claude-haiku-4.5` | Anthropic | **all** | 100 | 500 | 10 | 125 | — |
| `claude-sonnet-4.5` | Anthropic | pro,pro_plus,max,business,enterprise | 300 | 1500 | 30 | 375 | — |
| `claude-sonnet-4.6` | Anthropic | pro,pro_plus,individual_trial,business,enterprise,max | 300 | 1500 | 30 | 375 | 200 000 |
| `claude-sonnet-5` | Anthropic | pro,pro_plus,business,enterprise,max | 200 | 1000 | 20 | 250 | 200 000 |
| `gemini-2.5-pro` | Google | pro,pro_plus,max,business,enterprise,individual_trial,edu | 125 | 1000 | 12 | 0 | — |
| `gemini-3.1-pro-preview` | Google | edu,pro,pro_plus,individual_trial,business,enterprise,max | 200 | 1200 | 20 | 0 | 200 000 |
| `gemini-3.5-flash` | Google | pro,pro_plus,business,enterprise,max | 150 | 900 | 15 | 0 | 200 000 |
| `gpt-5-mini` | Azure OpenAI | **all** | 25 | 200 | 2 | 0 | — |
| `gpt-5.3-codex` | OpenAI | pro,edu,pro_plus,individual_trial,business,enterprise,max | 175 | 1400 | 17 | 0 | 272 000 |
| `gpt-5.4` | OpenAI | pro,pro_plus,individual_trial,business,enterprise,max | 250 | 1500 | 25 | 0 | 272 000 |
| `gpt-5.4-mini` | OpenAI | pro,pro_plus,individual_trial,edu,business,enterprise,max | 75 | 450 | 7 | 0 | 272 000 |
| `gpt-5.6-luna` | OpenAI | pro,pro_plus,business,enterprise,max | 100 | 600 | 10 | 125 | 200 000 |
| `gpt-5.6-terra` | OpenAI | pro,pro_plus,business,enterprise,max | 250 | 1500 | 25 | 312 | 272 000 |
| `kimi-k2.7-code` | Moonshot AI | pro,pro_plus,individual_trial,edu,max,business,enterprise | 95 | 400 | 19 | 0 | 224 000 |
| `mai-code-1-flash-picker` | Microsoft | free,edu,pro,pro_plus,max,business,enterprise | 75 | 450 | 7 | 0 | 128 000 |

Present but **not** picker-enabled, so unreachable via `--model` at any tier:
`claude-opus-4.8`, `claude-opus-4.8-fast`, `claude-opus-4.7`, `claude-opus-4.5`,
`claude-fable-5`, `gpt-5.5`, `gpt-5.6-sol`, `gpt-5.4-nano`. **Note this
contradicts `copilot help config`, which documents them under the `model`
key** — further evidence that the help text is not a contract.

`--context long_context` applies to: `claude-fable-5`, `claude-opus-4.7`,
`claude-opus-4.8`, `claude-opus-4.8-fast`, `claude-sonnet-4.6`,
`claude-sonnet-5`, `gemini-3.1-pro-preview`, `gemini-3.5-flash`, `gpt-5.4`,
`gpt-5.5`, `gpt-5.6-luna`, `gpt-5.6-sol`, `gpt-5.6-terra`.

**Vision is near-universal** — 26 of 46 models advertise it, including every
picker-enabled one. That makes `--attachment` the only open variable for
`ACCEPTS_IMAGES` (§10).

### 4e. Auto-routing is real, disclosed, and per-turn

```json
{"type":"session.auto_mode_resolved","data":{
  "chosenModel":"claude-haiku-4.5","reasoningBucket":"medium",
  "categoryScores":{"reasoning":0.2714,"code_gen":0.353,"tool_use":0.3885,"debugging":0.3436},
  "predictedLabel":"no_reasoning","confidence":0.66,
  "candidateModels":["claude-haiku-4.5","gpt-5-mini"]}}
```

Two runs of comparable prompts routed differently (`claude-haiku-4.5`, then
`gpt-5-mini` with `reasoningBucket: low`). The chosen model is disclosed in
three places: this event, `assistant.message.data.model`, and the
`assistant_usage_events.model` column. **Model attribution must be read
post-hoc, per call** — `Usage::fold_usage`'s heaviest-model rule is exactly
right here, and per-call rows make it accurate.

`session.auto_mode_resolved` is emitted **only in auto mode** — it is absent
when `--model` is pinned, which makes its presence a reliable discriminator for
"the model was chosen for me".

### What does *not* work for enumeration

- **`copilot --silent -p "/model --list --json"` is a paid model round-trip, not
  a CLI listing.** Two historical executions of that exact command are in the
  session store (sessions `e5939c19…` and `5c1dfeab…`, 2026-07-19) and returned
  **two different schemas**: `{"available_models":[…],"default":"auto","note":
  "Could not fetch remote docs; list reflects local environment only…"}` versus
  `{"models":[{"id":"gpt-5-mini",…,"supported_modes":["chat","code","analysis"]}],…}`.
  Each cost 2–3 model calls. **Unusable as a data source.**
- `copilot help config` documents 21 ids under the `model` key
  — **and it is wrong**: 8 of them are not picker-enabled (§4d)
  (`claude-sonnet-5`, `claude-sonnet-4.6`, `claude-sonnet-4.5`,
  `claude-haiku-4.5`, `claude-fable-5`, `claude-opus-4.8`,
  `claude-opus-4.8-fast`, `claude-opus-4.7`, `claude-opus-4.6`,
  `claude-opus-4.5`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`,
  `gpt-5.4`, `gpt-5.3-codex`, `gpt-5.4-mini`, `gpt-5-mini`,
  `gemini-3.1-pro-preview`, `gemini-3.5-flash`, `kimi-k2.7-code`). **This is the
  documented list, not the entitled list** — the probe proves they diverge.
- A **probe loop over `--model <id>`** does not work either: argument validation
  and the "no prompt provided" check both fire *before* model validation, so
  there is no free way to test acceptance one id at a time — a *rejection* is
  free but an *acceptance* costs a real call. The debug-log route (§4b) sidesteps
  this entirely.

### Reasoning effort

`--effort` / `--reasoning-effort`: `none | minimal | low | medium | high |
xhigh | max`. **Verified accepted and applied** (§4a): `--effort high` with
`--model claude-sonnet-5` produced
`"model": "capi:claude-sonnet-5:defaultReasoningEffort=high"` in the debug log —
the effort is encoded into the model handle, not sent as a separate parameter.
Auto-mode emits its own `reasoningBucket`;
`assistant_usage_events.reasoning_effort` records what was used.

#### ⚠ Effort is NOT universal — a fixed default hard-fails (probe P5)

The catalog's per-model `capabilities.supports.reasoning_effort` is `null` for
four picker-enabled models. Passing `--effort` to one of them is a **hard,
pre-flight failure on every run**:

```
$ copilot -p "…" --model kimi-k2.7-code --effort medium
Error: Model "kimi-k2.7-code" does not support reasoning effort configuration (requested: "medium").
exit = 1
$ copilot -p "…" --model kimi-k2.7-code                     # same run, no --effort
exit = 0    assistant.message.data.model = "kimi-k2.7-code"
```

| model | reasoning_effort levels |
|---|---|
| `claude-haiku-4.5`, `claude-sonnet-4.5`, `gemini-2.5-pro`, `kimi-k2.7-code` | **none — passing `--effort` is exit 1** |
| `claude-sonnet-4.6` | low, medium, high, max |
| `claude-sonnet-5` | low, medium, high, xhigh, max |
| `gemini-3.1-pro-preview`, `gpt-5-mini`, `mai-code-1-flash-picker` | low, medium, high |
| `gemini-3.5-flash` | minimal, low, medium, high |
| `gpt-5.3-codex` | low, medium, high, xhigh |
| `gpt-5.4`, `gpt-5.4-mini` | none, low, medium, high, xhigh |
| `gpt-5.6-luna`, `gpt-5.6-terra` | none, low, medium, high, xhigh, max |

**This is the OpenCode `--variant` lesson verbatim (ADR-0005 D3): the flag must
be omitted when unset, never defaulted, or the adapter sends a value the
provider rejects.** A hardcoded `--effort medium` — the Codex D-shape — would
break `--model kimi-k2.7-code` on 100 % of runs.

#### An out-of-range level is silently dropped to the model default — it is NOT clamped

The binary "does this model do effort at all" is the only thing validated. The
level itself is not, and the fallback is **not** the nearest supported level —
it is the model's own default, in *both* directions. Probe P6, three runs
against `gpt-5-mini` (supports exactly `low, medium, high`):

| requested | recorded in `assistant_usage_events.reasoning_effort` |
|---|---|
| `xhigh` — above the ceiling | **`medium`** |
| `minimal` — below the floor | **`medium`** |
| `high` — in range | `high` |

Corroborated on a second model: `--effort minimal` on `claude-sonnet-5`
(supports `low, medium, high, xhigh, max`) also recorded `medium`.

**Asking for `xhigh` on a model that stops at `high` therefore yields *less*
effort than asking for `high`.** The request is inverted, exit is 0, and nothing
in the stream says so. This is the strongest argument for normalising effort in
the adapter rather than passing the operator's string through: the vendor's own
fallback is intent-destroying.

Ordering, per `--help`: `none < minimal < low < medium < high < xhigh < max`.
Note `low`, `medium` and `high` are supported by **every** model that supports
effort at all, so a clamp that never exceeds the request always lands.

The adapter also cannot trust that the effort it asked for is the effort it got.
`assistant_usage_events.reasoning_effort` is the only truth, read post-hoc.

`--context default|long_context` sets the context-window tier for
tiered-pricing models.

---

## 5. C5 — Authentication

**The logged-out signature was captured before login** (ADR-0040 C5 discipline):

```
$ copilot -p "…" --allow-all-tools --output-format json     # logged out
exit = 1
stderr:
  Error: No authentication information found.
  Copilot can be authenticated with GitHub using an OAuth Token or a Fine-Grained
  Personal Access Token.
  To authenticate, you can use any of the following methods:
    • Start 'copilot' and run the '/login' command
    • Set the COPILOT_GITHUB_TOKEN, GH_TOKEN, or GITHUB_TOKEN environment variable
    • Run 'gh auth login' to authenticate with the GitHub CLI
```

stdout was **empty** (0 bytes) — the marker is stderr-only. Candidate detector
marker: `"no authentication information found"`. Remediation command for the
adapter's `AUTH_ERROR_MSG`: **`copilot login`**.

- Login verb: `copilot login [--host <host>]`, OAuth device flow. Also
  `/login` and `/logout` interactively.
- Credential storage: OS credential store, falling back to a plaintext file
  under `~/.copilot/`. `cmdkey /list` showed **no** Copilot entry while logged
  out (only unrelated `git:https://github.com` / `api.github.com` entries).
- Accepted token types: fine-grained PATs (v2) with the **"Copilot Requests"**
  permission, OAuth tokens from the Copilot CLI app, OAuth tokens from the `gh`
  app. **Classic `ghp_` PATs are not supported.**

### ⚠ Env-var cross-contamination hazard

Precedence: `COPILOT_GITHUB_TOKEN` > `GH_TOKEN` > `GITHUB_TOKEN`.

Ralphy's own GitHub work runs through `gh`, and CI/automation contexts routinely
export `GH_TOKEN`/`GITHUB_TOKEN`. If either is set when Ralphy spawns Copilot,
**Copilot silently authenticates as that token's identity**, overriding the
operator's `copilot login`. This is the direct analog of the `ANTHROPIC_API_KEY`
scrub, and it is worse in one respect: the failure is silent and the run
succeeds under the wrong account. In this environment all three were unset, so
the hazard is latent, not observed.

Other auth-adjacent env: `GH_HOST`, `COPILOT_GH_HOST`, `COPILOT_HOME`
(relocates config/state), `COPILOT_OFFLINE`, and the whole
`COPILOT_PROVIDER_*` BYOK family (`BASE_URL`, `TYPE`, `API_KEY`,
`BEARER_TOKEN`, `WIRE_API`, `TRANSPORT`, `MODEL_ID`, `WIRE_MODEL`,
`MAX_PROMPT_TOKENS`, `MAX_OUTPUT_TOKENS`). **Setting `COPILOT_PROVIDER_BASE_URL`
bypasses GitHub auth and model routing entirely** — a BYOK escape hatch that
would also bypass every assumption in this document.

---

## 6. C6 — Usage and the session store

### Topology: SQLite, not files

`~/.copilot/` (overridable with `COPILOT_HOME`):

```
session-store.db  (+ -wal, -shm)   ← the store
session-state/<session-uuid>/{checkpoints,files,research,workspace.yaml}
logs/process-<epoch>-<pid>.log
ide/<uuid>.lock
config.json                        ← machine-managed; user settings live in settings.json
```

`list_session_files` / `session_files_appeared` **do not apply**. This is the
OpenCode topology (a database), not the Claude/Codex/Kimi one.

### The tables that matter

```sql
sessions(id TEXT PK, cwd, repository, host_type, branch, summary,
         created_at, updated_at)

assistant_usage_events(
  id, session_id, turn_index, agent_id, parent_tool_call_id, model NOT NULL,
  input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
  reasoning_tokens, total_nano_aiu, request_multiplier, duration_ms,
  time_to_first_token_ms, inter_token_latency_ms, initiator, api_endpoint,
  reasoning_effort, finish_reason, content_filter_triggered,
  token_details_json, created_at)
  -- indexed on (session_id, id), (session_id, turn_index), (model)
```

Also present: `turns`, `checkpoints`, `session_files`, `session_refs`,
`forge_trajectory_events`, `forge_skill_proposals`, `dynamic_context_items`,
and an FTS5 `search_index`.

### Minted session id → direct key lookup ✅

Probe P1 minted `1138b4fc-b139-44b4-9a3e-a7fbcdd6181b`, passed it as
`--session-id`, and it came back verbatim in `result.sessionId` **and** as the
`sessions.id` primary key:

```
{'id':'1138b4fc-…','cwd':'C:\\Dev\\FinCal','repository':'paulocorcino/FinCal',
 'host_type':'github','branch':'feat/opencode-v2','created_at':'2026-07-20T08:52:57.932Z'}
```

with its usage row:

```
{'turn_index':0,'model':'claude-haiku-4.5','input_tokens':17522,'output_tokens':75,
 'cache_read_tokens':0,'cache_write_tokens':17512,'reasoning_tokens':61,
 'total_nano_aiu':2227500000,'request_multiplier':0.33,'finish_reason':'stop',
 'initiator':'user','api_endpoint':'/v1/messages'}
```

Probe P2 (two calls in one session):

```
turn 0  claude-haiku-4.5  in=22913 out=350  cache_read=0      cache_write=22903  reasoning=159  finish=tool_calls  initiator=user
turn 0  claude-haiku-4.5  in=23345 out=23   cache_read=22903  cache_write=437    reasoning=0    finish=stop        initiator=agent
```

### Cumulative or incremental?

**Incremental — sum, do not keep-last.** The two P2 rows are distinct model
calls within one turn, and `input_tokens` is the *prompt size of that call*
while `cache_read`/`cache_write` describe that call's cache behaviour. Note
`turn_index` is `0` for both, so **`turn_index` is not a per-call key** — only
`id` is. Historical rows confirm the same shape (session `e5939c19…`: three
rows, one turn).

Field mapping to `Usage` is one-to-one and needs no invention:
`input_tokens→input`, `output_tokens→output`, `cache_read_tokens→cache_read`,
`cache_write_tokens→cache_creation`, `model→model`. `reasoning_tokens` has **no
`Usage` slot** (it appears to be a subset of `output_tokens`: 61 of 75 in P1).

### Pricing comes free (ADR-0034)

`token_details_json` carries the actual rate card, in nano-AIU per batch:

```json
[{"batchSize":1000000,"costPerBatch":25000000000,"tokenCount":14924,"tokenType":"input"},
 {"batchSize":1000000,"costPerBatch":2500000000,"tokenCount":0,"tokenType":"cache_read"},
 {"batchSize":1000000,"costPerBatch":200000000000,"tokenCount":356,"tokenType":"output"}]
```

### ⚠ Two currencies

`result.usage.premiumRequests` (0.33 in P1 and P2; **0** for the `--model auto`
"say OK" probe) matches `assistant_usage_events.request_multiplier` (0.33), not
a token count. The stream reports **AI credits / premium requests**; the
database reports **tokens**. `Usage` is token-denominated, so the database is
the source of truth and the envelope's `usage` block is a cross-check only.
`total_nano_aiu` is the credit cost × 10⁻⁹.

`/usage`, `/context`, `/statusline quota`, and the `/exit` summary expose this
interactively; none is scriptable.

---

## 7. C7 — Limits

⚠ **Nothing in this section was observed live.** No quota exhaustion was induced
— the probed account's models are non-premium (`request_multiplier` 0.33 and
0.0), which makes deliberately burning a limit impractical.

From `copilot help limits` and `help config`:

- `--max-ai-credits <credits>` — **opt-in, soft cap, minimum 30.** "Usage is
  known only after a model response returns. A response can therefore exceed or
  exhaust the limit before the CLI can observe that it has done so; the next
  model call is then blocked." Sub-30 values are rejected at **argument parse
  time** (`error: option '--max-ai-credits <credits>' argument '1' is
  invalid…`, exit 1) — before model validation.
- Usage accumulates across a whole non-interactive run; **subagents share the
  parent's limit**.
- `continueOnAutoMode` (config, default `false`): "eligible rate limit errors
  (per-model, weekly, or integration limits) trigger an automatic switch to auto
  mode and retry. Does not apply to global rate limits, generic 429s, or BYOK
  providers." **This is a silent-retry mechanism in the family of the OpenCode
  quota-swallowing failure** — a vendor-internal retry that hides the limit from
  the caller. Default-off is fortunate; it must be verified to stay off.
- No documented reset hint, no `Retry-After` surface, no semantic exit code.

**Open question for validation**: which of exit code, a stream record, or prose
carries an exhausted limit, and whether any reset timestamp is recoverable.
Until answered, `Limit(None)` + ADR-0030's synthetic cadence is the only
defensible mapping.

---

## 8. C8 — Skills and prompts

### Skills load with zero work ✅

Copilot discovers skills from `.github/skills/`, `.agents/skills/`, **and
`.claude/skills/`** (project), `~/.copilot/skills/` or `~/.agents/skills/`
(personal), plugins, and `copilot skill add <dir>`.

Probe P1 ran in `C:\Dev\FinCal` and `session.skills_loaded` listed **16 skills
already discovered** from `C:\Dev\FinCal\.agents\skills\` — including
`reviewer` and `staged-plan`, the two Ralphy materializes — plus a builtin
`customize-cloud-agent`. Each entry carries `{name, description, source,
userInvocable, enabled, path}`.

So Ralphy's existing `materialize_assets` → `.ralphy/skills` would **not** be
picked up (wrong directory), but materializing into `.agents/skills/` or
`.claude/skills/` needs no flag at all, and `session.skills_loaded` gives a
**verifiable load receipt** — better than any other vendor, where loading is
assumed.

⚠ Not verified: whether an *invoked* skill behaves correctly end-to-end, only
that it is discovered.

### Native plan mode exists

`--plan`, `--mode plan|interactive|autopilot`, and interactive `/plan`. Also an
`--agent <name>` custom-agent surface and a `--plugin-dir`. ⚠ none exercised;
whether native plan mode can be coerced into writing `.ralphy/plan.md` is
unknown. The other four vendors all rejected native plan mode.

### Instruction files compete with the charter

`copilot init` generates `.github/copilot-instructions.md`; the CLI also loads
`AGENTS.md` "and related files" by default. `--no-custom-instructions` disables
this; `COPILOT_CUSTOM_INSTRUCTIONS_DIRS` adds more. In P1/P2 no instruction file
was reported loaded, but FinCal's `AGENTS.md` presence was not checked.

**Memory is also on by default** in interactive mode (`memory: true`, "agentic
memory (cross-session fact recall)"), though `--enable-memory` implies it is
**disabled in prompt mode** — which is what Ralphy wants.

---

## 9. C9 — Blast radius ⚠ the section that matters most

Copilot ships capabilities that would violate Ralphy's never-push /
never-open-a-PR ethos, **on by default**.

| Capability | Default | Disable |
|---|---|---|
| **Bundled GitHub MCP server** — `github-mcp-server`, `transport: http`, `source: builtin`. Connected in **every** probe (`session.mcp_server_status_changed → connected`), holding the operator's GitHub credential. Its tool surface is a "default CLI subset" expandable to "all toolsets". | **ON** | `--disable-builtin-mcps`, or `--disable-mcp-server github-mcp-server` |
| **Session export to GitHub web/mobile** | **ON** (implied by `--no-remote-export` existing) | `--no-remote-export` |
| **Remote control of the session from GitHub web/mobile** | ON | `--no-remote` |
| `/delegate` — "Send this session to GitHub and Copilot will create a PR; use `--base` to choose the PR target branch" | interactive verb | n/a in `-p` ⚠ unverified |
| `--share-gist` — pushes the session transcript to a secret gist | opt-in | don't pass it |
| **Auto-update mid-run** | ON (off in CI, detected via `CI`/`BUILD_NUMBER`/`RUN_ID`/`SYSTEM_COLLECTIONURI`) | `--no-auto-update`, `COPILOT_AUTO_UPDATE=false` |
| **Background tasks** — `session.background_tasks_changed` fired **6×** in probe P2 | ON | ⚠ no flag found; lifetime beyond process exit unverified |
| Repo/user hooks | ON | `disableAllHooks` |
| Other MCP config sources: `~/.copilot/mcp-config.json`, `.mcp.json`, `.github/mcp.json`, plugins | ON | `--disable-mcp-server`, `--disable-builtin-mcps` |

The GitHub MCP server is the sharp edge: an agent under a `--yolo` charter, with
a connected GitHub MCP holding a real token, can open a PR **without ever
shelling out to `git push`** — bypassing every guard Ralphy has, all of which
operate on the working tree and the process boundary.

Probes P2 and the `--model auto` probe both passed `--disable-builtin-mcps
--no-remote --no-remote-export` and completed normally (exit 0, tool use, commit),
so **disabling all three costs nothing functionally**.

### ✅ The kill switch is verifiable in-band

`--disable-builtin-mcps` does not merely omit the server — it emits a receipt
Ralphy can assert on (probe P3):

```json
{"type":"session.mcp_servers_loaded",
 "data":{"servers":[{"name":"github-mcp-server","status":"disabled",
                     "source":"builtin","transport":"http"}]}}
```

`status: "disabled"` versus P1/P2's `connected`. **The adapter can fail the run
if it ever observes `connected`** — a guard with no analog in the other four
vendors, where hardening is asserted only by the flags passed.

`--secret-env-vars=VAR,…` strips named env values from shell and MCP
environments and redacts them from output — a useful hardening primitive with no
analog in the other adapters.

---

## 10. C10 — Cross-platform and I/O hygiene

- **Binary**: `C:\Users\PICHAU\AppData\Local\WinGet\Links\copilot.exe` — a
  **winget shim**, which `resolve_program`'s PATH+PATHEXT walk finds. No
  `~/.local/bin` special case needed (contrast Kimi).
- **Encoding**: no cp1252 crash with `--output-format json` on redirected
  stdout. `NO_COLOR` honoured; `--no-color` available.
- **Windows shell**: `powershellFlags` config, default
  `["-NoProfile","-NoLogo"]` — the CLI shells out through `pwsh` on Windows, and
  the help warns that changing these flags "will break the runtime".
- **`USE_BUILTIN_RIPGREP`** — a bundled ripgrep is used by default.
- **`ACCEPTS_IMAGES`: `true` — ✅ VERIFIED** (probe P4). `--attachment <path>`
  with a real PNG (`docs/screenshots/100-auth-card-20260718.png`, 18 797 bytes)
  and a prompt asking for the most prominent word in the image returned
  `ATTACH_OK|Registrar|RALPHY_DONE_EXIT`, exit 0 — a word that appears only in
  the image's pixels. The flag is "only valid in non-interactive mode", which is
  exactly Ralphy's mode, and can be repeated. Catalog corroboration:
  `capabilities.supports.vision` is `true` for every picker-enabled model except
  `mai-code-1-flash-picker`, and `claude-sonnet-5`'s limits carry
  `max_prompt_images: 5`, `max_prompt_image_size: 3145728`.
- `--screen-reader`, `--mouse`, `--banner`, `--no-color`, `terminalProgress`
  (OSC 9;4 escapes, default on — worth disabling for clean capture).

---

## 11. Timings

| Probe | Wall time | Note |
|---|---|---|
| P1 — trivial sentinel prompt | 11.7 s | 6.8 s session, ~2.7 s API |
| `--model auto` "say OK" | ~9 s | |
| P2 — 23 KB charter, file write, git commit | 22.2 s | 14.5 s session, 9.4 s API, 2 model calls |
| auth failure | 2.5 s | fails fast |
| model rejection | 5.6 s | after MCP connect, before any model call |

Startup overhead (MCP connect + skill discovery) is ~2–3 s before the first
token. `--disable-builtin-mcps` should trim it.

---

## 12. Open questions blocking a complete ADR

1. **Limits (§7)** — the surface of an exhausted quota is entirely unobserved.
   Is there a semantic exit code, a stream record, or only prose? Any reset hint?
2. ~~**Model entitlement (§4)**~~ — **RESOLVED.** Rejection was plan-scoped;
   pinning works on a paid plan, and §4b gives a free deterministic enumeration
   plus plan detection. Remaining sub-question: is the entitlement re-checked
   mid-run if a plan changes or a quota trips?
3. ~~**stdin prompt channel (§1)**~~ — **RESOLVED.** 24 250 bytes delivered
   intact with both ends verified; see §1.
4. ~~**`--effort` acceptance (§4)**~~ — **RESOLVED**, and the answer changed the
   design: effort is accepted, but four picker-enabled models reject the flag
   outright, and an unsupported level on a supporting model is silently coerced.
   See §4 *Reasoning effort*.
5. **Hooks (§3)** — event names and payload schema unknown; potentially a
   deterministic completion signal. The custom sentinel already works (§3), so
   this is an optimisation, not a gap.
6. ~~**`--attachment` (§10)**~~ — **RESOLVED**: `ACCEPTS_IMAGES = true`, verified.
7. **Background tasks (§9)** — do they outlive process exit? Ralphy's per-issue
   budget assumes the process boundary is the run boundary.
8. **`continueOnAutoMode` (§7)** — confirm it stays off, and that no other
   silent-retry path swallows a limit the way OpenCode does.
9. **Native plan mode (§8)** — reject by analogy with the other four vendors, or
   evaluate?

---

## Appendix — reproduction

```powershell
# static surface (free, no model calls)
copilot --help
copilot help billing commands config environment limits logging permissions providers
copilot login --help ; copilot mcp --help ; copilot skill --help ; copilot plugin --help

# logged-out signature — RUN THIS BEFORE AUTHENTICATING
copilot -p "hi" --allow-all-tools --output-format json    # exit 1, stderr marker

# minted session id + sentinel + structured stream
$sid = [guid]::NewGuid().ToString()
copilot -p "…charter…" --allow-all --output-format json --session-id $sid `
        --no-remote --no-remote-export --disable-builtin-mcps --no-ask-user

# usage harvest (copy the WAL — never open the live DB)
copy ~/.copilot/session-store.db* $tmp/
python -c "import sqlite3;c=sqlite3.connect('session-store.db');
print([dict(zip([d[0] for d in c.description],r)) for r in
c.execute('select * from assistant_usage_events where session_id=?',('$sid',))])"
```

**Side effect to clean up**: probe P2 left commit `49732d59`
*"chore(probe): ralphy copilot adapter spike"* (`.ralphy-probe/hello.txt`) on
`FinCal@feat/opencode-v2`.
