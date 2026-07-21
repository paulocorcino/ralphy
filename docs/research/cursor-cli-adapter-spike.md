# Cursor CLI — adapter spike

Evidence for a prospective `ralphy-agent-cursor`, gathered against **Cursor
Agent CLI** on **both** target platforms:

| Platform | Binary | Version |
|---|---|---|
| Windows 11 Pro 26200 | `%LOCALAPPDATA%\cursor-agent\agent.cmd` | `2026.07.16-899851b` |
| WSL (Ubuntu) | `~/.local/bin/cursor-agent` | `2026.07.17-3e2a980` |

Lab repo: **`C:\Dev\FinCal`** (branch `afk/run-20260720-143515`), plus two
disposable control repos under `%TEMP%\cursorlab-{a,b}`. All lab mutations were
reverted; `git status` in FinCal matches its pre-spike snapshot exactly.

This document answers the C-questions of
[ADR-0040](../adr/0040-agent-adapter-onboarding-contract.md). It records
**observations**, not decisions; decisions belong in the Cursor adapter ADR.
Every claim cites a command that was run and its output. Where a capability is
asserted only by `--help` or by vendor documentation and was not exercised, it
is marked **⚠ unverified**.

Session date: 2026-07-20. Operator account: `paulo@corcino.com.br` (GitHub
auth), **`subscriptionTier: "Free"`** (machine-confirmed). C4's multi-tier
caveat applies in full: nothing here was reproduced on a paid tier.

**Status: Phase 1 complete except C7 (limits).** Sections marked 🔒 were
captured in the logged-out window and cannot be reproduced without a logout.

---

## 0. Executive summary

Cursor CLI has the **best headless ergonomics** and the **widest blast radius**
of any vendor Ralphy has evaluated. Both extremes are load-bearing.

Five things it does better than any existing vendor:

1. **A free, machine-readable auth answer.** `status --format json` →
   `{"isAuthenticated": false}`, exit 0, no paid call (§5).
2. **stdin takes a full charter.** 26 372 bytes piped with no prompt argument
   arrived intact — head and tail markers both echoed (§1, P1). Officially
   documented: print mode is inferred from *piped stdin* alone.
3. **A terminal envelope carrying usage**, with cache-read and cache-write
   already separated — ADR-0008 D2 satisfied natively (§2).
4. **Ralphy can mint the session id.** `agent create-chat` prints a UUID;
   `--resume <that uuid>` adopts it — verified end to end (§6, P13). The
   ADR-0008 D10 snapshot-diff is unnecessary.
5. **Structured tool-call records with real diffs**: `editToolCall` reports
   `linesAdded`, `linesRemoved` and a `diffString` per edit (§2).

And five findings that constrain or block:

1. **A run uploads the repository to Cursor's servers by default** — 476
   merkle-sync lines on the first FinCal run. **`.cursorindexingignore`
   suppresses it** (0 uploads in a controlled A/B on fresh repos) while leaving
   the edit tool working. `.cursorignore` also suppresses it but **breaks the
   edit tool** — and the agent routed around it via the shell (§9, P11).
2. **The CLI harvests skills from `~/.claude/skills` and `.claude/skills`** —
   documented behaviour, not a bug — and injected **78 skills** into one
   request. There is no CLI-side way to restrict the roots (§8, P12).
3. **`--model` mutates persistent operator state, and a *failed* run keeps the
   mutation.** One rejected `--model` probe wrote four keys into
   `cli-config.json` and poisoned every later run that passed no `--model`
   (§4). This is a new hazard class: **argv is not the only input, and argv
   writes back**.
4. **No local token accounting.** Tokens exist only in the live stream's
   envelope. `ralphy-usage-scan` (ADR-0033) has no source for interactive
   sessions (§6).
5. **`--list-models` lists 170 ids the account cannot use.** On Free every named
   model is rejected with `ActionRequiredError: Named models unavailable`. The
   listing is a catalogue, not an entitlement — the Copilot trap, repeated (§4).

The `stop` hook — which would have given deterministic completion — **does not
fire in the CLI**. Only `beforeShellExecution` did (§3, P15).

---

## 1. C1 — Invocation and the headless contract

