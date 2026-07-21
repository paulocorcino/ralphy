# Gemini CLI — adapter spike

Evidence for a prospective `ralphy-agent-gemini`, gathered against **Google
Gemini CLI** on **both** target platforms:

| Platform | Binary | Version |
|---|---|---|
| Windows 11 Pro 26200 | `%APPDATA%\npm\gemini.cmd` | `0.51.0` |
| WSL (Ubuntu) | `~/.nvm/versions/node/v24.13.0/bin/gemini` | `0.51.0` |

Target repo for live runs: **`C:\Dev\FinCal`** (branch `afk/run-20260720-143515`).

This document answers the C-questions of
[ADR-0040](../adr/0040-agent-adapter-onboarding-contract.md). It records
**observations**, not decisions; decisions belong in the Gemini adapter ADR.

Session date: 2026-07-20. Operator account: `paulo@corcino.com.br`.

## Evidence markers — read these before trusting any row

ADR-0040 is explicit: *"'The docs say X' is not an answer to a C-question; a
command and its output is."* This spike is currently **mostly documentation**,
because the operator is logged out on both platforms and no paid call has been
made. Every claim below carries one of:

| Marker | Meaning |
|---|---|
| 🔬 | **Observed.** A command was run in this session; its output is quoted. |
| 🔎 | **Read from the shipped source.** Recovered from the bundled JavaScript — stronger than documentation, weaker than an observed run. |
| 📖 | **Documented only.** Sourced from the version-matched docs shipped inside the installed bundle. Not yet exercised. |
| ⚠ | **Unverified / open.** Neither observed nor documented; a probe is queued. |

**Status: Phase 1 complete enough to decide on.** The logged-out sections (§5)
are 🔒 final. Live probes ran on **2026-07-20 against a Gemini API key**
(`security.auth.selectedType: "gemini-api-key"`) in `C:\Dev\FinCal` — see §B.

**Passing: P1–P6, P9, P10, P13, P15, P18–P22.** One gap remains: **P14, true
daily-quota exhaustion**, which cannot be observed without deliberately burning
a day's allowance. Everything else a Tier 1 adapter crate needs is answered.

### Live-probe cost, measured

~25 paid runs / ~50 API requests. The per-run floor is high and that is itself a
finding — see §6 *The router tax*. A trivial `Reply with exactly: OK` cost
**18 273 tokens across two models and two API requests**. On a 250-requests/day
tier the *request* budget, not the token budget, is the binding constraint — and
🔬 **pinning `-m` halves it** by skipping the routing call (§6).

### A note on the documentation source