| Question | Finding |
|---|---|
| Headless one-shot | `-p, --print` — *"Print responses to console (for scripts or non-interactive use). Has access to all tools, including write and shell."* |
| **Prompt channel** | ✅ **stdin, verified (P1).** `agent -p --output-format stream-json --force < payload.txt`, **no prompt argument**: 26 372 bytes arrived whole — reply echoed both `RALPHY_HEAD_7F3A` (first line) and `RALPHY_TAIL_9C2B` (last line); envelope recorded `inputTokens: 19264`. Cursor's docs confirm print mode is inferred from *"non-TTY stdout or piped stdin"*. **This is the channel the adapter must use.** |
| Argv ceiling | ⏸ not probed. With stdin proven there is no reason to push a ~26 KB charter at the ~32 KB Windows argv ceiling. |
| Full autonomy | `-f, --force` / `--yolo`, *"Force allow commands unless explicitly denied"*. Stored as `isRunEverything: true`. The *"unless explicitly denied"* is real: `cli-config.json` `permissions.deny` still vetoes (§9). |
| Middle ground | `--auto-review` — a **server-side classifier** decides which tool calls auto-run and prompts for the rest. Prompting is fatal headless; unusable. |
| Workspace trust | `--trust`, *"only works with --print/headless mode"*. Never needed across 7 runs in untrusted dirs; the flag implies a hang path that did not materialise. ⚠ trigger conditions unverified. |
| Working directory | Honoured — `system/init` echoed `"cwd":"C:\\Dev\\FinCal\\.cursor-probe"` with no flag passed. `--workspace <path-or-name>` and `--add-dir <path>` exist; `--workspace` also accepts a *saved workspace name*, an ambiguity worth pinning. |
| Isolated worktrees | `-w, --worktree [name]` → `~/.cursor/worktrees/<repo>/<name>`, `--worktree-base`, `--skip-worktree-setup`. Ralphy owns its branches; must stay off (§9). |
| Elapsed | P1 35.6 s, run C 51 s, plan-mode run 19 s. `duration_ms` in the envelope tracks API time only. |
| PTY required for billing? | No evidence it is. Headless billed normally; the Claude particularity (ADR-0002) does not appear to apply. ⚠ unverified against a bill. |

## 2. C2 — The output stream

`--output-format` = `text | json | stream-json`. `--stream-partial-output` adds
text deltas.

### Discriminators observed (P1 and run C)

| `type/subtype` | Shape |
|---|---|
| `system/init` | once, first — `apiKeySource`, `cwd`, `session_id`, `model`, `permissionMode` |
| `user/` | the echoed prompt |
| `thinking/delta` … `thinking/completed` | 34 and 48 deltas in the two runs |
| `assistant/` | one per model turn, `message.content[].text`, carries `model_call_id` |
| `tool_call/started` · `tool_call/completed` | one pair per tool call |
| `result/success` | the terminal envelope |

```json
{"type":"result","subtype":"success","duration_ms":25253,"duration_api_ms":25253,"is_error":false,"result":"…","session_id":"61683475-…","request_id":"887d3279-…","usage":{"inputTokens":19264,"outputTokens":1303,"cacheReadTokens":5248,"cacheWriteTokens":0}}
```

- **Terminal envelope: yes** — `subtype`, `is_error`, `duration_ms`,
  `session_id`, `request_id`, and `usage` with cache read/write **separated**
  (ADR-0008 D2 satisfied without folding). Cursor's docs add the failure
  contract: on error *"the stream may end early without a terminal event"* and
  the process exits non-zero, with the message on stderr. **Absence of the
  envelope is itself the failure signal.**
- **Final assistant message** is duplicated verbatim into `result.result` — the
  "last assistant record with no tool requests" heuristic is unnecessary.
- **`session_id` on every record.**
- **No `model` field anywhere in the stream.** `system/init` reports the
  *requested* model (`"Auto"`), never the resolved one (§4).
- **Progress fields exist — but only for the edit tool.** `editToolCall.result.success`
  carries `linesAdded`, `linesRemoved`, `diffString`, `afterFullFileContent`
  and the absolute `path`. `shellToolCall.result.success` carries
  `exitCode`, `stdout`, `stderr`, `executionTime` and **no file-change data at
  all**. ADR-0040's warning holds exactly: work done through the shell reports
  zero progress. In run C the agent wrote one file per channel; the stream
  accounted for one.
- **`hookAdditionalContexts`** appears on every `tool_call` record — the hook
  system injects per-call context (§3).
- 🔒 **The auth error ignores `--output-format`** — with `json`, `stream-json`
  and `text` alike, stdout was empty and prose went to stderr (§5).

### The TUI trap

`agent ls` under piped stdio **crashes and still exits 0**:

```
ERROR Raw mode is not supported on the current process.stdin, which Ink uses
      as input stream by default.
```

The renderer is Ink. The interactive subcommands are unusable headless, and
**exit code 0 is not proof of success** for them.

## 3. C3 — Completion and the sentinel

- **Sentinel: ✅ verified (P4).** `RALPHY_DONE_5E1D` requested as the final line
  survived to both the `assistant` record and `result.result`, in three separate
  runs including the WSL one.
- **Exit codes**: `0` success, `1` for auth failure, unknown model, entitlement
  refusal. **No semantic code** (no Kimi-style `75 = RETRYABLE`), and Cursor's
  docs enumerate none. Combined with `agent ls` returning 0 on a crash, exit
  code alone is weak — prefer `result.is_error` plus envelope presence.
- **Hooks: the `stop` hook does not fire in the CLI (P15).** Cursor documents a
  full hook system — `sessionStart/End`, `preToolUse/postToolUse`,
  `beforeShellExecution/afterShellExecution`, `afterFileEdit`, `stop`
  (*"called when the agent loop ends"*, with `status` and an optional
  `followup_message`) — configured in `.cursor/hooks.json` (project) or
  `~/.cursor/hooks.json` (user), with Enterprise/Team precedence above both.
  A project `hooks.json` registering `stop`, `beforeShellExecution` and
  `afterFileEdit` was placed in the lab repo and a run performed both a shell
  call and a file edit. **Only `beforeShellExecution` fired — twice.**
  `stop` and `afterFileEdit` produced nothing. This corroborates the
  outstanding community report that the CLI emits only the shell events, and it
  **closes the door on deterministic completion**: `DONE_SENTINEL` plus the
  envelope remain the mechanism.