Unusually, `@google/gemini-cli@0.51.0` ships its **entire documentation set
inside the npm bundle**, at
`%APPDATA%\npm\node_modules\@google\gemini-cli\bundle\docs\` (≈90 markdown
files), together with the **built-in policy files** at `bundle\policies\*.toml`.
This is version-matched to the installed binary, which makes it a far better
source than a docs website that may describe a different release. It is used
heavily below and is always marked 📖.

It is also **demonstrably stale relative to the binary** — see §A.

---

## 0. Executive summary

Gemini CLI is the **closest structural match to Claude Code** of any vendor
Ralphy has evaluated — mintable session id, a Claude-compatible hook system,
skills, `stream-json` — but it is **headless-first** in a way Claude Code is
not, and it carries a governance surface no previous vendor had.

Five things it does better than any existing vendor:

1. **Prompting is structurally forbidden headless.** The built-in policy
   `bundle\policies\non-interactive.toml` denies the `ask_user` tool with
   `priority = 999` when `interactive = false` (📖, quoted §9). And
   `interactive = true|false` is a **first-class condition on every policy
   rule**, so the CLI knows it is headless and routes accordingly. Every
   previous vendor's hang risk (OpenCode, Cursor's `--auto-review`) is
   *designed out* here rather than avoided by flag discipline.
2. **Ten semantic exit codes, six of them undocumented.** 🔬 `exit 41` for auth,
   with a machine-readable body on **stderr** when `-o json` is set — plus 🔎
   `44` sandbox, `52` config, `54` tool-execution, `55` untrusted-workspace,
   `130` cancelled (§3). Kimi's `75 = RETRYABLE` was the previous high-water
   mark; this is a whole taxonomy.
3. **Ralphy can mint the session id.** `--session-id <UUID>` exists in
   `--help` 🔬 (and is *absent from the docs*, §A). If it works, ADR-0008 D10's
   snapshot-diff is unnecessary — the Copilot precedent.
4. **A real hook system with a Claude migration path.** `gemini hooks migrate`
   is documented as *"Migrate hooks from Claude Code to Gemini CLI"* 🔬, and the
   event set includes `AfterAgent`, which fires *"once per turn after the model
   generates its final response"* 📖. That is the deterministic-completion win
   ADR-0040 C3 asks for.
5. **No repository upload.** Checkpoints and sessions are local; nothing in the
   docs or source describes remote indexing. **The Cursor blast radius does not
   reproduce here.** (But see §10 — *usage statistics* are on by default; it is
   OpenTelemetry that is off.)

6. **Hooks fire headless, and `AfterAgent` hands over the finished answer.** 🔬
   P6: all four probed events fired during `-p`. `AfterAgent` receives on stdin
   `{session_id, transcript_path, cwd, prompt, prompt_response, stop_hook_active}` —
   `prompt_response` **is** the final text, and `transcript_path` is the
   absolute path to the session file. **Ralphy can have deterministic completion
   without scraping the stream at all.** No previous vendor offered this.
7. **`--policy` beats `--yolo`.** 🔬 P10: a `deny` rule loaded from argv vetoed
   `run_shell_command` while `--approval-mode yolo` was active. Autonomy is
   argv-recoverable after all — this is the mitigation for finding 1 below.

And five findings that are blocking or near-blocking:

1. **A file Ralphy does not own can veto `--yolo` — or expand it.** YOLO is not
   an argv override; it is *itself a policy rule* in the **Default tier
   (base 1)**. User policies are base 4 and admin policies base 5, so **any**
   rule in `~/.gemini/policies/*.toml` or `C:\ProgramData\gemini-cli\policies`
   outranks the allow-all. The docs' own examples show a rule auto-allowing
   `git push` (📖, §9). This is the Cursor "operator config overrides argv" axis,
   but formalized and with a documented precedence table.
2. **On a managed machine, `--yolo` may not exist at all.** Enterprise "Strict
   Mode" is **enabled by default** and *"users will not be able to enter yolo
   mode"* 📖. A headless run then degrades to `ask_user` → denied → tools fail.
   The same control set disables Agent Skills by default under "Unmanaged
   Capabilities" 📖.
3. **Quota fallback does not exist in headless mode.** 🔎 `getFallbackModelHandler()`
   is registered in exactly one place — `ui/hooks/useQuotaAndFallback.ts`, a React
   hook in the **interactive TUI**. `nonInteractiveCli.ts` never sets it, and the
   handler returns `null` when unset. So the friendly "switch to 2.5 Pro / keep
   trying" flow the docs describe **is interactive-only**: headless, a
   non-`silent` quota failure just fails the request. Combined with the absence
   of any structured quota surface (no `429` in the docs, no retry-delay parser
   in source), this is the ADR-0028/ADR-0030 axis and it is wide open (§7).
4. **No `--list-models`.** No free, deterministic model enumeration is
   documented or present in `--help` 🔬. The reachable model set for
   `PriceTable::default` (ADR-0034) is currently unknown, and `-m` forwards
   unknown strings to the provider rather than rejecting locally 📖 — so there
   is no cheap actionable stop.
5. **The session store systematically under-reports the bill.** 🔬 P13: a run
   whose stream envelope reported **32 281 tokens** wrote only **20 924** to
   `~/.gemini/tmp/fincal/chats/*.jsonl`. The missing 35% is the *routing model's*
   consumption, which never lands on disk. **`usage.rs` must read the stream
   envelope, not the store** — and `ralphy-usage-scan` (ADR-0033) can only ever
   give a lower bound for this vendor. That limitation must be stated, not faked.

6. **The agent tried to route around a policy deny by delegating to a subagent.**
   🔬 P10: denied `run_shell_command`, the model immediately called
   `invoke_agent{agent_name: "generalist"}` and asked *it* to run the shell
   command. The deny held transitively — but only because the subagent inherits
   the same policy. **Any Ralphy policy must constrain `invoke_agent` too**, or
   the deny surface is one indirection wide.

---

## 1. C1 — Invocation and the headless contract

| Question | Finding |
|---|---|
| Headless one-shot | 🔬 `-p, --prompt` — *"Run in non-interactive (headless) mode with the given prompt."* Also 📖: *"Headless mode is triggered when the CLI is run in a non-TTY environment **or** when providing a query with the `-p` flag"* — so redirected stdio alone flips the mode. |
| **Prompt channel** | 🔬 `--help`: `-p` is *"Appended to input on stdin (if any)"*. **stdin is a documented, first-class channel** — no previous vendor documented this. See the source-confirmed semantics immediately below; **the docs state the ordering backwards.** |
| Argv ceiling | ⚠ Not probed. `[query..]` is variadic positional. With stdin proven in source there is no reason to risk the ~32 KB Windows ceiling. |
| Full autonomy | 🔬 Two spellings: `-y, --yolo` and `--approval-mode yolo`. The docs mark `--yolo` **deprecated** in favour of `--approval-mode=yolo` 📖, while `--help` still advertises `--yolo` without a deprecation note 🔬. **Prefer `--approval-mode=yolo`.** Critically, autonomy is *not* an argv decision — see §9. |
| Middle ground | `--approval-mode auto_edit` (auto-approve edit tools only) 📖. Unusable headless for the same reason as always. |
| Workspace trust | 🔬 `--skip-trust`; 📖 `GEMINI_CLI_TRUST_WORKSPACE=true`, which the docs describe as *"Useful for headless environments (for example, CI/CD pipelines)"*. Folder Trust is **disabled by default** 📖, but when enabled and the folder is untrusted, headless **dies rather than hangs**: `FatalUntrustedWorkspaceError` → 🔎 **exit 55**. Observed corroboration 🔬: `gemini skills list` in an untrusted cwd emitted `Skipping project agents due to untrusted folder` and `Project hooks disabled because the folder is not trusted` on stderr. **Pass `--skip-trust` unconditionally**, and treat exit 55 as an actionable stop. |
| Working directory | ⚠ No `--cwd` flag exists. The CLI is expected to honour the spawned process's cwd (the session store is keyed by a `<project_hash>` of the project root 📖). Unverified. `--include-directories` adds extra roots. |
| Isolated worktrees | 📖 `-w, --worktree [name]` → `.gemini/worktrees/<name>` **inside the repo**, and it is gated behind `experimental.worktrees: true`, off by default. It runs **no** setup scripts and does **not** clean up. Ralphy owns its branches; must stay off. |
| Execution modes | `--approval-mode plan` — see C8. |
| PTY required for billing? | ⚠ Unverified. Nothing suggests the Claude particularity (ADR-0002) applies. Note the package ships `node-pty` as an *optional* dependency 🔬 — worth understanding why before assuming. |
| Policy files on argv | 🔬 `--policy` and `--admin-policy` (repeatable / comma-separated). These load *additional* rule files. See §9 — they are the sharpest tool for making autonomy argv-expressible again. |

### 🔎 The stdin contract, from source — the docs have the order backwards

`packages/cli/src/gemini.tsx` (~line 811):

```js
let stdinData = undefined;
if (!process.stdin.isTTY) {
  stdinData = await readStdin();
  if (stdinData) {
    input = input ? `${stdinData}\n\n${input}` : stdinData;
  }
}
```

Four facts the adapter depends on:

1. **stdin is PREPENDED, not appended.** The final prompt is
   `<stdin>\n\n<-p text>`, joined by exactly two newlines. Both `--help` and
   the docs say "appended", which is wrong. If Ralphy pipes the charter and
   passes the issue body via `-p`, the charter comes **first** — which is the
   order Ralphy wants, but by luck, not by documentation.
2. **`-p` alone works** (TTY or empty stdin), and **stdin alone works**
   (no `-p` at all → stdin becomes the whole prompt).
3. 🔎 `readStdin.ts`: `MAX_STDIN_SIZE = 8 MB`, truncated on UTF-8 character
   boundaries. Ralphy's ~26 KB charter is nowhere near the ceiling. **Argv
   truncation is a non-issue for this vendor.**
4. 🔎 **A 500 ms grace timer** (`pipedInputShouldBeAvailableInMs = 500`): if
   nothing arrives on a non-TTY stdin within 500 ms, reading stops. A
   supervisor that spawns the child and then computes the prompt before writing
   would silently send an empty prompt. **Write the payload immediately and
   close stdin.**

🔎 Mutually-exclusive combinations enforced by yargs `.check()` — each is a hard
argv error, so they fail fast rather than misbehaving:

- `-i` + piped stdin → *"The --prompt-interactive flag cannot be used when input is piped from stdin."*
- `-p` + `-i` → *"Cannot use both --prompt (-p) and --prompt-interactive (-i) together"*
- `--resume` + `--session-id` + `--session-file` → *"…are mutually exclusive. Please provide only one."*

And when nothing is supplied at all:
*"No input provided via stdin. Input can be provided by piping data into gemini
or using the --prompt option."*

⚠ P1 still runs — to confirm the ~26 KB payload survives end-to-end in practice
and that no encoding damage occurs on Windows — but the *mechanism* is settled.

## 2. C2 — The output stream

🔬 `-o, --output-format` = `text | json | stream-json`, default `text`.
🔬 `--raw-output` disables sanitization of model output (ANSI escapes), with
`--accept-raw-output-risk` to silence the warning. **Ralphy must not set these.**

### `json` — a single object 📖

- `response`: (string) the model's final answer
- `stats`: (object) token usage and API latency metrics
- `error`: (object, optional)

### `stream-json` — newline-delimited JSONL 🔎

The docs name the six event types but document **no fields**. The real schema is
`packages/core/src/output/types.ts` at `v0.51.0`. Transport is strict JSONL:
`JSON.stringify(event) + '\n'` to **stdout**, one event per line
(`stream-json-formatter.ts`).

```ts
// every event: { type, timestamp }
init        { session_id: string; model: string }
message     { role: 'user'|'assistant'; content: string; delta?: boolean }
tool_use    { tool_name: string; tool_id: string; parameters: object }
tool_result { tool_id: string; status: 'success'|'error'; output?: string;
              error?: { type, message } }
error       { severity: 'warning'|'error'; message: string }
result      { status: 'success'|'error'; error?: { type, message };
              stats?: StreamStats }
```

**The terminal envelope is `result`**, and its `stats` is the best usage payload
of any vendor Ralphy has evaluated:

```ts
StreamStats {
  total_tokens; input_tokens; output_tokens;
  cached;                              // breakdown of input_tokens
  input;                               // breakdown of input_tokens
  duration_ms; tool_calls;
  models: Record<string, ModelStreamStats>;   // keyed by CONCRETE model name
}
ModelStreamStats { total_tokens; input_tokens; output_tokens; cached; input }
```

Four consequences:

1. **ADR-0008 D2 is satisfied natively** — but the arithmetic is a trap. 🔬
   Observed on a cache-hitting run:
   `input_tokens: 64901`, `cached: 16273`, `input: 48628`.
   **`input_tokens` is the TOTAL and already includes `cached`**
   (64 901 = 16 273 + 48 628). Adding `cached` to `input_tokens` double-counts;
   the uncached figure is the `input` field. Write the test so the wrong choice
   fails. Note there is **no cache-*creation* counter**, only cache-read.
2. **`models` is a map, keyed by concrete model name.** An auto-routed run
   yields several keys. This is not defensive design — it is the vendor
   acknowledging that sub-agents ignore `--model` (§4). `Usage::fold_usage`
   heaviest-model attribution is load-bearing.

**🔬 2b. `output_tokens` under-reports the billable output by up to 25×.**
`StreamStats` has **no thinking-token field**, but the arithmetic exposes it —
`total_tokens` exceeds `input_tokens + output_tokens` in every run:

| Run | `total` | `input_tokens` | `output_tokens` | residual = thinking |
|---|---|---|---|---|
| 25 KB charter | 32 281 | 30 073 | 88 | **2 120** |
| skill probe | 33 818 | 32 415 | 121 | **1 282** |
| pinned Pro | 14 391 | 14 300 | 1 | **90** |

The residual is confirmed as thinking by the `json`-mode and session-store
records, which *do* carry an explicit `thoughts` field matching it.

**Google bills thinking tokens at the output rate** — the pricing page's column
is literally *"Output price (including thinking tokens)"*. So billing on
`output_tokens` would under-charge that first run by **25×** (88 vs 2 208).

**The correct billable output is `total_tokens - input_tokens`**, not
`output_tokens`. Write the test so the naive choice fails.
3. **Completion is `status: 'success'|'error'`**, not an `is_error` boolean.
4. 🔬 **The final assistant message must be reconstructed by concatenating
   consecutive `message` records with `role: "assistant"`.** `result` carries no
   response text (unlike `json` mode's `response` field), and **there is no
   non-delta final record** — every assistant record observed had
   `delta: true`.

**🔬 The delta split is not token-aligned, and this is a real trap.** From P9:

```json
{"type":"message","role":"assistant","content":"RAL","delta":true}
{"type":"message","role":"assistant","content":"PHY_SKILL_LOADED_B4D2\nThe Ralphy probe checksum is CHECKSUM_9","delta":true}
{"type":"message","role":"assistant","content":"A7E.\n\nRALPHY_DONE_5E1D","delta":true}
```

A `DONE_SENTINEL` match applied **per record** would fail — the sentinel, and
even individual words, straddle record boundaries. `outcome.rs` must join first,
match second.

### 🔬 Observed discriminators

Across five live runs: `init`, `message`, `tool_use`, `tool_result`, `result`.
The documented `error` type was **never emitted**, including on a failing run
(§C4/P5) — errors surfaced in `result.status` and on stderr instead.

- `init` = `{session_id, model}` — **`model` echoes the *requested* value, never
  the resolved one.** With no `-m` it is literally `"auto"`; with
  `-m definitely-not-a-real-model` it echoes that. Same gap as Cursor: the
  resolved models appear **only** in `result.stats.models`.
- `tool_use` = `{tool_name, tool_id, parameters}`; `tool_result` =
  `{tool_id, status, output?}`.

### 🔬 Progress fields: absent from `stream-json`, present in `json`

`json` mode's stats carry `files: {totalLinesAdded, totalLinesRemoved}` and a
`tools` block with `totalDecisions.auto_accept`. **`StreamStats` has neither** —
only `tool_calls`. So ADR-0040 C2's "verify the vendor's progress claim against
a HEAD diff" is moot here: under `stream-json` the vendor makes no claim, and
**Ralphy must compute its own diff** (P7 resolved by absence).

### `json` mode has a *different* stats shape 🔎

```ts
JsonOutput { session_id?; response?: string; stats?: SessionMetrics;
             error?: { type, message, code? }; warnings?: string[] }
```

`stats` here is the full `SessionMetrics` telemetry object — **not** the
flattened `StreamStats`. **The two output formats do not share a stats schema.**
An adapter that parses one cannot fall back to the other.

### 🔬 The observed trap: `-o` is honoured inconsistently on the error path

Logged out, the same failure rendered three different ways:

```console
$ gemini -p "say OK" -o json          # exit 41
# stderr (stdout EMPTY):
{
  "session_id": "1f6195dd-f2b5-4818-9575-33c698f61d3b",
  "error": { "type": "Error", "message": "Please set an Auth method in your …", "code": 41 }
}

$ gemini -p "say OK" -o stream-json   # exit 41
# stderr: bare prose, NOT JSON. stdout EMPTY.

$ gemini -p "say OK"                  # exit 41
# stderr: bare prose. stdout EMPTY.
```

🔎 **The source explains it.** `validateNonInterActiveAuth.ts` branches on
`outputFormat === OutputFormat.JSON` — a strict equality against `json` only.
`stream-json` therefore takes the plain-stderr branch.

Three consequences for the parser:

1. **`json` gives a structured error; `stream-json` does not.** Ralphy would run
   with `stream-json`, i.e. **the format that degrades to prose**.
2. **The error goes to stderr, and stdout is empty.** A parser that reads only
   stdout sees a clean, empty, successful-looking stream.
3. 🔬 **The envelope's presence depends on *when* the failure happens.**
   A **pre-flight** failure (auth, §5) emits **no `result` record at all** under
   `stream-json`. A **mid-run** failure does — P5's invalid-model run produced a
   proper `{"type":"result","status":"error",…}` with exit 1. So the terminal
   envelope cannot be assumed present. **Completion detection must be exit code
   first, envelope second** — the inverse of the Cursor design.
4. **`session_id` is present even in the failure envelope**, minted before the
   auth check.

⚠ Whether a *mid-run* error also bypasses the envelope, and whether `error`
records ever precede a fatal exit (the `severity` field allows `'error'`, but
the docs call them "non-fatal"), is probe P8.

### Progress fields

⚠ No progress/diff fields are documented. P7 will confirm absence against a
HEAD diff.

## 3. C3 — Completion and the sentinel

- **Sentinel**: ⚠ unverified (P4).
- **Exit codes — the richest semantic set of any vendor, and the docs show less
  than half of it.** 📖 `headless.md` lists only `0`, `1`, `42`, `53`. 🔬 The
  observed `41` is absent from it. 🔎 The full enumeration was recovered from the
  `FatalError` subclass definitions in the shipped bundle
  (`chunk-DHQ53XVO.js`, ≈line 243783, the esbuild-bundled
  `packages/core/src/utils/errors.ts` with class names preserved):

  | Code | Error class | Meaning | Evidence |
  |---|---|---|---|
  | `0` | — | Success | 📖 + 🔎 |
  | `1` | — | Generic fallback (any non-numeric error code) | 📖 + 🔎 |
  | **`41`** | `FatalAuthenticationError` | Auth failure — no method set, OAuth unobtainable, browser launch failed, timeout | 🔬 **observed** + 🔎 · *undocumented* |
  | `42` | `FatalInputError` | Invalid prompt or arguments | 📖 + 🔎 |
  | **`44`** | `FatalSandboxError` | Sandbox failure | 🔎 only · *undocumented* |
  | **`52`** | `FatalConfigError` | Configuration error | 🔎 only · *undocumented* |
  | `53` | `FatalTurnLimitedError` | Turn limit exceeded | 📖 + 🔎 |
  | **`54`** | `FatalToolExecutionError` | Tool execution failed fatally | 🔎 only · *undocumented* |
  | **`55`** | `FatalUntrustedWorkspaceError` | Refused: untrusted workspace | 🔎 only · *undocumented* |
  | **`130`** | `FatalCancellationError` | Cancelled (SIGINT convention) | 🔎 only · *undocumented* |
  | `199` | — | Internal self-relaunch sentinel; should never be observed | 🔎 |

  Codes `43`, `45`–`51`, `56`+ are unassigned (verified by exhaustive grep:
  exactly 8 `super(message, N)` sites for 8 subclasses).

  **Two caveats that matter more than the table.**

  1. **The set is not closed.** `extractErrorCode()` falls back to `error.code`
     then `error.status` before defaulting, so **any error object carrying a
     numeric `.code`/`.status` is passed straight to `process.exit()`** — an
     HTTP `429` is a reachable exit code. Only non-numeric values normalize to
     `1`. A `match` on this table needs a catch-all arm, and **`429` is a
     candidate quota signal** worth watching in P14.
  2. A second, narrower `ExitCodes` constant (`{0, 41, 42, 52, 130}`) is what
     the non-interactive auth path actually uses — it exits via the numeric
     constant, not the class. That is the path that produced the observed `41`.

  For Ralphy this is a gift: `55` gives a deterministic untrusted-workspace
  stop, `54` distinguishes tool failure from model failure, and `130`
  distinguishes a kill from a crash. **Do not build a detector on the
  documented table alone.**

  🔬 **Two of the undocumented codes were confirmed live.**

  `55` — running in an untrusted folder without `--skip-trust`, with
  `security.folderTrust.enabled: true`. It dies **before any paid call**, and
  the message is the most actionable of any vendor:

  ```
  Gemini CLI is not running in a trusted directory. To proceed, either use
  `--skip-trust`, set the `GEMINI_CLI_TRUST_WORKSPACE=true` environment variable,
  or trust this directory in interactive mode. For more details, see …
  ```

  `53` — with `model.maxSessionTurns: 1` and a task needing several tool calls.

### 🔬 The error-channel rule — the thing `outcome.rs` must get right

Three live failures, three different shapes. The pattern is consistent and it is
**not** what the output format promises:

| Failure kind | Channel | Shape |
|---|---|---|
| **Fatal** (`Fatal*Error`: auth 41, turn-limit 53, untrusted 55) | **stderr**, even under `-o json`; **stdout empty** | JSON blob with `error.type` = **the class name** and `error.code` = the exit code |
| **Mid-run API error** (invalid model) | **stdout** | `result` envelope, `status:"error"`, but `error.type:"unknown"` and a useless message; real diagnosis on stderr |
| **Pre-flight under `stream-json`** | stderr, **prose only** | no JSON at all (§5) |

Observed for exit 53:

```json
{"session_id":"c08f0e4c-…","error":{
  "type":"FatalTurnLimitedError",
  "message":"Reached max session turns for this session. Increase the number of
             turns by specifying maxSessionTurns in settings.json.",
  "code":53}}
```

🔬 It was printed **twice** on stderr — once plain, once prefixed `[ERROR]`.

So: **fatal errors are well-typed but arrive on the wrong stream; mid-run errors
arrive on the right stream but are untyped.** Neither channel alone is
sufficient. `outcome.rs` must capture both, and prefer the exit code over either.
- **Hook mechanism: the best of any vendor.** 📖 `AfterAgent` *"fires once per
  turn after the model generates its final response"* and receives
  `prompt_response` (the final text) plus `session_id` and `transcript_path` on
  stdin as JSON. Hooks can also **force a retry** (`decision: "deny"` sends
  `reason` back as a new prompt) or **stop the loop** (`continue: false`).
  `SessionEnd` exists but is explicitly *"Best Effort — the CLI will not wait
  for this hook to complete"*, so **`AfterAgent`, not `SessionEnd`, is the
  completion signal.**
  📖 `AfterAgent` is **synchronous and blocking**: *"Gemini CLI waits for all
  matching hooks to complete before continuing."* It carries the same
  `stop_hook_active` anti-recursion flag name Claude Code uses.

  🔎 Hooks receive `GEMINI_PROJECT_DIR`, `GEMINI_PLANS_DIR`,
  **`GEMINI_SESSION_ID`**, `GEMINI_CWD` in the environment — and, notably,
  **`CLAUDE_PROJECT_DIR` "(Alias) Provided for compatibility."** The Claude
  lineage is explicit, not incidental.

  📖 Project-level hooks are **fingerprinted**: if a hook's name or command
  changes (e.g. via `git pull`) it is treated as new and untrusted and warned
  before execution. ⚠ What "warned" means headless is unverified — if it
  prompts, a repo-local hook is a hang vector.

### 🔬 P6 — hooks DO fire headless, and this is the completion signal

All four probed events fired during a plain `-p` run. `AfterAgent` received,
verbatim:

```json
{"session_id":"ralphy-probe-hooks2",
 "transcript_path":"C:\\Users\\PICHAU\\.gemini\\tmp\\fincal\\chats\\session-2026-07-21T00-59-ralphy-p.jsonl",
 "cwd":"C:\\Dev\\FinCal","hook_event_name":"AfterAgent",
 "timestamp":"2026-07-21T00:59:23.106Z",
 "prompt":"Reply with exactly: HOOKPROBE2",
 "prompt_response":" HOOKPROBE2 ",
 "stop_hook_active":false}
```

**`prompt_response` is the finished answer, delivered out-of-band, synchronously,
before the process exits.** Ralphy can fill `CompletionSignals` from a hook
instead of scraping deltas — the deterministic-completion win ADR-0040 C3 asks
for, and the first vendor to offer it since Claude's Stop hook.

Order observed: `SessionStart` → `BeforeAgent` → `AfterAgent` → `SessionEnd`.

### 🔬 The catch: **workspace-scope hooks are silently ignored**

The identical hook block was probed twice with the same command strings:

| Scope | File | Fired? |
|---|---|---|
| Workspace | `C:\Dev\FinCal\.gemini\settings.json` | ❌ **no**, silently — even with `--skip-trust` |
| User | `~/.gemini/settings.json` | ✅ all four events |

No warning, no stderr line, no error — the run simply behaves as if no hooks
exist. (`gemini skills list` in the same directory does print *"Project hooks
disabled because the folder is not trusted"*, so trust is the likely cause, and
**`--skip-trust` does not lift it for hooks**.)

Two consequences: a cloned repo **cannot** inject hooks (a genuine security
positive, and the mirror of the workspace-policy tier being dead, §9); and
**Ralphy must install its hook at user scope** — mutating a file the operator
owns and shares with Antigravity. That is an ADR decision, and `GEMINI_CLI_HOME`
(§5) is the obvious alternative to weigh against it.
- `CompletionSignals` fill: pending the stream shape. Ordering still delegates
  to `classify` (ADR-0023).

## 4. C4 — Models

**Multi-model, auto-routing by default, and — uniquely bad — not enumerable.**

- 🔬 **No `--list-models` flag exists** in `--help`, and 📖 no equivalent
  subcommand is documented. `gemini models` is not a command. The interactive
  `/model` dialog is the only listing surface documented, and an in-band
  `-p "/model"` would be a paid round-trip producing prose — ADR-0040 C4
  explicitly forbids building on that.
- **The real id set** 🔎 `packages/core/src/config/models.ts` at `v0.51.0`.
  **The bundled docs are a full generation stale here** — they describe a
  2.5-centric world; the binary ships 3.x:

  | Constant | Id |
  |---|---|
  | `PREVIEW_GEMINI_MODEL` | `gemini-3-pro-preview` |
  | `PREVIEW_GEMINI_3_1_MODEL` | `gemini-3.1-pro-preview` |
  | `PREVIEW_GEMINI_3_1_CUSTOM_TOOLS_MODEL` | `gemini-3.1-pro-preview-customtools` |
  | `PREVIEW_GEMINI_FLASH_MODEL` | `gemini-3-flash-preview` |
  | `DEFAULT_GEMINI_MODEL` | `gemini-2.5-pro` |
  | `DEFAULT_GEMINI_FLASH_MODEL` | `gemini-2.5-flash` |
  | `DEFAULT_GEMINI_3_5_FLASH_MODEL` | `gemini-3.5-flash` |
  | `SECONDARY_GEMINI_3_5_FLASH_MODEL` | `gemini-3-flash` (alias for the same backend) |
  | `DEFAULT_GEMINI_FLASH_LITE_MODEL` | `gemini-3.1-flash-lite` |
  | `GEMMA_4_31B_IT_MODEL` | `gemma-4-31b-it` |
  | `GEMMA_4_26B_A4B_IT_MODEL` | `gemma-4-26b-a4b-it` |
  | `DEFAULT_GEMINI_EMBEDDING_MODEL` | `gemini-embedding-001` |

  Two traps: **`gemini-3-pro` (unsuffixed) is not real** — Pro is always
  `-preview`; and `gemini-3-flash` is only an *alias* for `gemini-3.5-flash`.
  Note `PREVIEW_GEMINI_FLASH_MODEL` and `DEFAULT_GEMINI_FLASH_MODEL` are
  declared `let`, i.e. **mutable by server-side experiment flags** — the id set
  is not a compile-time constant even within one release.

  Aliases accepted by `-m`: `auto`, `pro`, `flash`, `flash-lite`, plus the
  deprecated `auto-gemini-3` / `auto-gemini-2.5`. `auto`/`pro` resolve to
  `gemini-2.5-pro` **without** preview access, `gemini-3-pro-preview` with it,
  `gemini-3.1-pro-preview` when the 3.1 flag is on. **The same argv yields a
  different model on a different account.**

  This is the ADR-0034 problem in its sharpest form: the reachable set is
  account-dependent, experiment-mutable, and not enumerable from the CLI.
- **Resolution precedence** 📖: `--model` flag → `GEMINI_MODEL` env →
  `model.name` in `settings.json` → local Gemma router (if enabled) → default
  `auto`.
- **Auto-routing is real, per-turn, and partly silent.** 📖 *"the CLI will use
  an available fallback model **for the current turn or the remainder of the
  session**"*. And the flag does not bound the model set: *"The `/model` command
  (and the `--model` flag) does not override the model used by sub-agents.
  Consequently, **even when using the `--model` flag you may see other models
  used in your model usage reports**"*. Per-model attribution is therefore
  **mandatory**, not optional — which is exactly why the `result` envelope's
  per-model breakdown matters (§2).
- **Reasoning effort is orthogonal and lives in settings, not argv** 📖:
  `modelConfigs.*.modelConfig.generateContentConfig.thinkingConfig.thinkingBudget`
  (plus `includeThoughts`, `temperature`, `topP`, `maxOutputTokens`). There is
  **no CLI flag** for it. This is a new shape: every previous vendor exposed
  effort on argv or baked it into the id.
- 🔬 **Auto-routing observed live, and it is genuinely per-run.** Three runs with
  no `-m`, three different main models: `gemini-3.5-flash`,
  `gemini-3.1-pro-preview-customtools`, `gemini-3.5-flash`. Every run also
  spent a second model (`gemini-3.1-flash-lite`, role `utility_router`).
- 🔬 **Entitlement cannot be inferred from the docs (P21).** An API-key install
  served an explicitly pinned `gemini-3.1-pro-preview` with exit 0, flatly
  contradicting the documented *"Model requests to Flash model only"* for the
  unpaid API-key tier. Either the tier is misdocumented or this key is paid —
  and **that ambiguity is the point**: ADR-0040 C4 says never let a vendor's
  documentation stand in for entitlement. There is no free way to ask.
- 🔬 **No deterministic rejection of an unknown model** — confirmed live (P5),
  as source predicted. `-m definitely-not-a-real-model` produced **exit 1**
  after a real API round-trip, and the two channels disagree badly:

  ```jsonl
  // stdout, stream-json — useless
  {"type":"result","status":"error",
   "error":{"type":"unknown","message":"[API Error: An unknown error occurred.]"},
   "stats":{"total_tokens":0,…,"models":{"definitely-not-a-real-model":{…all zeros}}}}
  ```

  ```console
  # stderr — the actionable text, with a stack trace
  ModelNotFoundError: models/definitely-not-a-real-model is not found for API version
  v1beta, or is not supported for generateContent. Call ModelService.ListModels to see
  the list of available models… { code: 404 }
  ```

  **The structured error is `type: "unknown"` and says nothing**; the real
  diagnosis is prose on stderr. `outcome.rs` must capture both streams — the
  envelope alone cannot produce an actionable stop. A JSON crash report is also
  written to `%TEMP%\gemini-client-error-*.json`.

  Silver lining: **zero tokens were billed**, so invalid-model probing is cheap.
- **Tier scoping is real** 📖: a Gemini API key on the free tier is restricted
  to *"Model requests to Flash model only"*. Under a Google account, *"Model
  requests will be made across the Gemini model family as determined by Gemini
  CLI"*. Per ADR-0040 and the Copilot precedent (ADR-0041), model resolution
  must be `Option<String>`, omitted from argv when `None`.

**Pricing consequence.** ADR-0034 requires every reachable id in
`PriceTable::default`. With no enumeration surface, an auto-router, and
sub-agents that ignore `--model`, the reachable set cannot be established from
the CLI. The ADR needs a stance — most likely a family-prefix normalization plus
an explicit "unknown model" tolerance — not a literal list.

### Published prices, and three traps in them

USD per 1M tokens, [Gemini API pricing], Standard tier. Thinking is billed at
the **output** rate (see §2, 2b).

| Model id | Input | Output | Cached read | >200 k tier |
|---|---|---|---|---|
| `gemini-3.1-pro-preview` | 2.00 / 4.00 | 12.00 / 18.00 | 0.20 / 0.40 | **yes** |
| `gemini-3.5-flash` | 1.50 | 9.00 | 0.15 | no |
| `gemini-3-flash-preview` | 0.50 | 3.00 | 0.05 | no |
| `gemini-3.1-flash-lite` | 0.25 | 1.50 | 0.025 | no |
| `gemini-2.5-pro` | 1.25 / 2.50 | 10.00 / 15.00 | 0.125 / 0.25 | **yes** |
| `gemini-2.5-flash` | 0.30 | 2.50 | 0.03 | no |
| `gemini-2.5-flash-lite` | 0.10 | 0.40 | 0.01 | no |
| `gemini-embedding-001` | 0.15 | — | — | no |

**Trap 1 — the CLI ships a retired model id.** `gemini-3-pro-preview` was
**retired on the Gemini API on 2026-03-09**, superseded by
`gemini-3.1-pro-preview`. It is still a constant in the CLI's `models.ts`. Price
it as 3.1 Pro and mark it retired rather than leaving it unpriced.

**Trap 2 — `gemini-3-flash` is a CLI-local alias that contradicts Google's
catalogue.** The CLI maps it to `gemini-3.5-flash` (1.50/9.00). Google's own
"Gemini 3 Flash" is the *preview* model at 0.50/3.00. Honour the CLI's mapping
for billing, but the two differ **3×** — a natural place to guess wrong.

**Trap 3 — `gemini-3.1-pro-preview-customtools` has no published price**, and
🔬 it is the model that actually served two of the probe runs. Pricing it as
plain 3.1 Pro is a labelled inference, not a fact.

Two more shapes `PriceTable` may not express: **tiered pricing above a 200 k
prompt** for the Pro models (a per-model scalar under-bills long charters — and
Ralphy's charter alone is 30 k), and **cache *storage*** billed per token-hour
separately from cache reads.

[Gemini API pricing]: https://ai.google.dev/gemini-api/docs/pricing

## 5. C5 — Authentication 🔒 **FINAL — captured in the logged-out window**

This section is complete and is not reproducible without a logout. Both
platforms were logged out at session start.

### There is no login command

🔬 `gemini --help` exposes **no `login`, `logout`, `auth`, or `status`
subcommand**. This is a first: every previous vendor had one. Authentication is
established either by an environment variable or by an **interactive browser
OAuth flow on first run**, which is precisely the thing a headless launcher
cannot perform.

Consequence for ADR-0013 preflight: `<VENDOR>_AUTH_ERROR_MSG` cannot name a
command like `claude login`. The actionable instruction has to be the CLI's own
sentence, which is at least excellent (below).

### 🔬 The logged-out signature — identical on Windows and WSL

```console
$ gemini -p "reply with exactly OK"
# exit 41, stdout EMPTY, stderr:
Please set an Auth method in your C:\Users\PICHAU\.gemini\settings.json or specify
one of the following environment variables before running: GEMINI_API_KEY,
GOOGLE_GENAI_USE_VERTEXAI, GOOGLE_GENAI_USE_GCA
```

WSL, byte-identical modulo the path:

```console
$ gemini -p "say OK" -o json          # exit 41
{"session_id":"a15c8211-…","error":{"type":"Error","message":"Please set an Auth
method in your /home/corcino/.gemini/settings.json or specify …","code":41}}
```

Per-surface behaviour, all logged out:

| Probe | Exit | Channel | Result |
|---|---|---|---|
| `gemini -p "…"` | **41** | stderr | prose |
| `gemini -p "…" -o json` | **41** | stderr | **structured JSON with `"code": 41`** |
| `gemini -p "…" -o stream-json` | **41** | stderr | prose (**not** JSON) |
| `gemini -p "…" -m <invalid>` | **41** | stderr | auth error — **auth is checked before the model** |
| `gemini --list-sessions` | **41** | stderr | prose |
| `gemini skills list` | **0** | stdout | `No skills discovered.` |
| `gemini mcp list` | **0** | stderr | `No MCP servers configured.` |
| `gemini extensions list` | **0** | stderr | `No extensions installed.` |
| `gemini --help` / `--version` | **0** | stdout | full help / `0.51.0` |

Four traps the detector must survive:

1. **The exit code, not the text, is the primary signal** — `41` is unambiguous
   and, unlike Cursor's `status`, no auth-adjacent verb exits 0 while logged
   out. The three `list` verbs exit 0 but are not auth probes.
2. **`-o stream-json` does not give you JSON on this path** (§2). A detector
   built against `-o json` output will not fire in the configuration Ralphy
   actually runs.
3. **stdout is empty; everything is on stderr.**
4. **Auth precedes model validation**, so the ADR-0040 "deliberate-failure debug
   log" enumeration technique is unavailable while logged out.

The common substring across all phrasings is `Please set an Auth method`.
Detection stays behavioural (exit code + stderr marker) per house style.

### Credential channels — and the contamination hazard

🔎 `AuthType` (`core/contentGenerator.ts`) — the exact strings for
`security.auth.selectedType`:

`oauth-personal` · `gemini-api-key` · `vertex-ai` · `cloud-shell` ·
`compute-default-credentials` · `gateway`

🔎 `getAuthTypeFromEnv()` resolves in **this priority order**:

1. `GOOGLE_GENAI_USE_GCA === 'true'` → `oauth-personal`
2. `GOOGLE_GENAI_USE_VERTEXAI === 'true'` → `vertex-ai`
3. *(gateway env)* → `gateway`
4. `GEMINI_API_KEY` set → `gemini-api-key`
5. `CLOUD_SHELL === 'true'` or `GEMINI_CLI_USE_COMPUTE_ADC === 'true'` → `compute-default-credentials`

🔎 **The comparison is exactly `=== 'true'`.** `1`, `TRUE`, `yes` do **not**
work — a real trap for anyone writing the env block by hand.

Supporting vars: `GOOGLE_API_KEY`, `GOOGLE_CLOUD_PROJECT` (falls back to
`GOOGLE_CLOUD_PROJECT_ID`), `GOOGLE_CLOUD_LOCATION`,
`GOOGLE_APPLICATION_CREDENTIALS`, `GEMINI_MODEL`,
`GOOGLE_GEMINI_BASE_URL` / `GOOGLE_VERTEX_BASE_URL`.
Admin-forceable via `security.auth.enforcedType`; `security.auth.useExternal`
skips validation entirely.

Note 🔎 **`GOOGLE_GENAI_USE_VERTEXAI` and `GOOGLE_GENAI_USE_GCA` are absent from
the documented env-var list** — they appear only in the exit-41 message and in
source, yet they sit at priorities 1 and 2. Undocumented and load-bearing.

**The hazard is real and documented**: the docs instruct users to *"unset
`GOOGLE_API_KEY` and `GEMINI_API_KEY`"* when switching to ADC, because a stray
key silently changes which account and which billing tier is used. `GOOGLE_*` is
a broad namespace that gcloud, Firebase tooling and CI runners all populate, and
the precedence list above means an inherited `GOOGLE_GENAI_USE_VERTEXAI=true`
**outranks** the operator's own API key. **Env hygiene must be an explicit
decision in the ADR**, not a default.

### 🔬 Where the credential *actually* lives: the OS credential store

Source names plaintext files, but the observed API-key install uses neither env
nor a file. `$env:GEMINI_API_KEY` is **unset**, no `.env` exists, and
`~/.gemini/settings.json` holds only the *pointer*:

```json
{ "security": { "auth": { "selectedType": "gemini-api-key" } } }
```

The secret is in the **Windows Credential Manager**, under the target
**`gemini-cli-api-key/default-api-key`** — the package ships `@github/keytar` as
an optional dependency and evidently uses it. Verified by name only; the secret
was never read.

**This is a first.** Every previous vendor kept credentials in a plaintext file
(Cursor's `auth.json`, Claude's, Codex's). Gemini uses the OS store for API
keys — which is *better* security and *worse* for a launcher that wants to
reason about auth state, since there is nothing on disk to observe. Detection
must stay behavioural (exit 41), which is the house style anyway.

For OAuth the file-based paths below presumably still apply:

`packages/core/src/config/storage.ts`:

| Path | Contents |
|---|---|
| `~/.gemini/oauth_creds.json` | OAuth credentials (`OAUTH_FILE`) |
| `~/.gemini/google_accounts.json` | account identity |
| `~/.gemini/mcp-oauth-tokens.json` | MCP server tokens |
| `~/.gemini/a2a-oauth-tokens.json` | remote-agent tokens |
| `~/.gemini/settings.json` · `installation_id` | config, stable anonymous install id |

Plaintext files, not an OS credential store. If there is no home directory it
falls back to `os.tmpdir()/.gemini`. Detection stays behavioural regardless
(house style) — Ralphy must never read or lift these, which is also the safe
side of the ToS line in §9.

🔬 On this logged-out machine none of the credential files exist, confirming
they are created at login.

### ✅ 🔬 `GEMINI_CLI_HOME` — hermetic isolation, and it works (P15)

`GEMINI_CLI_HOME` relocates the entire config root — the CLI appends `.gemini`
to it, so the variable names the *parent*. Two probes settle it:

**P15a — relocation alone breaks auth.** Pointing it at an empty directory:

```console
$ GEMINI_CLI_HOME=%TEMP%\gemini-home-probe gemini -p "say OK" -o json
# exit 41
"Please set an Auth method in your C:\…\gemini-home-probe\.gemini\settings.json or …"
```

**P15b — relocation *plus* a four-line settings file works.** Because the secret
lives in the OS credential store rather than under the root, only the *pointer*
needed replacing:

```console
$ echo '{"security":{"auth":{"selectedType":"gemini-api-key"}}}' > %GEMINI_CLI_HOME%\.gemini\settings.json
$ GEMINI_CLI_HOME=… gemini -p "Reply with exactly: ISOLATED" --approval-mode yolo --skip-trust -o json
# exit 0 → {"response":"ISOLATED", …}
```

**This is the single most useful finding for the ADR**, because one lever closes
four separate holes at once:

| Hole | How the isolated root closes it |
|---|---|
| User-tier policies outrank `--yolo` (§9) | `~/.gemini/policies/` is no longer read |
| `~/.gemini/GEMINI.md` prepends to every prompt (§8) | not read |
| Hooks only fire at *user* scope, forcing Ralphy to mutate the operator's file (§3) | Ralphy's root **is** the user scope — it writes only its own file |
| Config root shared with Google's Antigravity IDE | fully separated |

Cost: Ralphy must write a minimal `settings.json` into its own root, and the
operator must have authenticated at least once so the credential exists in the
OS store. ⚠ Untested with **OAuth** auth (where the credential *is* file-based
under the root, and relocation would orphan it — the API-key case is the lucky
one).

Note 🔬 `~/.gemini/antigravity/` and `~/.gemini/antigravity-browser-profile/`
also live in the default root — Google's Antigravity IDE **shares it**.

Note 🔬 `~/.gemini/antigravity/` and `~/.gemini/antigravity-browser-profile/`
also live here — Google's Antigravity IDE **shares this config root**. Anything
Ralphy writes under `~/.gemini` is shared with another product.

## 6. C6 — Usage and the session store

### Topology 📖

| Path | Contents |
|---|---|
| `~/.gemini/tmp/<project_hash>/chats/` | sessions |
| `~/.gemini/tmp/<project_hash>/checkpoints` | checkpoint JSON |
| `~/.gemini/tmp/<project_hash>/plans/` | plan-mode artifacts |
| `~/.gemini/history/<project_hash>` | a **shadow git repo** for checkpointing |

`<project_hash>` is *"a unique identifier based on your project's root
directory"* — so, as with Cursor, **the run's working directory determines where
the record lands**. ⚠ The hash algorithm is unknown; a `scan_gemini` cannot
reverse it and must enumerate.

**Documented session contents** 📖: *"Your prompts and the model's responses ·
All tool executions · **Token usage statistics (input, output, cached, etc.)** ·
Assistant thoughts and reasoning summaries"*.

### 🔬 The store, observed (P13)

`<project_hash>` in the **path** is not a hash at all — it is the project
directory's **basename**, with the real path in a sibling file:

```
~/.gemini/tmp/fincal/.project_root            -> "c:\dev\fincal"
~/.gemini/tmp/fincal/chats/session-2026-07-21T00-56-ralphy-p.jsonl
```

The filename is `session-<ISO-timestamp>-<first 8 chars of session id>.jsonl`.
A real 64-hex `projectHash` exists, but **inside** the file's header record.
For `scan_gemini` (ADR-0033) this is friendly: enumerate the directories under
`tmp/`, read `.project_root` to map back to a repo — no hash to reverse.

**Format: JSONL, as an append-only event log with `$set` mutation records** —
a small event-sourced store, not a document:

```jsonl
{"sessionId":"ralphy-probe-p1p2p3p4p6","projectHash":"3c489ab0…","startTime":"…","lastUpdated":"…","kind":"main"}
{"$set":{"messages":[…]}}
{"id":"…","timestamp":"…","type":"user","content":[{"text":"…"}]}
{"$set":{"lastUpdated":"…"}}
{"id":"…","type":"gemini","content":"OK","thoughts":[…],
 "tokens":{"input":20637,"output":30,"cached":0,"thoughts":257,"tool":0,"total":20924},
 "model":"gemini-3.1-pro-preview-customtools"}
```

- **Usage is PER-TURN and per-model**, carried on each `type: "gemini"` record.
  It is **incremental — sum it, do not keep-last** (the Kimi convention, not the
  Codex one). Getting this backwards multiplies the bill (ADR-0040 C6).
- `thoughts` tokens **are** broken out here, alongside `cached` and `tool`.
- `kind: "main"` on the header implies non-main session kinds — and 🔬 there is
  one: **a subagent invocation creates a nested session of its own.**

  ```
  chats/session-2026-07-21T01-00-ralphy-p.jsonl          # kind: "main"
  chats/ralphy-probe-policy/78d80d17-….jsonl             # kind: "subagent", + "directories"
  ```

  The nested file is keyed by the **parent's session id as a directory** and the
  subagent's own UUID as the filename. That one subagent burned **17 595
  tokens** on `gemini-3.5-flash`. A `scan_gemini` that globs `chats/*.jsonl`
  **misses subagent consumption entirely** — it must recurse. Conversely, the
  stream envelope appears to aggregate parent and subagent by model, so a naive
  "sum the store and the envelope" would double-count. ⚠ Which of the two is
  authoritative for a delegating run is untested.

### 🔬 The router tax — and why the store cannot be the usage source

Every run silently makes a **second, paid model call to route the request**.

| Run | Stream envelope total | Session file total | Missing |
|---|---|---|---|
| `Reply with exactly: OK` | 18 273 | 14 567 | 3 706 (`gemini-3.1-flash-lite`, role `utility_router`) |
| 25 KB charter probe | 32 281 | 20 924 | 11 357 (35%) |

Three consequences:

1. **`usage.rs` must parse the stream envelope, not scan the store.** A store
   scan under-reports by 20–35%.
2. **`ralphy-usage-scan` (ADR-0033) can only ever produce a lower bound** for
   interactive Gemini sessions the operator ran outside Ralphy. The ADR must say
   so plainly rather than fake a total.
3. The router tax makes the **floor cost per run high**: ~14 k tokens of system
   prompt plus ~3–11 k of routing before the task's own tokens. Combined with
   the free tier's 250 *requests*/day and two requests per prompt, a Ralphy loop
   gets roughly 125 turns/day on that tier.