- `CompletionSignals` fill: `is_error`, `subtype`, envelope presence, and the
  final text. Ordering delegates to `classify` (ADR-0023).

## 4. C4 — Models

Multi-model. Two enumeration surfaces — `--list-models` and the `models`
subcommand — produce identical output, exit 0, no paid call. **170 ids** on a
**Free** account.

- **Reasoning effort is inside the id, not orthogonal.** The suffix grammar is
  `<family>[-thinking]-<none|low|medium|high|xhigh|max>[-fast]`.
  `claude-opus-4-8` alone yields 16 ids. Families: `gpt-5.6-{sol,terra,luna}`,
  `gpt-5.{1,2,4,5}`, `gpt-5.3-codex`, `gpt-5.4-{mini,nano}`,
  `claude-{opus-4-8,opus-4-7,sonnet-5,fable-5,4.6-*,4.5-*,4-sonnet}`,
  `composer-2.5`, `cursor-grok-4.5`, `gemini-3{,.1-pro,.5-flash}`,
  `kimi-k2.7-code`, `glm-5.2`. **This collides with ADR-0004's amended tier
  routing** (sol/terra/luna at fixed medium effort): here the effort *is* part
  of the id string.
- **A bracket-override syntax** — `claude-opus-4-8[context=1m,effort=high,fast=false]`
  — means the id on the command line need not equal the id in the store.
  ⚠ grammar unverified.
- **Policy markers in display names**: every `claude-fable-5-*` is labelled
  `(NO ZDR)`. A data-governance signal a price table cannot express.
- **Auto-routing is real and undisclosed in the stream.** P1 ran with no
  `--model`; `system/init` said `"model":"Auto"`; the store blob recorded
  `"providerOptions":{"cursor":{"modelName":"cursor-grok-4.5-high"}}`. The same
  session's system prompt says *"powered by Composer"* — **the system-prompt
  string is not a model indicator**.
- **Tier is machine-readable**: `about --format json` → `"subscriptionTier": "Free"`
  (`null` logged out). Cheapest entitlement probe of any vendor.

### Entitlement: the listing is a catalogue, not a permission (P6)

| `--model` | Cost | stderr |
|---|---|---|
| `definitely-not-a-real-model` | free, 6 s, **no paid call** | `Cannot use this model: definitely-not-a-real-model. Available models: auto, gpt-5.3-codex-low, …` — **the full 170-id list inline** |
| `claude-opus-4-8-thinking-max` | 15 s | `ActionRequiredError: Named models unavailable Free plans can only use Auto. Switch to Auto or upgrade plans to continue.` |
| `gpt-5.6-sol-max` | 6 s | *(identical `ActionRequiredError`)* |

All exit 1. Consequences:

1. **The invalid-model rejection is the cheapest enumeration** — it prints the
   catalogue for free, before any paid call, and doubles as the actionable stop
   ADR-0040 asks for.
2. **On Free, every named model fails; only `auto` runs.** Model resolution must
   be `Option<String>`, omitted from argv when `None`, never a hardcoded
   default — the Copilot precedent (ADR-0041), reproduced exactly.
3. **`ActionRequiredError` is a distinct error class** reaching stderr as prose
   with exit 1. Strong candidate for how quota also surfaces (§7); match the
   class, not the phrase.

### 🔴 `--model` writes back to the operator's config — and failure does not roll it back

After the two rejected probes above, `cli-config.json` contained:

```json
"model": {"modelId":"gpt-5.6-sol","displayName":"GPT-5.6 Sol 272K Max","maxMode":false},
"selectedModel": {"modelId":"gpt-5.6-sol","parameters":[]},
"modelSelectionHistory": ["gpt-5.6-sol","claude-opus-4-8","default"],
"modelParameters": {"default":[],"claude-opus-4-8":[],"gpt-5.6-sol":[]},
"hasChangedDefaultModel": true
```

**Every subsequent run that passed no `--model` then failed** with the same
`ActionRequiredError`, because the persisted default was a model the plan
cannot use. `about --format json` confirmed it: `"model": "GPT-5.6 Sol 272K Max"`.
Deleting the keys was not enough — the next failing run rewrote them. The state
only cleared once the keys were purged *and* the following run passed
`--model auto` explicitly.

Two consequences, both new to Ralphy:

- **A rejected flag still mutates durable state.** A single mistyped model id
  bricks every later run of a *different* tool that shares the config.
- **The adapter must always pass `--model` explicitly** — including
  `--model auto` when it has no preference — because omitting it does not mean
  "default", it means "whatever the last invocation left behind".

⚠ Unverified: whether a paid tier accepts all 170, and whether `maxMode` is a
separate billing multiplier.