### 🔬 The lever: pinning `-m` removes the router tax entirely

`-m gemini-3.1-pro-preview` produced **one model, one API request, no
`utility_router`** (14 391 tokens total). `--approval-mode plan` does the same.

So the routing call is spent **only when the model is left as `auto`**. Pinning
a model roughly **halves the request count** — which is the binding constraint on
a request-capped tier — and removes 3–11 k tokens per turn.

This inverts the usual ADR-0004 reasoning. Elsewhere, leaving the model unpinned
is the humble default; here it has a measurable per-run price, and the ADR should
say so when choosing whether `model` is `None` by default.

### Minting the session id — ✅ **the docs are wrong, in Ralphy's favour**

📖 `session-management.md` documents only `--resume`, `--list-sessions`,
`--delete-session`, and states ids are CLI-generated UUIDs.

🔬 But `gemini --help` on the installed 0.51.0 lists `--session-id` and
`--session-file`, and 🔎 `packages/cli/src/config/config.ts` confirms the
semantics:

```js
.option('session-id', { type: 'string', nargs: 1,
  description: 'Start a new session with a manually provided UUID.',
  coerce: v => { /* rejects empty; must match /^[a-zA-Z0-9-_]+$/ */ } })
```

Four facts:

1. **`--session-id` starts a NEW session with Ralphy's id** — it does not
   resume one. That is exactly the semantic Ralphy needs.
2. **The validation is `^[a-zA-Z0-9-_]+$`**, not a real UUID check. Any
   alphanumeric/dash/underscore string is accepted despite the description.
   Ralphy's existing run ids would likely pass as-is.
3. It is **mutually exclusive** with `--resume` and `--session-file` (hard argv
   error, §1).
4. `--session-file` takes *a JSON file*, which is the strongest hint yet that
   the on-disk session store is JSON.

**Consequence: the ADR-0008 D10 snapshot-diff is unnecessary for this vendor** —
lookup becomes a direct key, the Copilot precedent. P2 downgrades from
"discover" to "confirm it lands on disk under that id".

### Telemetry as a usage source 📖

The OTel schema is richer than anything on disk is documented to be —
`gemini_cli.api_response` carries `input_token_count`, `output_token_count`,
`cached_content_token_count`, `thoughts_token_count`, `tool_token_count`,
`total_token_count`, `model`, `auth_type`, `duration_ms`. Telemetry is **off by
default** and can write to a local file (`telemetry.outfile`).

This is a genuine design option for `usage.rs` — and a trap: enabling telemetry
turns on `logPrompts`, which **defaults to `true`**, shipping prompt text. If
the ADR goes this way it must set `telemetry.logPrompts: false` explicitly.

### `ralphy-usage-scan` (ADR-0033)