## 5. C5 — Authentication 🔒 **FINAL — captured in the logged-out window**

### Login command

`agent login` — *"Authenticate with Cursor. Set `NO_OPEN_BROWSER` to disable
browser opening."* `agent logout` clears stored auth. `NO_OPEN_BROWSER` is the
only documented way to stop a login attempt launching a browser on a headless
box.

### The structured answer — the best preflight surface of any vendor

```console
$ agent status --format json          # logged out, exit 0, free
{"status":"unauthenticated","isAuthenticated":false,"hasAccessToken":false,"hasRefreshToken":false,"message":"Not logged in"}

$ agent status --format json          # logged in
{"status":"authenticated","isAuthenticated":true,"hasAccessToken":true,"hasRefreshToken":true,
 "userInfo":{"email":"…","userId":188968474,"firstName":"…","lastName":"…","createdAt":"2025-04-12T06:54:34.841Z"}}

$ agent about --format json           # logged out → tier null; logged in → "Free"
{"cliVersion":"2026.07.16-899851b","model":"Auto","subscriptionTier":null,"osPlatform":"win32","osArch":"x64","userEmail":null,"terminalProgram":"unknown","shell":"cmd","lastRequestId":null}
```

This is still **behavioural** detection in the ADR-0040 sense — the CLI's own
answer, not credential-file inspection — so it does not violate house style. It
does mean ADR-0013's preflight has a *choice* between the exit-code/stderr
signature and a JSON field. `hasAccessToken`/`hasRefreshToken` reported
separately from `isAuthenticated` suggests an expired-token state distinct from
logged-out; ⚠ unverified.

### The logged-out signature

Identical byte-for-byte on Windows and WSL:

| Probe | Exit | Channel | Message |
|---|---|---|---|
| `agent status` | **0** | stdout | `Not logged in` |
| `agent about` | **0** | stdout | `User Email  Not logged in` |
| `agent --list-models` | **1** | stderr | `Error: Authentication required. Run 'agent login', pass --api-key/--auth-token, or set CURSOR_API_KEY/CURSOR_AUTH_TOKEN.` |
| `agent models` | **1** | stderr | *(same)* |
| `agent -p "hello world" --yolo` | **1** | stderr | `Error: Authentication required. Please run 'agent login' first, or set CURSOR_API_KEY environment variable.` |
| `CURSOR_API_KEY=<garbage> agent -p … --yolo` | **1** | stderr | `⚠ Warning: The provided API key is invalid.` / `The API key was loaded from the CURSOR_API_KEY environment variable.` |

Three traps:

1. **`status` and `about` exit 0 when logged out.** A preflight that shells out
   to `agent status` and checks `$?` passes while logged out. The signal is in
   the *text*, not the code.
2. **Two different "authentication required" phrasings** (listing vs execution
   path). The only common substring is `Authentication required`.
3. **An invalid key is a third case emitted as `⚠ Warning:`**, and it does
   **not** contain `Authentication required`. Its marker is
   `The provided API key is invalid`.

### Credential channels and contamination

`agent login`; `--api-key <key>`; `--auth-token` (**in the error text, absent
from `--help`**); `CURSOR_API_KEY`; `CURSOR_AUTH_TOKEN`. Cursor also documents
`CURSOR_CONFIG_DIR` and honours `XDG_CONFIG_HOME` — both relevant to isolating
the config-file hazard of §4.

`CURSOR_*` is a namespace Ralphy does not set, but the Cursor *editor* may
export it into an integrated terminal. Env hygiene must be an explicit decision.

### Where the credential lives

**`%APPDATA%\Cursor\auth.json`** — plaintext JSON, `{"accessToken","refreshToken"}`,
~415 chars each. Not an OS credential store. Login also rewrote
`~/.cursor/cli-config.json`, adding `authInfo` (email, displayName, numeric
userId, `authId: "github|user_…"`) and `privacyCache: {"ghostMode": true, "privacyMode": 1}`.

## 6. C6 — Usage and the session store

### Topology — two local stores, plus a server copy

```
~/.cursor/chats/<cwd-hash>/<session-id>/meta.json      # 137 B
~/.cursor/chats/<cwd-hash>/<session-id>/store.db       # 148 KB, SQLite
~/.cursor/projects/<cwd-slug>/agent-transcripts/<sid>/<sid>.jsonl
```

- `<cwd-hash>` is an opaque 32-hex digest of the cwd; `<cwd-slug>` is the
  readable form (`C-Dev-FinCal-cursor-probe`). **Both key on cwd**, so Ralphy's
  working directory decides where the record lands.
- `store.db`: SQLite, **two tables** — `blobs(id, data)` and `meta(key, value)`.
  A content-addressed blob graph: `meta` holds one hex-encoded JSON row
  (`{"agentId":…,"latestRootBlobId":…,"name":"New Agent","mode":"default","isRunEverything":true}`)
  pointing at a root blob; blobs hold raw request/response messages plus binary
  link nodes.
- The transcript JSONL is **3 lines**: two bare `{"role","message"}` records
  (no `type` field, unlike the stream) and `{"type":"turn_ended","status":…}`.
- Chats also live **server-side** — Cursor's docs point at `cursor.com/agents`
  to continue a CLI session from web or mobile.

### 🔴 The blocking finding: no local token accounting

**Neither store records tokens.** No `inputTokens`, no cost, no credit unit,
anywhere on disk. The only usage report is the live stream's `result.usage`,
which dies with the process.

- `usage.rs` cannot be a store scan; it must capture from the stream mid-run.
- **`ralphy-usage-scan` (ADR-0033) has no source for Cursor.** A `scan_cursor`
  can enumerate sessions and count turns, but cannot report tokens for
  *interactive* sessions run outside Ralphy. That is a capability gap to state
  plainly, not to fake.
- Cumulative-vs-incremental is moot: exactly one `result` record per run.
- **Billing is dollar-denominated credits over token pricing**, reset monthly on
  the subscription anniversary (Cursor docs). The CLI never mentions credits, so
  Ralphy's token counts and Cursor's bill are different units.

### Model attribution — present, but only in the blob graph

`providerOptions.cursor.modelName = "cursor-grok-4.5-high"` sits inside a
request blob. Recoverable at the cost of walking a content-addressed SQLite blob
graph and parsing embedded JSON.

### ✅ Minting the session id (P13)

```console
$ agent create-chat
868f1553-01ac-4335-89c6-6c1f101d6009
$ agent -p --resume 868f1553-… --force --output-format stream-json < payload.txt
{"type":"system","subtype":"init",…,"session_id":"868f1553-01ac-4335-89c6-6c1f101d6009",…}
```