⚠ Cannot be specified until the store format is observed. The docs claim usage
*is* on disk, so unlike Cursor a real `scan_gemini` looks feasible.

## 7. C7 — Limits

**The weakest-documented area, and the one most likely to burn a run.**

📖 Daily caps (*"maximum requests per user per day"*):

| Auth | Tier | Cap |
|---|---|---|
| Google account | Code Assist Individual | 1,000 |
| Google account | Google AI Pro | 1,500 |
| Google account | Google AI Ultra | 2,000 |
| Gemini API key | Free | **250**, Flash only |
| Gemini API key | Paid | varies |
| Vertex AI | Express (free) | varies |
| Workspace | Code Assist Standard | 1,500 |
| Workspace | Code Assist Enterprise | 2,000 |

Plus an unspecified **per-minute** limit.

### 🔎 The finding: **quota fallback is interactive-only**

📖 The docs paint a friendly picture — on hitting the Gemini 3 Pro daily limit
*"you'll be given the option to switch to Gemini 2.5 Pro, upgrade for higher
limits, or stop. You'll also be told when your usage limit resets."*

🔎 **None of that exists headless.** `packages/core/src/fallback/handler.ts`:

```js
const handler = config.getFallbackModelHandler();
if (typeof handler !== 'function') {
  return null;
}
```

`setFallbackModelHandler()` is called in **exactly one place**:
`packages/cli/src/ui/hooks/useQuotaAndFallback.ts` — a React hook in the
interactive TUI. `nonInteractiveCli.ts` and `nonInteractiveCliAgentSession.ts`
contain **zero** references to fallback or quota.

So in headless mode:

- The handler is unset → returns `null` → **no fallback offered, the request
  fails.** `-m pro` does **not** auto-degrade to Flash on a 429 the way the
  interactive UI would.
- The only fallback that survives is the branch *above* it, `action === 'silent'`,
  driven by per-model policy in `packages/core/src/availability/`. That covers
  routine auto-routing, not quota exhaustion.

This is good news and bad news. Good: **no hang, and no invisible downgrade of
the main model.** Bad: **the run just dies**, and Ralphy must classify it.

Three open holes, all ⚠:

1. **No structured surface for the failure.** No exit code is reserved for
   quota; `quotaErrorDetection.ts` exports only `isApiError` / `isStructuredError`
   and contains **no retry-delay parser**. The likely shape is a generic API
   error → **exit `1`**, with any reset text embedded in the message string.
   But recall §3: `extractErrorCode()` passes any numeric `.code`/`.status`
   straight to `process.exit()`, so **a raw `429` exit code is reachable**.
   Which of the two happens is P14 and cannot be settled from source.
2. **No machine-readable reset hint.** The "you'll be told when your limit
   resets" text is a TUI message. If this holds, ADR-0030's synthetic
   ~30-minute cadence applies and **`Limit(None)` is the honest emission**.
3. 🔎 `FallbackIntent` is a documented union —
   `retry_always | retry_once | retry_with_credits | stop | retry_later | upgrade`
   — which suggests a richer signal exists internally than reaches a headless
   caller. Whether any of it is observable is unknown.

### 🔬 P14 attempt — the CLI absorbs transient rate limits silently

Twelve runs fired in parallel (24 API requests inside a few seconds) **all
returned exit 0**. No 429 surfaced, no warning, no `error` record.

The explanation is in the stack trace captured during P5:
`retryWithBackoff` sits between `classifyGoogleError` and the caller. **The CLI
retries internally with exponential backoff**, so a transient per-minute 429 is
invisible to the caller — it manifests only as latency.

Two consequences:

1. **A rate limit only reaches Ralphy after the CLI's own retries are
   exhausted.** Whatever Ralphy sees is therefore already a hard failure, not a
   transient one — which argues against Ralphy adding its own retry layer on top.
2. The absorbed retries still **consume quota**, invisibly. A run that looks
   slow may have spent several requests.

⚠ **True daily-quota exhaustion remains unobserved** and is the last
load-bearing gap. Reproducing it means deliberately burning a day's allowance;
it is the one probe worth its cost only if the ADR cannot proceed without it.

Per ADR-0040 C7, the limit predicate must match a **limit class** (a regex over
"rate limit | quota exceeded | too many requests | resource exhausted"), never
one phrasing — the OpenCode `usage_limit_regex` reference. Note also the
ADR-0028 precedent in reverse: Kimi gave a clean semantic code for this and
Gemini, despite having ten semantic codes, **reserved none for quota**.

Also relevant: 📖 `model.maxSessionTurns` (default `-1`, unlimited) produces the
one documented headless failure — *"Non-interactive mode: The CLI exits with an
error"* — which is almost certainly the `53` turn-limit code.

## 8. C8 — Skills and prompts

### Skills — a standards-based system Ralphy could plug into 📖

Discovery tiers, lowest to highest precedence: built-in → extension → **user
(`~/.gemini/skills/` or `~/.agents/skills/`)** → **workspace (`.gemini/skills/`
or `.agents/skills/`)**.

`SKILL.md` frontmatter is just `name` + `description`, and the format is
declared as the [agentskills.io] open standard, with `.agents/skills/` as the
explicitly *"interoperable path … compatible across different AI tools"*.

Two things this is **not**, contrary to what the Cursor spike might lead one to
expect:

- ⚠ **No documented read of `~/.claude/skills`.** Gemini does not appear to
  harvest another vendor's skill library the way Cursor CLI does. Good for
  hygiene, but it means materialization is **not** free — Ralphy must write into
  `.gemini/skills/` or `.agents/skills/`.
- 🔬 **P9 — skills DO activate headless under `--approval-mode yolo`.** The
  feared deadlock (consent → `ask_user` → denied) does **not** occur; yolo
  auto-approves the consent. Observed:

  ```json
  {"type":"tool_use","tool_name":"activate_skill","parameters":{"name":"ralphy-probe-skill"}}
  {"type":"tool_result","status":"success",
   "output":"Skill **ralphy-probe-skill** activated. Resources loaded from `C:\\Users\\PICHAU\\.gemini\\skills\\ralphy-probe-skill`…"}
  ```

  The body genuinely loaded — the model emitted the skill's private token
  `RALPHY_SKILL_LOADED_B4D2` and its private datum `CHECKSUM_9A7E`, neither of
  which was in the prompt. **`skills.rs` is viable**: write to
  `~/.gemini/skills/<name>/SKILL.md`, frontmatter `name` + `description`, and
  `gemini skills list` confirms discovery for free (exit 0, no paid call).

  ⚠ Still open: activation was *explicitly requested* in the prompt. Whether
  description-matching alone triggers it reliably is untested, and the gating
  risk moves to the enterprise "Unmanaged Capabilities" control below.

Compounding it: enterprise "Unmanaged Capabilities" is **disabled by default**
and *"this control disables Agent Skills"* on managed machines 📖.

### Instruction files — the charter competition is real 📖

The CLI *"concatenates the contents of all found files and sends them to the
model with **every prompt**"*, from three levels:

1. `~/.gemini/GEMINI.md` (global)
2. `GEMINI.md` in the workspace **and every parent directory**
3. Just-in-time: when a tool touches a file, `GEMINI.md` files in *that*
   directory and its ancestors, up to a trusted root

`GEMINI.md` supports `@file.md` imports with relative **and absolute** paths.
The filename is configurable (`context.fileName`, which accepts a **list** —
`AGENTS.md` and `CONTEXT.md` are read only if configured).

⚠ **No documented way to disable this discovery.** No `--no-memory` flag, no
`context.enabled`. For Ralphy — whose whole contract is that the charter is the
instruction set — an unownable file that prepends itself to every prompt is a
direct conflict needing an ADR stance. Note 🔬 this machine already has a
(zero-byte) `~/.gemini/GEMINI.md`.

Related repo-local vectors: `.gemini/commands/*.toml` (custom slash commands,
with `!{...}` shell injection), `.gemini/agents/*.md` (subagents **and remote
agents**), `.geminiignore`.

### Native plan mode — closer to fitting than any previous vendor 📖

`--approval-mode plan` is read-only… *except* it explicitly permits
`write_file`/`replace` for `.md` files in the plans directory. Verbatim from
the shipped `bundle\policies\plan.toml`:

```toml
[[rule]]
toolName = ["write_file", "replace"]
decision = "allow"
priority = 70
modes = ["plan"]
argsPattern = "…\\.gemini[\\\\/]+tmp[\\\\/]+[\\w-]+[\\\\/]+plans[\\\\/]+[\\w-]+\\.md\"…"
```

And the plans directory is **configurable to a repo-local path**
(`general.plan.directory`, e.g. `.gemini/plans`), with the docs showing the
matching policy rule to allow it.

So unlike every previous vendor, "the planner writes its own plan file" is
*natively expressible*. **But** two blockers:

- The path is constrained: *"user-configured paths for the plans directory are
  restricted to the project root"*, and the built-in `argsPattern` only matches
  `.md` under a `plans` directory. Ralphy writes `.ralphy/plan.md` — ⚠ whether
  that path can be permitted requires a custom policy via `--policy`.
- 📖 Plan-mode transitions are **denied in yolo mode** (`yolo.toml`, priority
  999) — so `--approval-mode plan` and `--approval-mode yolo` are mutually
  exclusive, and the planner run cannot also be fully autonomous.

Encouragingly, headless plan mode is designed for: 📖 *"the policy engine
automatically approves the `enter_plan_mode` and `exit_plan_mode` tools without
prompting"* and *"when exiting Plan Mode to execute the plan, Gemini CLI
automatically switches to YOLO mode"*.

### 🔬 P18 — plan mode runs headless, but hijacks the plan's location

`--approval-mode plan` completed cleanly (exit 0, sentinel intact). The prompt
explicitly said *"Save it to the file `.ralphy-probe/plan.md`"*, and a
`--policy` rule allowed `write_file` in plan mode at priority 200. The model
**never attempted that path**:

```json
{"type":"tool_use","tool_name":"write_file","parameters":{
  "file_path":"C:\\Users\\PICHAU\\.gemini\\tmp\\fincal\\ralphy-probe-plan\\plans/plan.md", …}}
```

It was not denied — it simply wrote to the vendor's private plans directory,
`~/.gemini/tmp/<project>/<session-id>/plans/plan.md`, because that is where plan
mode's own system prompt sends it. **Permitting the path is not enough; the
instruction loses to the vendor's plan-mode prompt.**

This is ADR-0040 C8's prediction, confirmed: *"native plan modes persist to
vendor-private stores… Rejecting it is the norm."* **Ralphy's planner must run
in normal/yolo mode with the Ralphy charter** — which P1 and P10 already showed
works, `write_file` included.

Two side observations, both cost-relevant:

- 🔬 **Plan mode pays no router tax.** The run used exactly one model
  (`gemini-3.1-pro-preview-customtools`), with no `utility_router` call — the
  docs' "Planning Phase routes to a high-reasoning Pro model" is real and it
  bypasses the router.
- 🔬 An undocumented plan-mode tool appeared: `update_topic`
  (`{strategic_intent, title, summary}`).

Overlay slots: to be decided in the ADR.

## 9. C9 — Blast radius and the product ethos

Ralphy never pushes and never opens PRs.

### The headline: autonomy is not argv-expressible

This is the same axis Cursor introduced, but here it is **formalized, documented
and quantified**. From `bundle\policies\yolo.toml` and `plan.toml` 🔬:

```
# Priority bands (tiers):
# - Default policies (TOML):   1 + priority/1000
# - Extension policies (TOML): 2 + priority/1000
# - Workspace policies (TOML): 3 + priority/1000
# - User policies (TOML):      4 + priority/1000
# - Admin policies (TOML):     5 + priority/1000
…
#   998: YOLO mode allow-all (becomes 1.998 in default tier)
```

**`--yolo` is a Default-tier rule at final priority 1.998.** Every user rule
(4.x) and every admin rule (5.x) outranks it. Therefore:

- A `deny` in `~/.gemini/policies/*.toml` — a file on the operator's machine
  that Ralphy does not own — **vetoes tools under `--yolo`**.
- An `allow` there **expands** what runs unattended. The docs ship exactly the
  dangerous example: a rule named *"Allow pr-creator to push code"* with
  `commandPrefix = "git push"`, `decision = "allow"`.
- Admin policies (`C:\ProgramData\gemini-cli\policies`,
  `/etc/gemini-cli/policies`) cannot be outranked at all.

### 🔬 P10 — `--policy` outranks `--yolo`. Autonomy is argv-recoverable.

A one-rule file passed on argv vetoed a tool while yolo was active:

```toml
[[rule]]
toolName = "run_shell_command"
decision = "deny"
priority = 100
denyMessage = "RALPHY_POLICY_DENIED_SHELL"
```

```console
$ gemini -p "…write a file, then run git status…" \
    --approval-mode yolo --skip-trust --policy .ralphy-probe/deny-shell.toml -o stream-json
```

`write_file` succeeded; the shell was refused. **This is the mitigation for the
tier problem**: Ralphy can pin the tool surface from argv rather than hoping the
operator's `~/.gemini/policies/` is empty. ⚠ Whether a `--policy` rule also
outranks an *admin*-tier deny is untested (and almost certainly it does not).

### 🔬 The third escape route: an untrusted folder silently demotes `--yolo`

During P19, before the fatal exit, stderr carried:

```
YOLO mode is enabled. All tool calls will be automatically approved.
Approval mode overridden to "default" because the current folder is not trusted.
```

**`--approval-mode yolo` is silently downgraded to `default` in an untrusted
folder.** In that probe the run then died with exit 55, so the demotion was
academic — but the demotion is announced *independently* of the fatal error, and
`GEMINI_CLI_TRUST_WORKSPACE` / `--skip-trust` are what prevent both.

So autonomy can be lost three distinct ways, none of them visible in argv:
a higher-tier **policy** rule (below), enterprise **Strict Mode**, and an
**untrusted folder**. Only the third has a loud message.

### 🔬 …but the agent tried to escape it by delegating

In the same run, immediately after the deny, the model did this:

```json
{"type":"tool_use","tool_name":"invoke_agent",
 "parameters":{"agent_name":"generalist",
   "prompt":"Please run the shell command 'git status --short' and return its output."}}
{"type":"tool_result","status":"success"}
```

It reached for the `generalist` subagent — which **inherits all tools from the
parent session** — to do what it had just been forbidden to do. The deny held,
because the subagent inherits the policy too, and the final answer honestly
reported the failure. But the attempt was unprompted and immediate.

**Consequence for the ADR:** a policy that names only `run_shell_command` is one
indirection wide. Any Ralphy policy must also constrain `invoke_agent` (the
engine supports a `subagent` rule key for exactly this), or disable subagents
outright with `{"experimental":{"enableAgents":false}}`. Note this also has a
cost dimension: the delegation attempt helped push that run to **71 714 tokens**
for what was a three-line file write.