**The minted id is adopted.** Ralphy can know the session id before spawning;
usage/store lookup is a primary-key read, and ADR-0008 D10's snapshot-diff is
unnecessary. (Cursor's docs never promise this — it is verified, not documented.)
`--continue` is documented as an alias for `--resume=-1`.

## 7. C7 — Limits ⬜ **the one open C-question**

Not exercised — no quota was hit across 9 runs on the free tier. What is known:

- Cursor bills **dollar-denominated credits** at per-1M-token rates, reset
  **monthly on the billing anniversary**; the Free plan has **no published
  numeric quota**. At the cap the documented behaviour is *"a notification in
  the editor"* — editor-framed, with no CLI wording.
- **No documented machine-readable limit signal and no documented exit codes.**
- The `ActionRequiredError` class (§4) is the most likely carrier: it is already
  used for a plan-entitlement refusal, which is the same family of condition.
- Community reports of `ConnectError: [resource_exhausted]` under concurrency
  exist but are unverified and undocumented.

Consequence for ADR-0030: with no reliable reset hint, `Limit(None)` and the
synthetic ~30-minute cadence is the expected answer — but the **detector** still
has to be written against a phrase nobody has captured. Closing this needs a
deliberate exhaustion run.

## 8. C8 — Skills and prompts

### Discovery roots — verified by planting markers (P12)

A marker `SKILL.md` was planted in four candidate roots and the agent was asked
to list every skill available to it:

| Root | Result |
|---|---|
| `~/.cursor/skills/` | ✅ found |
| `<repo>/.cursor/skills/` | ✅ found |
| `<repo>/.claude/skills/` | ✅ found |
| `--plugin-dir <dir>` with `<dir>/skills/<name>/SKILL.md` | ❌ **not** found |

Cursor's docs confirm and extend this: auto-discovered roots are
`.agents/skills/`, `.cursor/skills/`, `~/.agents/skills/`, `~/.cursor/skills/`,
and *"for backward compatibility, Cursor also scans `.claude/skills/`,
`.codex/skills/`, `~/.claude/skills/`, and `~/.codex/skills/`"*, walked
recursively. **Reading Claude Code's directory is deliberate.**

`--plugin-dir` missed the marker because a plugin directory requires a
`.cursor-plugin/plugin.json` manifest — the flag is for plugins, not a bare
skills root. A community report says plugin-bundled skills do not reach the CLI
registry at all. ⚠ unverified here.

**Materialization is therefore free**: `<repo>/.cursor/skills/` is a
Ralphy-writable root the CLI reads without any flag.

### The other side of the same fact

Run C reported **78 skills** available, including the operator's entire personal
Claude Code library (`grill-me`, `handoff`, `reviewer`, `caveman`, `claude-api`,
…) and every Claude Code *plugin* skill (`expo-*`, `fishjam`, `typegpu`, …).
All of it is described in the request sent to Cursor.

**There is no CLI-side way to restrict the roots.** The IDE has an "Include
third-party Plugins, Skills, and other configs" toggle; a Cursor staff member
confirmed on the forum that it **does not apply to `cursor-cli`**. This is an
open feature request.

### Vendor-pushed skills

The first authenticated run **downloaded 17 skills** into
`~/.cursor/skills-cursor/`, with `.sync-manifest.json` timestamped to the run:
`babysit canvas create-hook create-rule create-skill create-subagent loop
migrate-to-skills review review-bugbot review-security sdk shell split-to-prs
statusline update-cli-config update-cursor-settings`.

`split-to-prs` — *"Split current work into small reviewable PRs"* — is
server-pushed PR-opening guidance landing on disk unasked.
`update-cli-config` and `update-cursor-settings` mutate operator configuration.

### Native plan mode — rejected by evidence (P9)

`--mode plan` on WSL, asked to write `.ralphy/plan.md`:

> Plan mode is active, so I cannot write files or otherwise change the system.
> **Refusal message (verbatim):** *"Plan mode is active. The user indicated that
> they do not want you to execute yet — you MUST NOT make any edits, run any
> non-readonly tools (including changing configs or making commits), or
> otherwise make any changes to the system. This supersedes any other
> instructions you have received (for example, to make edits)."*

The file was not created. The mode is **hard read-only and explicitly
supersedes the charter**, so it cannot satisfy Ralphy's "the planner writes
`.ralphy/plan.md` itself" contract. ADR-0040's expected answer — reject the
native plan mode — is confirmed by experiment rather than assumed.

Overlay slots: to be decided in the ADR.

## 9. C9 — Blast radius and the product ethos

### The repository leaves the machine — and the opt-out is undocumented but real

The first `-p` run (cwd `C:\Dev\FinCal\.cursor-probe`, task explicitly forbidden
from reading files or running commands) produced
`~/.cursor/projects/C-Dev-FinCal/worker.log`, 86 KB:

```
[info] runServer socketPath=\\.\pipe\anysphere-Corcino-c--users-pichau-cursor-projects-c-dev-fincal
[debug] Starting typescript-language-server npxPath=C:\WINDOWS\System32\cmd.exe
[info] Getting tree structure for workspacePath=C:\Dev\FinCal
[info] Syncing merkle subtree path=CONTEXT.md localHash=c7a4889… remoteHash=undefined
[info] Applying change type=add relativePath=CONTEXT.md
…
```

**476 `Syncing merkle` / `Applying change` lines.** It indexed the **parent
repository**, not the cwd it was given; it ran with `ghostMode: true` and
`privacyMode: 1` set; it spawned a TypeScript language server through
`cmd.exe` and opened a named pipe; and it wrote a server-issued `repo.json` id.
The install ships `merkle-tree-napi.win32-x64-msvc.node` for exactly this.

**P11 — the controlled A/B.** Two fresh git repos, 12 TypeScript files each,
identical trivial prompt:

| Repo | Ignore file | `Applying change` | `Syncing merkle` |
|---|---|---|---|
| `cursorlab-a` | none | **15** | 16 |
| `cursorlab-b` | `.cursorindexingignore` = `*` | **0** | 1 |

`.cursorindexingignore` suppresses the upload. The indexing service still
starts (worker.log, LSP child, named pipe) but transmits nothing.

**`.cursorignore` also suppresses it — and breaks the agent.** With
`.cursorignore` = `*`, the same run reported:

> *"Write was blocked; creating the file via the shell instead."*

The edit tool was denied, and the agent **routed around the guard using the
shell tool** — precisely the leak Cursor's own docs warn about ("terminal and
MCP server tools used by Agent cannot block access to code governed by
`.cursorignore`"). With `.cursorindexingignore` alone, the edit tool worked
normally (2 `editToolCall` records, file written).

So the viable opt-out is **`.cursorindexingignore`, never `.cursorignore`** —
and it is a file Ralphy would have to place in the operator's repository.

**Documentation status: silent.** No CLI flag, env var or `cli-config.json` key
disables indexing; the ignore-files docs are IDE-scoped; merkle trees are never
mentioned in official docs; `cursor.com/data-use` describes uploading the
codebase "in small chunks to compute embeddings". **Privacy Mode is a
training/retention guarantee, not an indexing switch** — it can be enforced
team-wide from the dashboard, but nothing ties it to the CLI. A forum thread
asking exactly this has been unanswered since Aug 2025.

### The rest of the surface

| Capability | Evidence | Concern |
|---|---|---|
| **Operator config overrides argv, and argv writes back** | §4: a *failed* `--model` persisted 4 keys; `--force` is *"unless explicitly denied"* by `permissions.deny` | **New axis.** Autonomy and model are not fully expressible in argv, and Ralphy can corrupt the operator's config. |
| **Server-pushed skills** | 17 skills synced mid-run, incl. `split-to-prs`, `update-cursor-settings` | A live vendor→agent instruction channel, including PR-opening guidance. |
| **Foreign skill harvesting** | 78 skills injected, incl. all of `~/.claude/skills` | No CLI-side allowlist (§8). |
| **PR/commit attribution on by default** | `attributeCommitsToAgent: true`, `attributePRsToAgent: true` | Presupposes the agent commits and opens PRs. |
| **MCP servers** | `agent mcp …`; config at `.cursor/mcp.json` **and** `~/.cursor/mcp.json`; `--approve-mcps`; per-project `mcp-auth.json` already populated (`plugin-expo-expo`) | Repo-local config means a cloned repo can propose MCP servers. `--approve-mcps` must never be set. |
| **`worker` verb** | `agent worker start` | **Downgraded**: docs confirm triple opt-in (team admin enables self-hosted agents, someone runs `worker start`, a session requests self-hosted routing). A normal run cannot trigger it. |
| **Plugin marketplace** | `agent plugin marketplace` | Third-party code discovery path. |
| **Worktree setup scripts** | `.cursor/worktrees.json`; `--skip-worktree-setup` | Repo-local file that executes scripts. |
| **Repo-local rules** | `agent generate-rule`, `.cursor/rules` | Competes with Ralphy's charter; disable path ⚠ unverified. |
| **Self-update** | `agent update`; versioned install tree | The two machines already differ (07.16 vs 07.17). Mid-run behaviour ⚠ unverified. |
| **Server-side tool classifier** | `--auto-review` | Sends tool-call decisions to a Cursor service. |
| **Sandbox** | `--sandbox enabled\|disabled`; `cursorsandbox.exe` ships on Windows too (in `versions/`), not only Linux; config default `sandbox.mode: "disabled"` | Available on both platforms; unexercised. |
| **Telemetry** | `statsig-cache.json` (536 KB) present before login | Feature-flag/telemetry service; opt-out ⚠ unverified. |

## 10. C10 — Cross-platform and I/O hygiene

- **Binary resolution.** Neither name is on `PATH` on Windows. The install is
  `%LOCALAPPDATA%\cursor-agent\` with **four** entry points — `agent.cmd`,
  `agent.ps1`, `cursor-agent.cmd`, `cursor-agent.ps1` — plus `versions\<ver>\`.
  On WSL: `~/.local/bin/cursor-agent`, also off `PATH` for non-login shells
  (the Kimi precedent). **Two names for one binary**: `resolve_program` must try
  both, and the error text always says `agent login` regardless of which was
  invoked. Cursor's CI recipe adds `$HOME/.cursor/bin` to `PATH` — **a third
  location** this install does not use.
- **Windows spawn shape.** `agent.cmd` is a batch shim that execs
  `powershell.exe -NoProfile -ExecutionPolicy Bypass -File cursor-agent.ps1`,
  setting `CURSOR_INVOKED_AS`. So a run is `.cmd` → PowerShell → `node.exe` →
  the CLI: three hops. `ralphy-proc-util::resolve_program` already handles
  `.cmd` shims via `PATHEXT` (the opencode precedent) — but **it resolves
  through `PATH`, and Cursor is not on `PATH`**, so a non-PATH probe like Kimi's
  is required. The `versions/` tree ships its own `node.exe`, `rg.exe`,
  `crepectl.exe`, `cursorsandbox.exe`, `better_sqlite3.node` and
  `merkle-tree-napi…node`.
- **Encoding**: no cp1252 damage on redirected stdout with `stream-json`
  (32 853 bytes, UTF-8 payload). The hazard is Ink raw-mode on stdin, not the
  codepage.
- **Version drift across platforms** already present (07.16 vs 07.17), and
  `agent update` exists.
- **WSL parity ✅ (P14)**: identical stream shape, identical envelope, stdin
  works, same auth strings — verified on the newer 07.17 build.
- **`ACCEPTS_IMAGES`** (ADR-0025): no attachment flag in `--help`; expected
  `false`. ⚠ unverified.

---

## A. Appendix — the full command surface (`agent --help`) 🔒

Captured logged-out, Windows `2026.07.16-899851b`. Identical structure on WSL.

### Global options

| Flag | Meaning |
|---|---|
| `-v, --version` | version number |
| `--api-key <key>` | auth key (or `CURSOR_API_KEY`) |
| `-H, --header <header>` | custom header on agent requests, repeatable |
| `-p, --print` | non-interactive; all tools including write and shell |
| `--output-format <format>` | `text \| json \| stream-json` |
| `--stream-partial-output` | text deltas (needs `--print` + `stream-json`) |
| `--mode <mode>` | `plan` (read-only) \| `ask` (read-only Q&A) |
| `--plan` | shorthand for `--mode=plan` |
| `--resume [chatId]` | resume a session; accepts a `create-chat` id (verified) |
| `--continue` | alias for `--resume=-1` |
| `--model <model>` | model id, with bracket parameter overrides |
| `--list-models` | list available models and exit |
| `-f, --force` / `--yolo` | force allow commands unless explicitly denied |
| `--auto-review` | server classifier auto-runs safe tool calls, prompts for the rest |
| `--sandbox <mode>` | `enabled \| disabled`, overrides config |
| `--approve-mcps` | auto-approve all MCP servers |
| `--trust` | trust workspace without prompting (headless only) |
| `--workspace <path-or-name>` | workspace dir or saved workspace name |
| `--add-dir <path>` | extra workspace root, repeatable |
| `--plugin-dir <path>` | load a local plugin directory (needs `.cursor-plugin/plugin.json`) |
| `-w, --worktree [name]` | isolated git worktree under `~/.cursor/worktrees/` |
| `--worktree-base <branch>` | base ref for the new worktree |
| `--skip-worktree-setup` | skip `.cursor/worktrees.json` setup scripts |

Undocumented but named in error text: `--auth-token`. Undocumented in `--help`
and verified working: **prompt via stdin**.

Environment: `CURSOR_API_KEY`, `CURSOR_AUTH_TOKEN`, `CURSOR_CONFIG_DIR`,
`XDG_CONFIG_HOME`, `NO_OPEN_BROWSER`, `HTTP(S)_PROXY`, `NODE_EXTRA_CA_CERTS`.

### Subcommands

| Command | Purpose | Machine-readable? |
|---|---|---|
| `login` / `logout` | authenticate / clear auth | — |
| `mcp` | `login \| list \| list-tools \| enable \| disable` | ⚠ |
| `plugin` | `marketplace` | ⚠ |
| `worker` | `start \| debug`; self-hosted cloud worker (triple opt-in) | `--management-addr` HTTP/Prometheus |
| `status` / `whoami` | auth status | ✅ `--format text\|json` |
| `about` | version, system, account | ✅ `--format text\|json` |
| `update` | self-update | — |
| `create-chat` | create empty chat, **return its id** | ✅ bare UUID on stdout |
| `generate-rule` / `rule` | generate a Cursor rule (interactive) | — |
| `agent [prompt...]` | start the agent | — |
| `ls` | list/resume chats | ❌ **crashes under piped stdin, exits 0** |
| `resume` | resume the latest chat | ⚠ |

---

## B. Probe log

| # | Probe | Status |
|---|---|---|
| P1 | stdin channel, 26 372 B with head/tail markers | ✅ both markers echoed, `inputTokens: 19264` |
| P2 | argv ceiling | ⏸ deferred — stdin proven |
| P3 | stream shape, `stream-json` | ✅ 6 discriminators, envelope with usage |
| P4 | sentinel as last line | ✅ survived to `assistant` and `result.result`, 3 runs |
| P5 | session store location & topology | ✅ two local stores, SQLite blob graph, **no tokens** |
| P6 | model enumeration & entitlement | ✅ 170 ids free; **all named models refused on Free**; config write-back found |
| P7 | progress fields vs actual changes | ✅ edit tool reports diffs; **shell tool reports nothing** |
| P8 | limits / quota exhaustion | ⬜ **open** — the last C-question |
| P9 | `--mode plan` headless | ✅ hard read-only, supersedes the charter — reject |
| P10 | Windows spawn shape | ✅ `.cmd` → PowerShell → node; `resolve_program` needs a non-PATH probe |
| P11 | **indexing opt-out** | ✅ `.cursorindexingignore` = 0 uploads (A/B on fresh repos); `.cursorignore` breaks the edit tool |
| P12 | **skills discovery roots** | ✅ 3 of 4 roots hit; `--plugin-dir` needs a manifest; no CLI allowlist exists |
| P13 | `--resume <create-chat id>` | ✅ adopted — `session_id` matches the minted UUID |
| P14 | WSL parity | ✅ identical on 07.17 |
| P15 | **`stop` hook in headless** | ❌ **does not fire**; only `beforeShellExecution` did |
| P16 | **skill body actually loads** | ✅ secret present only in the body was returned; invocation appears as a `readToolCall` |
| P17 | **failure taxonomy** | ✅ four shapes measured — tool failure ≠ run failure; preflight = 0 records + exit 1; kill = records, no envelope, **empty stderr** |
| P18 | `--resume` with a never-existing UUID | ✅ accepted silently; `create-chat` is optional |
| P19 | stream timing through a pipe | ✅ incremental — first record 8.1 s, gaps to ~7.4 s, envelope 22.0 s (a *file* redirect is block-buffered) |

Residual gap after P17: **`is_error: true` and any `subtype` other than
`"success"` were never reproduced** — no lever on a Free account forces them.
The parser handles them defensively; an unknown `subtype` is not success.

### Reproduction

Raw captures: `%TEMP%\cursor-probe\raw\` (24 files — stream JSONL, stderr,
payloads, and the three `worker.log`s from the indexing A/B).

P1, verbatim:

```console
cd C:\Dev\FinCal\.cursor-probe
"%LOCALAPPDATA%\cursor-agent\agent.cmd" -p --output-format stream-json --force ^
  < payload.txt > p1-stdin.jsonl 2> p1-stdin.err
```

P11 control, verbatim:

```console
cd %TEMP%\cursorlab-a   & agent.cmd -p --model auto --force --output-format json < payload.txt
cd %TEMP%\cursorlab-b   & agent.cmd -p --model auto --force --output-format json < payload.txt
findstr /C:"Applying change" %USERPROFILE%\.cursor\projects\*cursorlab-a\worker.log   :: 15
findstr /C:"Applying change" %USERPROFILE%\.cursor\projects\*cursorlab-b\worker.log   :: 0
```

### Lab hygiene

All mutations to `C:\Dev\FinCal` were reverted (probe dir, `.cursorignore`,
`.cursorindexingignore`, `.cursor/hooks.json`, planted skill roots); `git status`
matches the pre-spike snapshot. The operator's `~/.cursor/cli-config.json` was
repaired after the §4 write-back incident. `~/.cursor/skills-cursor/` (17
vendor-pushed skills) and `~/.cursor/chats/` remain — they are Cursor's own
state, not Ralphy's to delete.