One relief 📖: the **Workspace tier is currently non-functional** (upstream
issue #18186), so a cloned repo *cannot* currently ship policy. That is a bug
Ralphy would be depending on — the ADR should say so out loud, because it will
be fixed.

### The rest of the surface

| Capability | Evidence | Concern |
|---|---|---|
| **Enterprise Strict Mode** | 📖 "Default: enabled. If enabled, users will not be able to enter yolo mode." | On a managed machine the autonomy flag is simply unavailable. Needs a preflight. |
| **Unmanaged Capabilities off by default** | 📖 "this control disables Agent Skills" | Skills-based design breaks on managed machines. |
| **Remote agents (A2A)** | 📖 `experimental.enableAgents` — **enabled by default**; defined by `.gemini/agents/*.md` (repo-local) with an `agent_card_url` | Delegates tasks to arbitrary remote endpoints; can shell out for tokens (`!gcloud auth print-token`) and open a browser for OAuth. **Repo-local definition + on-by-default is the sharpest edge in this table.** |
| **Subagents** | 📖 `codebase_investigator`, `cli_help`, `generalist` enabled by default; independent context, `max_turns` 30 | Extra billed turns Ralphy never requested, and they **ignore `--model`**. |
| **`browser_agent`** | 📖 disabled by default; launches Chrome, persistent profile at `~/.gemini/cli-browser-profile/` | Off by default; must stay off. |
| **MCP servers** | 🔬 `gemini mcp add/list/enable/disable`; 📖 config at `.gemini/mcp.json` and user scope; admin can **inject required servers** with `trust: true` (no approval) | Admin-injected trusted MCP servers bypass approval entirely. |
| **Extensions** | 🔬 `gemini extensions install <git-url> --auto-update` | Third-party code with an auto-update channel. |
| **Auto Memory** | 📖 off by default (`experimental.autoMemory`); mines past sessions **with background model calls** on a preview Flash model | Off by default; would spend tokens invisibly if on. |
| **Checkpointing** | 📖 shadow git repo at `~/.gemini/history/<project_hash>` | Explicitly does *not* touch the project's git repo. Benign, but it is a second copy of the code on disk. |
| **Usage statistics** | 📖 `privacy.usageStatisticsEnabled` default **`true`** | **On by default.** Collects tool names, success/failure, duration, model used, approval mode. Docs state it excludes prompt/response content, arguments, and file content. Opt out with `{"privacy":{"usageStatisticsEnabled":false}}`. **Needs an explicit ADR stance** — this is the one thing that phones home unasked. |
| **OpenTelemetry** | 📖 `telemetry.enabled` default **false**, `target` default `"local"`; `logPrompts` default **true** *if* enabled | Off by default and local even when on. The `logPrompts` default is the trap if the ADR ever enables it as a usage source (§6). |
| **Sandbox** | 📖 off by default; Docker/Podman, macOS Seatbelt, gVisor, LXC, and a **Windows native** mode using `icacls` low-integrity that is **persistent on the filesystem after the session ends** | The Windows mode leaves durable ACL changes. Leave sandboxing off; if ever enabled, this is a footgun. |
| **Push / PR verbs** | 📖 none exist as CLI verbs | ✅ No native PR-opening capability — better than Cursor. Reachable only via `run_shell_command`, i.e. governed by policy. |
| **Auto-update** | ⚠ `gemini update` is documented as a command; mid-run behaviour unverified | |

### 📖 The ToS line that must be read before anything ships

`resources/tos-privacy.md`, verbatim:

> **Directly accessing the services powering Gemini CLI (for example, the Gemini
> Code Assist service) using third-party software, tools, or services (for
> example, using OpenClaw with Gemini CLI OAuth) is a violation of applicable
> terms and policies. Such actions may be grounds for suspension or termination
> of your account.**

Ralphy spawns the `gemini` binary as a subprocess rather than reusing its OAuth
token against Google's endpoints, which is *not* what that sentence describes.
But it names a competing agent-runner by name and the distinction is one clause
wide. **This belongs in front of a human before Phase 3**, and it is the only
finding in this spike that is a business risk rather than an engineering one.

## 10. C10 — Cross-platform and I/O hygiene

- **Binary resolution.** 🔬 Windows: `%APPDATA%\npm\gemini.cmd` / `.ps1` (npm
  global shims — **not** a native `.exe`); `Get-Command gemini` resolves to the
  `.ps1`. WSL: `~/.nvm/versions/node/v24.13.0/bin/gemini`.
- 🔬 **The WSL PATH trap reproduces, and worse than for Kimi.** In WSL,
  `which gemini` returns **`/mnt/c/Users/PICHAU/AppData/Roaming/npm/gemini`** —
  the *Windows* shim, inherited through PATH interop — which then fails:

  ```console
  $ wsl -e bash -lc 'gemini --version'
  /mnt/c/Users/PICHAU/AppData/Roaming/npm/gemini: 15: exec: node: not found
  # exit 127
  ```

  So a positive `which` result points at a **broken** binary, and the working
  Linux install is off PATH for non-login shells. `resolve_program` must
  explicitly reject `/mnt/c/...` paths in WSL, not merely search PATH. Even the
  nvm binary needs `node` on PATH — invoking it with a bare `PATH` yields
  `/usr/bin/env: 'node': No such file or directory`, exit 127.
- ✅ **Windows spawn shape: already solved by existing infrastructure (P12).**
  The install is `.cmd`/`.ps1` shims over a Node bundle, with an extensionless
  shim beside them — which is precisely the case
  `ralphy-proc-util::resolve_program` was written for
  ([lib.rs:113-129](../../crates/ralphy-proc-util/src/lib.rs#L113-L129)), and its
  tests already fixture exactly this layout with `opencode.cmd`. **No new work
  in Tier 1 for Windows.**

- ⚠ **But `locate_program` has two real gaps against this vendor on WSL**, both
  observed:

  1. 🔬 It searches `PATH` first, and on WSL `PATH` contains the **Windows** npm
     directory through interop. `which gemini` returns
     `/mnt/c/Users/PICHAU/AppData/Roaming/npm/gemini` — a shim that then dies
     with `exec: node: not found`, **exit 127**. A positive resolution points at
     a broken binary, and detection and execution agree only in being wrong
     together.
  2. 🔬 The `~/.local/bin` fallback does not help: the working Linux install is
     under **`~/.nvm/versions/node/<version>/bin/gemini`**, which no current
     search path covers. (Even invoked directly it needs `node` on `PATH`,
     otherwise `/usr/bin/env: 'node': No such file or directory`, exit 127.)

  This is the [WSL vendor-CLI precedent] again (Kimi's `~/.kimi-code/bin`), but
  sharper: for Kimi `which` was merely *negative*, here it is **falsely
  positive**. The ADR needs a stance — most likely: on Linux, reject `PATH`
  entries under `/mnt/c/`, and add an nvm-aware probe.

[WSL vendor-CLI precedent]: ./kimi-cli-adapter-spike.md
- 🔬 **Encoding: clean (P22).** A UTF-8 payload piped through `cmd` redirection
  and returned via `stream-json` round-tripped **byte-exact** — Portuguese
  accents, CJK, and 4-byte astral-plane emoji alike:

  ```
  RALPHY_ENC_HEAD / Acentuação: ção é í õ ü / CJK: 日本語テスト / Emoji: 📂🚀 / RALPHY_ENC_TAIL
  ```

  No cp1252 damage. **The Kimi hazard (ADR-0028 D5) does not reproduce** — the
  CLI detects non-TTY and never engages the Ink renderer.

  The same capture also confirms the stdin contract verbatim: the `message/user`
  record read `…RALPHY_ENC_TAIL\n\n\nEcho back the exact text…`, i.e.
  literally `<stdin>` + `\n\n` + `<-p text>`.
- **Version parity**: 🔬 `0.51.0` on both platforms. No drift today.
- ✅ 🔬 **`ACCEPTS_IMAGES` (ADR-0025) = `true` — but through a different channel
  than any previous vendor (P24).** There is no attachment *flag*. The delivery
  path is the **`@<path>` syntax inside the prompt text**:

  ```console
  $ gemini -p "@.ralphy-probe/red.png What single colour fills this image? One word."
  → "Red"
  ```

  The model genuinely saw a 64×64 solid-red PNG. No `tool_use` record appeared,
  so the CLI resolved `@path` into an inline image part *before* the request —
  it is not a `read_file` round trip.

  Consequence for `command.rs`: unlike Copilot's `--attachment <path>` per image
  (ADR-0041 D12), Ralphy must **interpolate paths into the prompt string**.
  Attachment delivery is therefore coupled to prompt construction, not argv.

- 🔬 **`@` is live syntax in the prompt — but it fails safe (P25).** Since issue
  bodies routinely contain `@mentions`, this was worth pinning. A prompt reading
  *"Thanks @paulocorcino and @octocat … see @nonexistent-file.md … foo@bar.com"*
  passed through **completely unchanged** and the run succeeded: the CLI
  resolves `@` only when the path **exists**, and silently leaves the rest as
  literal text. Email addresses are unaffected.

  ⚠ The residual hazard is narrow but real: an issue body containing `@README.md`
  or `@src/` — a path that *does* exist in the target repo — would silently
  inject that file into the prompt. Context bloat and a minor injection vector;
  worth an explicit note in the ADR rather than a mitigation.

---

## A. Appendix — the full command surface

🔬 Captured from `gemini --help` on Windows `0.51.0`.

### ⚠ The docs and the binary disagree — trust `--help`

| Flag | In `--help` 🔬 | In `cli-reference.md` 📖 |
|---|---|---|
| `--session-id` | ✅ | ❌ **absent** |
| `--session-file` | ✅ | ❌ absent |
| `--policy` / `--admin-policy` | ✅ | ❌ absent |
| `--acp` | ✅ | only `--experimental-acp` |
| `--raw-output` / `--accept-raw-output-risk` | ✅ | ❌ absent |
| `--skip-trust` | ✅ | ✅ |
| `--experimental-zed-integration` | ❌ **absent** | ✅ |
| `--yolo` deprecation notice | ❌ not marked | ✅ marked deprecated |
| exit code `41` | 🔬 observed | ❌ absent from `headless.md` |
| exit codes `44/54/55/130` | 🔎 in source | ❌ absent |
| **stdin ordering** | says *"appended"* | says *"appended"* — 🔎 **source prepends** |
| **model ids** | — | 📖 a full generation stale (2.5-centric; binary ships 3.x) |
| `GOOGLE_GENAI_USE_GCA` / `USE_VERTEXAI` | in the error text only | ❌ absent from the env-var list, yet priority 1 and 2 |
| quota fallback headless | — | 📖 describes an interactive prompt as if universal |

**The shipped documentation is stale relative to the shipped binary, and wrong
in at least one load-bearing place (stdin ordering).** Every adapter decision
must cite `--help`, the source, or an observed run — never the docs alone.
This is why §B keeps live probes queued even where source has already answered:
source is what the binary *should* do; only a run shows what it does.

### Global options 🔬

| Flag | Meaning |
|---|---|
| `-d, --debug` | debug mode, verbose logging |
| `-m, --model <id>` | model (default `auto`) |
| `-p, --prompt <text>` | **headless mode**; *"Appended to input on stdin (if any)"* |
| `-i, --prompt-interactive <text>` | run prompt then stay interactive |
| `--skip-trust` | trust the workspace for this session |
| `-w, --worktree [name]` | start in a new git worktree |
| `-s, --sandbox` | run sandboxed |
| `-y, --yolo` | auto-approve all actions |
| `--approval-mode <m>` | `default` \| `auto_edit` \| `yolo` \| `plan` |
| `--policy <paths>` | additional policy files/dirs (repeatable) |
| `--admin-policy <paths>` | additional **admin** policy files/dirs (repeatable) |
| `--acp` / `--experimental-acp` | ACP mode (deprecated spelling) |
| `--allowed-mcp-server-names <list>` | MCP allowlist |
| `--allowed-tools <list>` | **deprecated**, use the policy engine |
| `-e, --extensions <list>` | restrict to these extensions |
| `-l, --list-extensions` | list and exit |
| `-r, --resume <id\|index\|latest>` | resume a session |
| `--session-file <path>` | load a session from a JSON file |
| `--session-id <uuid>` | **start a new session with a caller-provided UUID** |
| `--list-sessions` | list sessions for this project and exit |
| `--delete-session <index>` | delete a session |
| `--include-directories <list>` | extra workspace roots |
| `--screen-reader` | accessibility mode |
| `-o, --output-format <f>` | `text` \| `json` \| `stream-json` |
| `--raw-output` | disable output sanitization (**security risk**) |
| `--accept-raw-output-risk` | suppress that warning |
| `-v, --version` · `-h, --help` | |

### Subcommands 🔬

| Command | Subcommands |
|---|---|
| `gemini [query..]` | default — launch the agent |
| `gemini mcp` | `add <name> <cmdOrUrl> [args…]` · `remove` · `list` · `enable` · `disable` |
| `gemini extensions` | `install` · `uninstall` · `list` · `update` · `enable` · `disable` · `link` · `new` · `validate` · `config` |
| `gemini skills` | `list [--all]` · `enable` · `disable` · `install <src>` · `link <path>` · `uninstall` |
| `gemini hooks` | `migrate` — *"Migrate hooks from Claude Code to Gemini CLI"* |
| `gemini gemma` | `setup` · `start` · `stop` · `status` · `logs` (local LiteRT-LM routing) |
| `gemini update` | 📖 self-update (documented; not in the `--help` command list) |

### Hook events 📖

`BeforeTool` · `AfterTool` · `BeforeAgent` · **`AfterAgent`** · `BeforeModel` ·
`AfterModel` · `BeforeToolSelection` · `SessionStart` · `SessionEnd` ·
`Notification` · `PreCompress`.

Configured in `settings.json` under `hooks`. Communication: JSON on stdin, JSON
on stdout, logs on stderr. Hook exit codes: `0` = success (stdout parsed as
JSON), `2` = block (stderr becomes the reason), other = non-fatal warning.

### Approval modes and the policy vocabulary 📖

Modes ordered by permissiveness: `plan` < `default` < `autoEdit` < `yolo`.
Rule keys: `toolName` (wildcards `*`, `mcp_*`), `subagent`, `mcpName`,
`toolAnnotations`, `argsPattern`, `commandPrefix`, `commandRegex`, `decision`
(`allow`/`deny`/`ask_user`), `priority` (0–999), `denyMessage`, `modes`,
**`interactive`** (`true` = interactive only, `false` = headless only),
`allowRedirection`.

---

## B. Probe log

Live runs: **2026-07-20**, `C:\Dev\FinCal`, branch `afk/run-20260720-143515`,
auth `gemini-api-key`, CLI `0.51.0` on Windows.

| # | Probe | C | Status |
|---|---|---|---|
| P0 | logged-out signature, both platforms | C5 | ✅ **🔒 final** — exit `41`, structured under `-o json` only |
| P1 | **stdin channel** — 25 404-byte charter with head/tail markers | C1 | ✅ **pass** — arrived whole, both markers echoed, 30 073 input tokens; `-p` text confirmed to land **after** stdin |
| P2 | **`--session-id` adoption** | C6 | ✅ **pass** — `init.session_id` and the on-disk header both read `ralphy-probe-p1p2p3p4p6`; a non-UUID string was accepted |
| P3 | `stream-json` shape | C2 | ✅ **pass** — 5 discriminators observed, `result` envelope with per-model usage; **deltas split mid-word**, must be joined before matching |
| P4 | sentinel as the last line | C3 | ✅ **pass** — `RALPHY_DONE_5E1D` survived, though only after joining deltas |
| P5 | invalid `-m` | C4 | ✅ **pass** — exit 1, `result.status:"error"` but `type:"unknown"`; real diagnosis only on stderr; **0 tokens billed** |
| P6 | **hooks headless** | C3 | ✅ **pass, high value** — all 4 events fired from **user** scope; `AfterAgent.prompt_response` is the finished answer. **Workspace scope silently does not fire.** |
| P7 | progress fields vs HEAD diff | C2 | ✅ **resolved by absence** — `stream-json` reports no file stats at all (only `json` mode does) |
| P8 | mid-run error under `stream-json` | C2 | 🟡 **partial** — a mid-run error *does* emit a `result` envelope (P5); a pre-flight one does not. The documented `error` record type was never seen. |
| P9 | **skill activation headless** | C8 | ✅ **pass** — `activate_skill` auto-approved under yolo; skill body verifiably loaded |
| P10 | **`--policy` vs `--yolo`** | C9 | ✅ **pass** — argv policy vetoed the tool under yolo; **and the model tried to escape via `invoke_agent`** |
| P11 | full exit-code enumeration | C3 | ✅ **done** — 10 codes from source, 6 undocumented (§3) |
| P13 | session store format and granularity | C6 | ✅ **pass** — JSONL event log, per-turn per-model tokens, **but under-reports by 20–35% (router tax)** |
| P14 | **quota exhaustion surface** | C7 | 🟡 **partial** — 12 parallel runs (24 requests) never tripped it; `retryWithBackoff` **absorbs transient 429s silently**. True daily exhaustion still unobserved |
| P15 | **`GEMINI_CLI_HOME` isolation** | C5/C9 | ✅ **pass, highest design value** — relocation + a 1-line `settings.json` works; credential comes from the OS store |
| P16 | repo-local hook fingerprint warnings headless | C3/C9 | 🟡 **moot** — workspace hooks do not load at all (P6) |
| P18 | **native plan mode headless** | C8 | ✅ **ran, and disqualified itself** — writes to the vendor's private plans dir regardless of instruction *and* of an allowing policy |
| P19 | untrusted workspace → exit 55 | C1/C3 | ✅ **pass** — dies pre-flight with a fully actionable message; **also silently demotes `--yolo` to `default`** |
| P20 | `maxSessionTurns` → exit 53 | C3/C7 | ✅ **pass** — `error.type:"FatalTurnLimitedError"` on **stderr**, printed twice |
| P21 | entitlement: explicit `-m gemini-3.1-pro-preview` on this key | C4 | ✅ **served** — contradicts the documented Flash-only tier; **and pinning `-m` removes the router tax** |
| P22 | UTF-8 round trip through stdin | C10 | ✅ **pass** — byte-exact incl. CJK and astral emoji; confirms the `\n\n` join verbatim |
| P12 | Windows spawn shape vs `gemini.cmd` | C10 | ✅ **already solved** by `ralphy-proc-util::resolve_program`; **but two WSL gaps found** (§10) |
| P24 | `ACCEPTS_IMAGES` — headless vision via `@path` | C10 | ✅ **pass** — `true`, delivered in the prompt string, not argv |
| P25 | `@mention` safety in issue-body text | C10 | ✅ **fails safe** — unresolvable `@tokens` pass through literally |
| P17 | does description-matching alone activate a skill, without naming it? | C8 | ⬜ open |
| P23 | `GEMINI_CLI_HOME` isolation under **OAuth** auth (credential is file-based there) | C5 | ⬜ open |

### Reproduction

Probe artifacts were written to `C:\Dev\FinCal\.ralphy-probe\` and
`C:\Dev\FinCal\.gemini\`, and a probe skill to
`~/.gemini/skills/ralphy-probe-skill/`. **All were removed when the spike
closed**; the operator's `~/.gemini/settings.json` was backed up before the
user-scope hook test and restored verbatim afterwards.

The P1 command, verbatim:

```console
cd C:\Dev\FinCal
gemini -p "<override text>" --session-id ralphy-probe-p1p2p3p4p6 \
  --approval-mode yolo --skip-trust -o stream-json \
  < .ralphy-probe\payload.txt > .ralphy-probe\p1.jsonl 2> .ralphy-probe\p1.err
```

### 🔬 stderr is never empty — the capture must tolerate it

Every single run, including successful ones, emitted this preamble on stderr:

```
Warning: 256-color support not detected. Using a terminal with at least 256-color support is recommended…
YOLO mode is enabled. All tool calls will be automatically approved.
YOLO mode is enabled. All tool calls will be automatically approved.
Ripgrep is not available. Falling back to GrepTool.
```

Note the YOLO line is printed **twice**. A `stderr.is_empty()` health check
would report every run as degraded.

### Reproduction

Raw captures live in `%TEMP%\gemini-probe\` for this session only. Live runs
will use `C:\Dev\FinCal` on branch `afk/run-20260720-143515`; the probe
directory is disposable and must be removed from FinCal when the spike closes.

---

## C. ADR-0040's wiring inventory has drifted — measured

ADR-0040 predicts its own drift (*"This ADR is expected to drift, because the
wiring inventory tracks live code"*). Before Phase 3 is estimated, here is the
drift, measured against **Copilot** — the newest vendor, and therefore the
empirical inventory.

Method: every file mentioning `copilot` outside its own crate, excluding docs
and `.ralphy/knowledge`. **24 files.**

### Still accurate ✅

- **The three agent enums exist and do not share a definition**, exactly as
  warned: [`cli.rs:295`](../../crates/ralphy-cli/src/cli.rs#L295) `CliAgent` ·
  [`init/gate.rs:9`](../../crates/ralphy-cli/src/init/gate.rs#L9) `Agent` ·
  [`daemon/src/session.rs:42`](../../crates/ralphy-daemon/src/session.rs#L42) `Agent`.
- **`ALL` really is a hardcoded-length array** —
  [`gate.rs:25`](../../crates/ralphy-cli/src/init/gate.rs#L25):
  `pub const ALL: [Agent; 5]`. Gemini makes it `6`.
- Tier 2 (`assets/prompts/plan/overlay.<vendor>.md`), Tier 4's usage-scan module
  + `pub mod`/`pub use`, the daemon quartet, and the `app.js` trio all check out.

### Drifted ⚠

- **Tier 1 is understated by ~2.8×.** The ADR says *"~1 300 LOC"* over **7**
  files. `ralphy-agent-copilot` is **3 631 LOC** over **11**. The four files the
  ADR does not list are `settings.rs`, `catalog.rs`, `effort.rs`, `guards.rs`.
  `settings.rs` is not Copilot-specific exotica — a per-vendor settings struct is
  now the pattern, and it pairs with the missing Tier 3 site below.

### Missing from the ADR entirely ❌

- **[`crates/ralphy-cli/src/config.rs`](../../crates/ralphy-cli/src/config.rs)** —
  the largest omission. A vendor lands here in **~10 distinct places**: the
  `CopilotSettings` import, the `KEYS` array, a `with_<vendor>` helper, and arms
  in `set`, `unset`, the human `print`, and the JSON emitter — plus its own
  round-trip tests. This is where `gemini.plan_model` / `gemini.exec_model` would
  live, and per §4 it is also where the **`-m` pinning decision** (which removes
  the router tax) becomes operator-configurable.
- **[`crates/ralphy-cli/src/run.rs`](../../crates/ralphy-cli/src/run.rs)** — the
  ADR lists only `run/wiring.rs::build_agent`, but `run.rs` itself loads the
  vendor's settings section and threads the resolved values through
  (lines ~247, 377–407).

### Consequence for estimating Phase 3

**Roughly 24 files outside the adapter crate, plus an 11-file crate.** ADR-0040's
own warning holds and then some: anyone estimating "just write the adapter" is
estimating well under half the work.

**ADR-0040 was amended accordingly** (its Amendment 2), adding the two missing
Tier 3 sites and correcting Tier 1's size and file list.

[agentskills.io]: https://agentskills.io
