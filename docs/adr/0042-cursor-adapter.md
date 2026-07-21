# The Cursor adapter: a sixth vendor whose defaults must be refused before it runs

Ralphy gains a sixth agent CLI vendor, `cursor` (Cursor Agent CLI), as a new
isolated crate `ralphy-agent-cursor` implementing the same PTY-free `Agent`
trait ([ADR-0002](./0002-core-agnostic-adapter-boundary.md)). It is selected
**per run** by `--agent cursor`; the core keeps taking a single `&dyn Agent` and
never learns which vendor it holds ([ADR-0004](./0004-codex-adapter.md) D1).

The template is **Copilot**. Cursor shares Copilot's three defining traits: a
minted session id, a real terminal envelope, and a model axis that is a *plan
entitlement* rather than a CLI feature. Where Cursor is richer — a free
machine-readable auth answer, stdin proven at 26 KB, per-edit diffs in the
stream — the adapter takes the win. Where it is worse, it is worse in a way no
previous vendor was: **an ordinary run uploads the repository to the vendor's
servers, and the CLI writes back into the operator's config.** Half the
decisions below exist to refuse a default.

Grounded in **Cursor Agent CLI `2026.07.16-899851b` (Windows)** and
**`2026.07.17-3e2a980` (WSL)**, on a **Free** account, probed hands-on across
fifteen probes against `C:\Dev\FinCal` and two disposable control repos. Full
evidence — command surface, stream schema, session store, the indexing A/B, the
config write-back incident — is in
[docs/research/cursor-cli-adapter-spike.md](../research/cursor-cli-adapter-spike.md);
this ADR records the decisions, the spike records the observations.

Status: **accepted** — issue #243 (under PRD #242) landed the first slice:
`ralphy-agent-cursor`, the `--agent cursor` wiring, D6's indexing gate and D17's
config isolation. Usage accounting, skills materialization, the one-shot verbs
and Tier 4 are later slices.
Consistent with ADR-0002/0004/0005/0008/0013/0023/0030/0033/0034/0040; second
application of the [ADR-0040](./0040-agent-adapter-onboarding-contract.md)
onboarding contract, and the source of its Amendment 1.

## D1 — Selection is per run, via `--agent cursor`; the core is untouched

`CliAgent` gains a `Cursor` variant and `build_agent` boxes `CursorAgent` as
`Box<dyn Agent>`. Same stance as ADR-0004 D1 / ADR-0005 D1 / ADR-0028 D1 /
ADR-0041 D1, not re-litigated.

ADR-0040's canary applies: **three independent agent enums** share no
definition. `daemon/src/session.rs::Agent` is the one that fails silently —
`agent_flag` is exhaustive over the daemon's own enum, so a missing variant
compiles and only fails at runtime as `ArgvError::BadParam("agent")`. Add the
variant first and let the compiler walk the rest.

**Deferral LIFTED by #248** (see D19). The daemon now carries
`session::Agent::Cursor`, resolves the binary through the shared vendor locator
rather than a bare `PATH` name, and enforces D6 on the interactive launch. The
paragraph below is kept as the record of what #243 deferred and why.

**Deferred, deliberately, by #243.** The first slice did NOT add
`daemon/src/session.rs::Agent::Cursor`, so `from_query("cursor")` returns `None`
and the workbench rejects `agent=cursor` with `BadParam`. The reason is D14: the
daemon's `program_name` resolves a bare name on `PATH`, and this vendor is on
`PATH` on neither platform — a variant returning `"cursor-agent"` would compile,
pass review, and then fail to launch on the very machine this ADR was measured
on. Wiring it needs `ralphy_agent_cursor::locate_cursor` reachable from the
daemon, which is the workbench/Tier 4 slice's edit, not the run loop's. Until
that slice lands, an explicit `BadParam` is the honest answer; the issue that
undoes this supersedes this paragraph.

## D2 — The prompt goes in on stdin

```
agent -p --model <id|auto> --force --output-format stream-json
      --resume <minted-uuid>   < <charter on stdin>
```

No prompt argument. Ralphy's `prompt.plan.staged.md` is 25 917 bytes before any
issue body, against a Windows argv ceiling of ~32 KB. The spike verified stdin
end to end: a **26 372-byte** payload piped in with no `-p` text returned markers
planted on **both** its first and last line, `inputTokens: 19264`. Cursor's own
docs confirm print mode is inferred from piped stdin.

Three of six vendors now require stdin (Kimi ADR-0028 D2, Copilot ADR-0041 D2,
Cursor). Argv is the exception.

## D3 — Completion: the sentinel for intent, the envelope as the net; the `stop` hook is rejected

Cursor documents a `stop` hook — *"called when the agent loop ends"* — which
would have given deterministic completion without text scraping. **It does not
fire in the headless CLI.** A project `hooks.json` registering `stop`,
`beforeShellExecution` and `afterFileEdit` was exercised by a run that performed
both a shell call and a file edit; only `beforeShellExecution` fired, twice.

So completion stays the shared ladder ([ADR-0023](./0023-shared-outcome-classifier.md)),
fed by:

```json
{"type":"result","subtype":"success","is_error":false,"duration_ms":25253,
 "result":"…\nRALPHY_DONE_…","session_id":"…","request_id":"…",
 "usage":{"inputTokens":19264,"outputTokens":1303,"cacheReadTokens":5248,"cacheWriteTokens":0}}
```

- `DONE_SENTINEL` as the last line of `result.result` — verified surviving three
  runs including WSL.
- `result.is_error` and `subtype` as the structural signals.
- **Absence of the `result` record is itself a failure signal.** Cursor documents
  that on error the stream *"may end early without a terminal event"*, with the
  message on stderr and a non-zero exit.

Exit codes carry no semantics beyond 0/1 — there is no Kimi-style
`75 = RETRYABLE` — and `agent ls` returns 0 on a crash, so the exit code is
corroboration, never the primary signal.

### The failure taxonomy, measured

Four failure shapes were forced deliberately, because a spike that only ever
sees success writes an `outcome.rs` against half a contract:

| Shape | Exit | stdout | Envelope | stderr |
|---|---|---|---|---|
| **Tool call fails** (`exit 42` via the shell tool) | **0** | full stream | **`subtype:"success"`, `is_error:false`** | empty |
| **Preflight rejection** (`--workspace C:\definitely\not\here`) | 1 | **zero records** | none | `Error: Workspace directory does not exist: …` |
| **Killed mid-run** (`kill -9`) | — | partial stream | **none** | **empty** |
| **Interrupted** (`SIGINT`) | **130** | partial stream | **none** | `Aborting operation...` |
| **Auth / entitlement** (§D4, D8) | 1 | zero records | none | prose |

Three rules follow:

1. **A failed tool call is not a failed run.** The discriminator is inside the
   tool record — `shellToolCall.result.failure{exitCode, signal, aborted, …}`
   or `.permissionDenied{…}` instead of `.success` — and the run reports
   `success` regardless. The *degraded* predicate reads both; the outcome does
   not.
2. **Zero records plus exit 1 is a preflight rejection**, not a truncation.
   Ralphy can distinguish it from a dead child by the record count.
3. **Partial records with no envelope and an empty stderr is truncation** — the
   process died. This is the one case where stderr says nothing at all, so an
   adapter that classifies on stderr alone sees a silent success.
4. **An interrupt is distinguishable from a crash.** `SIGINT` exits **130** and
   prints `Aborting operation...`; a hard kill leaves an empty stderr. This is
   the one semantic exit code the vendor has, and it matters because it is the
   shape Ralphy's own budget and idle watchdogs produce when *they* stop the
   child ([ADR-0038](./0038-per-issue-budget-vs-idle-watchdog.md)) — "we stopped
   it" must not be reported as "it crashed".

**Amended by #245 — the model refusal is a fifth shape, and it is actionable.**
Both `--model` refusals (`Cannot use this model: <id>. Available models: …` for
an id outside the catalogue, `ActionRequiredError: Named models unavailable …`
for one the plan does not entitle) wear the *preflight rejection* shape above:
exit 1, zero records, prose on stderr. `model_refusal_stop`
(`ralphy-agent-cursor/src/model.rs`) recognizes them and returns the vendor's own
sentence, so the operator sees the flag to change instead of a bare `Stuck`.

Two traps that fix carries, both load-bearing:
- The adapter's `log` is stdout and stderr **merged**, and a working run's
  transcript can quote the sentence (this repository commits the refusal
  fixtures). The match is therefore anchored to the START of a line — stdout is
  stream-json, whose lines all begin with `{` — and additionally gated on rule
  2's zero-record shape.
- The entitlement sentence is **wrong about its own product**: a Free plan CAN
  name the first-party `composer-2.5`, verified live on 2026-07-21 (`exit 0`,
  `"type":"result","subtype":"success"`). Only non-`composer` ids are refused,
  which is why D4's exemption is expressed as "first-party is nameable" rather
  than as a paid-tier allow-list.

**Never reproduced:** `is_error: true`, or any `subtype` other than `"success"`.
Neither could be forced with the levers available on a Free account. The parser
must therefore handle them defensively — an unknown `subtype` is not success —
and this is a residual gap the validation note must close if it ever observes one.

### Output arrives incrementally, after an initial silence

Timed through a real pipe, the first record (`system/init`) arrived at **8.1 s**,
then records at 13.0 / 13.1 / 13.2 / 13.4 / 14.6 s, and the envelope at 22.0 s —
so inter-record gaps reach ~7.4 s and the run opens with ~8 s of total silence.
Any idle watchdog ([ADR-0038](./0038-per-issue-budget-vs-idle-watchdog.md))
tuned tighter than that will kill healthy runs.

(A redirect to a *file* is block-buffered and showed only 2 records after 14 s.
That is the shell's buffering, not the CLI's; Ralphy reads a pipe and is fine.)

## D4 — `--model` is always passed explicitly, including `auto`

**This is a correctness requirement, not a style choice.** `--model` writes back
into `~/.cursor/cli-config.json` — `model`, `selectedModel`, `modelParameters`,
`modelSelectionHistory` and `hasChangedDefaultModel` — and it does so **whether
the run succeeded or was rejected**. A rejected probe left a model the account
cannot use as the persistent default, and every later run that omitted
`--model` inherited it and failed the same way; purging the keys was not enough,
because the next failing run rewrote them. A *successful* pin persists just as
firmly: after one `--model composer-2.5` run, `agent about` reported the
operator's default as "Composer 2.5", and a nine-id probe left all nine in
`modelSelectionHistory`.

So the hazard is not "a typo breaks Ralphy" — it is **Ralphy silently
reassigning the model of the operator's own interactive Cursor sessions, on
every run**. D17 is the containment.

Therefore:

- Model resolution is `Option<String>`, exactly as ADR-0041 D5 requires for
  Copilot — **but `None` maps to `--model auto` on argv, never to omitting the
  flag.** On this vendor, omitting a flag does not mean "default", it means
  "whatever the last invocation left behind".
- No hardcoded model id. `--list-models` reports **170 ids on a Free account**
  and the entitlement is far narrower — but **not as narrow as the error message
  claims**. Nine ids were probed:

  | Result | Ids |
  |---|---|
  | **runs, exit 0** | `composer-2.5`, `composer-2.5-fast` |
  | refused, exit 1 | `claude-opus-4-8-thinking-max`, `gpt-5.6-sol-max`, `cursor-grok-4.5-low`, `gpt-5-mini`, `gemini-3-flash`, `kimi-k2.7-code`, `glm-5.2-high`, `gpt-5.4-nano-low` |

  The refusal says *"Free plans can only use Auto"*, and that is **wrong**:
  Cursor's own **Composer** family is nameable and runnable on Free. What is
  refused is naming a *third-party* model — including `cursor-grok-4.5-high`,
  which is precisely what `auto` **routes to on that same account**. So the
  restriction is on naming, not on reaching, and it exempts the vendor's
  first-party family.

  Three sources disagree — the listing (170), the error text ("only Auto") and
  the entitlement (Composer + auto) — and only a real request resolves it. An
  adapter that trusted the error text would wrongly tell a Free operator they
  cannot pin any model, when they can pin two. Hence `Option<String>` and no
  default, the ADR-0041 precedent with an extra twist.
- The free, deterministic rejection — `Cannot use this model: <id>. Available
  models: …` from an invalid id, before any paid call — is the actionable stop
  and the cheapest enumeration. Note it is a *different* error from the
  entitlement refusal, and only the former is free.

## D5 — Pricing normalizes the model id to its family

Cursor bakes reasoning effort into the id
(`<family>[-thinking]-<none|low|medium|high|xhigh|max>[-fast]`), so
`claude-opus-4-8` alone yields 16 ids and the bracket-override syntax
(`claude-opus-4-8[context=1m,effort=high,fast=false]`) leaves the reachable set
open-ended. Enumerating 170 literals in `PriceTable::default` (ADR-0034) is not
maintainable and would still miss the bracket forms.

`agent_slug` normalizes to the family key before the price lookup — strip a
trailing effort suffix, a `-fast` suffix, a `-thinking` marker and any bracket
expression. Unknown families still log "unknown model"; unknown *efforts* do not.

**The vendor normalizes the same way, which is the corroboration.** A run
invoked with `--model composer-2.5-fast` persisted `modelId: "composer-2.5"` —
the `-fast` suffix is a *parameter*, not part of the identity. Ralphy's
normalization is therefore matching the vendor's own model, not inventing one.

**Settled by #245.** The normalizer is
`ralphy_agent_cursor::model_family` (`src/model.rs`), and the rates were sourced
from cursor.com/docs/models into `crates/ralphy-cli/src/pricing/defaults.rs` —
one row per reachable family. Two corrections to the sketch above:

- The decoration order in the grammar is not fixed. The live catalogue spells
  both `claude-opus-4-8-thinking-max` and `claude-4.6-sonnet-medium-thinking`
  (and `gpt-5.5-extra-high`), so `model_family` strips to a *fixpoint* over a
  longest-first decoration list rather than in the sequence D5 assumed.
- Normalization happens at ATTRIBUTION, not at lookup: the adapter writes
  `Usage.model = model_family(requested)`, so the ledger persists the family key
  and `PriceTable::resolve` stays vendor-neutral (ADR-0004). The consequence is
  deliberate and worth knowing: the raw effort suffix is not retained, so a
  ledger row cannot distinguish `-max` from `-low` after the fact.

Every Cursor USD figure remains ADR-0034's counterfactual — Cursor bills
credits, not tokens — and `auto` is priced at the family the spike observed it
routing to, which is a guess about a router, not a published rate.

**This collides with [ADR-0004](./0004-codex-adapter.md)'s amendment**, where a
tier routes the model (sol/terra/luna) at a fixed medium effort. On Cursor the
effort *is* the id, so the tier→model mapping and the effort are one string.
The ADR-0004 mapping stays authoritative for *which family* a tier selects; the
effort suffix is this adapter's concern and is not exported to the tier vocabulary.

## D6 — Ralphy refuses to run Cursor in a repository that has not opted out of the codebase upload

An ordinary headless run spawns a background service that walks the workspace
and syncs a merkle tree of it to Cursor's servers. The first FinCal run produced
**476 `Syncing merkle` / `Applying change` lines** — from a task explicitly
forbidden to read files or run commands, with the operator's `ghostMode: true`
and `privacyMode: 1` already set, and it indexed the **parent repository**, not
the working directory it was given.

A controlled A/B on two fresh 12-file repositories settles the opt-out:

| Repo | Ignore file | `Applying change` |
|---|---|---|
| `cursorlab-a` | none | **15** |
| `cursorlab-b` | `.cursorindexingignore` = `*` | **0** |

The decision:

- **`.cursorindexingignore` is the opt-out.** `.cursorignore` also stops the
  upload but **denies the agent's edit tool** — and in the probe the agent
  routed around the denial via its shell tool, which is precisely the leak
  Cursor's own docs admit. Ralphy never writes or requires `.cursorignore`.
- **Ralphy does not create the file.** Writing into the operator's repository to
  disable a vendor's data flow is a decision Ralphy makes *for* the operator
  about their own code; that is not Ralphy's call, and the file would surface in
  their `git status` unexplained.
- **Ralphy refuses to start** when `--agent cursor` is selected and the working
  tree has no `.cursorindexingignore`. The preflight is an ADR-0013 stop with an
  actionable message naming the file, the one-line content, and what it prevents.
- The escape hatch is an explicit opt-in setting for an operator who *wants* the
  indexing (`cursor.allow_codebase_indexing_i_understand_the_risk`), mirroring
  ADR-0041 D7's shape. Ralphy never denies the operator a capability
  ([security posture](./0032-daemon-mode-supervised-launcher.md) precedent) —
  it denies a *silent* one.

### The gate covers every invocation, not just `run`

The indexing service is spawned by the CLI, not by Ralphy's run loop, so it
fires for the **one-shots too**. `diagnose_repo` and `triage_issues` execute
with their cwd inside the operator's repository and would upload it exactly as a
run does — a gate that only guarded `ralphy run` would leave the whole triage
surface open, which is the larger blast radius, not the smaller one.

So the rule is stated in terms of the child's working directory, not the verb:

> **Any `cursor` invocation whose cwd is inside a git repository requires
> `.cursorindexingignore` in that repository's root.**

`draft_issues` and `consolidate_knowledge` may run where there is no repository
at all; those are allowed through, because there is nothing to upload and
nowhere to put the file. The preflight resolves the repository root first and
skips the check when there is none — it must not degrade into "refuse
everything", which would make the one-shots unreachable.

This is the strictest stance Ralphy takes toward any vendor, and it is
proportionate: no other vendor transmits the repository as a side effect of
answering a question.

## D7 — The argv refuses the rest of the blast radius

Every run is spawned with, and only with, the autonomy it needs:

| Flag | Stance | Why |
|---|---|---|
| `--force` | **set** | required for non-interactive; note it is *"unless explicitly denied"* by the operator's `permissions.deny` (D8) |
| `--auto-review` | **never** | a server-side classifier that prompts for anything it deems unsafe — prompting is fatal headless, and it ships tool-call decisions to a Cursor service |
| `--approve-mcps` | **never** | `.cursor/mcp.json` is repo-local, so a cloned repository can propose MCP servers |
| `-w/--worktree`, `--worktree-base` | **never** | Ralphy owns its branches; `.cursor/worktrees.json` executes repo-local setup scripts |
| `--mode plan` / `--plan` | **never** | see D9 |
| `--trust` | **not set** | never needed across nine runs; revisit only if an untrusted-workspace prompt is ever observed |
| `--sandbox` | **left to the operator** | available on both platforms (`cursorsandbox.exe` ships on Windows too), unexercised by this spike; forcing a sandbox mode is a capability decision Ralphy has no evidence to make |

### The operator's deny list wins over `--force`, and it is visible

Measured: with `permissions.deny = ["Shell(git)"]` in the operator's config and
`--force` on the command line, a run asked to execute `git status --short`
produced

```json
{"result":{"permissionDenied":{"command":"git status --short",
  "workingDirectory":"C:\\Dev\\FinCal",
  "error":"Command blocked by permissions configuration","isReadonly":false}}}
```

Three things follow, and only the third needed a decision:

1. **It does not hang.** The denial is immediate and headless-safe — no
   interactive prompt appears, which was the real risk.
2. **It is a third tool-result discriminator**, alongside `success` and
   `failure`: `permissionDenied`, carrying the command, the cwd, the vendor's
   own error string and whether the call was read-only.
3. **The run still reports `subtype:"success"`, `is_error:false`, exit 0.** An
   operator whose deny list blocks something Ralphy needs gets a green run that
   quietly did less — and the earlier `.cursorignore` experiment showed the
   agent will *route around* a denial through another tool when it can, so the
   damage is not even consistently visible in the transcript.

So `permissionDenied` records are surfaced by the **degraded predicate**, the
same way failed tool calls are, and the run report names the blocked commands.
Ralphy does not read, validate or edit `permissions.deny` at preflight: the deny
list is the operator's deliberate policy, and a tool Ralphy never needed being
denied is not a warning worth interrupting them for. The stance is *make it
visible when it bites*, not *audit it in advance*.

Two capabilities are documented as out of reach and are **not** guarded, with
the reasoning recorded so a future reader does not re-open it: `agent worker`
requires triple opt-in (a team admin enabling self-hosted agents, an explicit
`worker start`, and a session requesting self-hosted routing), and the plugin
marketplace requires an explicit `--plugin-dir` with a `.cursor-plugin/plugin.json`
manifest.

## D8 — Auth detection reads the CLI's own structured answer

Cursor is the first vendor to answer authentication **free, deterministically
and machine-readably**:

```console
$ agent status --format json
{"status":"unauthenticated","isAuthenticated":false,"hasAccessToken":false,"hasRefreshToken":false,"message":"Not logged in"}
```

`is_cursor_auth_error` is therefore two-tier:

1. **Preflight (ADR-0013):** `status --format json` → `isAuthenticated`. This is
   still *behavioural* detection — the CLI's own answer, not inspection of its
   credential file at `%APPDATA%\Cursor\auth.json` — so the house style holds.
   **The exit code must be ignored: `status` exits 0 while logged out.**
2. **In-flight:** stderr markers, because a token can expire mid-run. Three
   distinct strings exist and a naive matcher misses one:
   - listing path — `Authentication required. Run 'agent login', pass --api-key/--auth-token, …`
   - execution path — `Authentication required. Please run 'agent login' first, …`
   - **invalid key — `⚠ Warning: The provided API key is invalid.`**, which does
     **not** contain `Authentication required`.

   The predicate matches `Authentication required` **or** `The provided API key
   is invalid`.

`CURSOR_AUTH_ERROR_MSG` names `agent login` verbatim — the string the CLI itself
prints, regardless of which of its two binary names was invoked.

**Env hygiene:** `CURSOR_API_KEY` and `CURSOR_AUTH_TOKEN` are left alone (Ralphy
sets neither, and scrubbing them would break an operator who authenticates that
way), but `CURSOR_CONFIG_DIR` and `XDG_CONFIG_HOME` are **passed through
untouched** — an operator isolating Cursor's config is exercising the only
defence against the D4 write-back, and Ralphy must not defeat it.

## D9 — Native plan mode is rejected; Ralphy's planner writes its own plan

`--mode plan` is hard read-only, and it says so in terms that override the
charter:

> *"Plan mode is active. The user indicated that they do not want you to execute
> yet — you MUST NOT make any edits, run any non-readonly tools … This
> supersedes any other instructions you have received (for example, to make
> edits)."*

Asked to write `.ralphy/plan.md`, it refused and the file was not created.
Ralphy's contract is that the planner **writes the plan itself**
([ADR-0009](./0009-split-planner-executor.md)), so the native mode cannot
satisfy it. The planning pass runs in the ordinary execution mode with the
planning charter, exactly as every other vendor. ADR-0040 predicted this answer;
it is recorded as measured, not assumed.

## D10 — Ralphy mints the session id

```console
$ agent create-chat
868f1553-01ac-4335-89c6-6c1f101d6009
$ agent -p --resume 868f1553-… …
{"type":"system","subtype":"init",…,"session_id":"868f1553-01ac-4335-89c6-6c1f101d6009",…}
```

The minted id is adopted. Ralphy knows the session id before spawning, so store
lookup is a primary-key read and the [ADR-0008](./0008-token-usage-tracking.md)
D10 snapshot-diff is unnecessary. Cursor's docs never promise this — it is
verified, not documented, so the adapter treats a mismatch between the minted id
and `system/init.session_id` as a hard error rather than assuming adoption.

**And `create-chat` turns out to be optional.** `--resume` with a UUID that has
never existed —
`--resume 00000000-0000-0000-0000-000000000000` — was accepted silently, exit 0,
with `system/init.session_id` echoing that exact UUID. So Ralphy generates its
own UUID and passes it straight to `--resume`, saving a process spawn and a
round trip per run. `create-chat` remains the documented path and stays in the
adapter as the fallback if a future build starts validating the id.

## D11 — Usage is captured from the stream; the interactive gap is stated, not faked

**No local store records tokens.** Cursor keeps two on-disk stores —
`~/.cursor/chats/<cwd-hash>/<sid>/store.db` (SQLite, a content-addressed blob
graph in two tables) and `~/.cursor/projects/<cwd-slug>/agent-transcripts/…jsonl`
— and neither carries a token count, a cost, or a credit. The only accounting is
`result.usage` in the live stream, which dies with the process.

- `usage.rs` captures `inputTokens`/`outputTokens`/`cacheReadTokens`/`cacheWriteTokens`
  from the terminal envelope. Cache read and write are already separated, so
  [ADR-0008](./0008-token-usage-tracking.md) D2 holds with no folding.
- **Records are incremental, so they are summed — measured, not assumed.** Two
  invocations against one minted session id:

  | Turn | `inputTokens` | `outputTokens` | `cacheReadTokens` |
  |---|---|---|---|
  | 1 | 18 336 | 16 | 128 |
  | 2 | **102** | 16 | **18 432** |

  Turn 2 reports its own 102 input tokens, not `18 336 + 102`; the first turn's
  context reappears as cache read. This is the **Kimi shape (sum)**, not the
  Codex shape (keep-last) — and it is the one place ADR-0040 C6 warns that
  guessing silently multiplies or divides the bill. The adapter's test asserts
  the sum against these two fixtures so the wrong choice fails.
- Model attribution is **not** in the stream (`system/init` reports the
  *requested* model, `"Auto"`); the resolved id lives only in a request blob as
  `providerOptions.cursor.modelName`. The adapter records the requested model
  and marks the resolved one unavailable rather than walking the blob graph.
- **`scan_cursor` (ADR-0033) enumerates sessions and reports tokens as
  unavailable.** It is still written — Tier 4 is not skipped — but it does not
  invent a number. An operator's *interactive* Cursor sessions are invisible to
  `ralphy usage`, and that is the honest answer.
- Note the unit mismatch: Cursor bills **dollar-denominated credits** at
  per-1M-token rates on a monthly anniversary reset. Ralphy's token counts are
  not Cursor's bill.

**The cumulative-vs-incremental question is settled** (above). What remains for
[0042-cursor-validation.md](./0042-cursor-validation.md) is narrower: whether
summing per-invocation usage across a whole issue matches what Cursor's own
dashboard bills, given the credit/token unit mismatch.

**Implemented** (#249): `crates/ralphy-agent-cursor/src/usage.rs` —
`parse_cursor_usage` sums the terminal `result.usage` records, wired into both
`plan()` and `execute()`; `cursor_session_store` locates the run's own scratch
store; `CURSOR_CREDIT_NOTE` states the credit/token unit mismatch once per
phase via `note_usage_provenance`. Tests assert the sum against the two live
P20 fixtures.

## D12 — Skills materialize into the repo-local Cursor root; the foreign harvest is accepted and documented

Cursor auto-discovers `SKILL.md` recursively under `.agents/skills`,
`.cursor/skills`, `~/.agents/skills`, `~/.cursor/skills` and — deliberately, per
its own docs — `.claude/skills`, `~/.claude/skills`, `.codex/skills`,
`~/.codex/skills`. Marker skills planted in three of these roots were all found;
`--plugin-dir` was not, because it requires a `.cursor-plugin/plugin.json`
manifest.

- **`skills.rs` materializes into `<repo>/.cursor/skills/`.** No flag, no env
  var, no manifest — the root is read by default. This is the cheapest skill
  delivery of any vendor.
- **The body loads, not just the name — verified.** A skill was planted whose
  frontmatter `description` deliberately did *not* contain a secret, while its
  body did. Asked to invoke it, the agent returned `RALPHY_VAULT_9K3X7Q`
  verbatim. The stream shows how: skill invocation appears as a **`readToolCall`**
  — the agent reads `SKILL.md` off disk on demand. So descriptions are injected
  eagerly (the token cost below) and bodies are pulled lazily, which is also why
  a skill Ralphy writes just before spawning is picked up with no cache to bust.
- **The foreign harvest is accepted.** In the probe the CLI injected **78
  skills** — the operator's entire personal Claude Code library and every plugin
  skill — into a single request, and a trivial "reply OK" run cost
  **18 212 input tokens** as a result. There is no CLI-side allowlist; the IDE's
  third-party toggle is confirmed not to apply to `cursor-cli`. Isolating
  `HOME`/`CURSOR_CONFIG_DIR` would suppress it but would also isolate the
  credential, forcing a second login — a worse trade for the operator.
  The adapter documents the behaviour and its token cost, and does not fight it.

**Implemented** (#246): `crates/ralphy-agent-cursor/src/skills.rs` +
`docs/configuration.md`'s Cursor section; `docs/live/cursor-246-skill-body.log`
re-verifies P16 (a planted skill's BODY, not its description, is read) under
Ralphy's own materialization rather than a hand-planted probe skill.

## D13 — Limits: pending

⬜ **Open, with a bound.** C7 is the one ADR-0040 question the spike did not
close. An exhaustion run was started and **stopped deliberately** after
**25 consecutive runs on the Free tier, 351 058 input tokens, zero failures** —
about six minutes of continuous driving at ~13 s per run. That is a useful
negative: the Free tier's ceiling is **not** low enough to be tripped by a short
burst, so a Ralphy queue will not discover it in the first few issues. It says
nothing about where the ceiling is.

What is known: Cursor publishes no numeric free-tier quota, no
machine-readable limit signal and no exit codes, and its cap message is
editor-framed. The `ActionRequiredError` class already carries a plan
entitlement refusal (D4) and is the leading candidate to carry the quota refusal
too, which would make a **class match** — not a phrase match — the right shape
(the OpenCode `usage_limit_regex` precedent, ADR-0040 C7).

Absent a reliable reset hint, `Limit(None)` and
[ADR-0030](./0030-synthetic-reset-for-unschedulable-limits.md)'s synthetic
~30-minute cadence apply automatically.

## D17 — Runs execute against an isolated `CURSOR_CONFIG_DIR`, seeded from the operator's own

D4 establishes that every `--model` — successful or rejected — rewrites the
operator's persistent default. Left alone, Ralphy would reassign the model of
its operator's interactive Cursor sessions on every single run. That is
unacceptable for a tool whose whole posture is "never change something the
operator did not ask for".

`CURSOR_CONFIG_DIR` is the containment, and it is cheap because of where the
credential lives:

| Under an isolated `CURSOR_CONFIG_DIR` | Result |
|---|---|
| `agent status --format json` | **still `authenticated`** — the credential is at `%APPDATA%\Cursor\auth.json`, outside the config dir |
| `-p --model composer-2.5-fast` | **exit 0**, ran normally |
| the operator's `~/.cursor/cli-config.json` | **untouched** — the pin did not leak |
| the isolated dir | received `cli-config.json`, `statsig-cache.json` and a `chats/` tree |

So: **each run gets a scratch config directory**, and Ralphy's chat records stay
out of the operator's chat list as a bonus.

**Seeded, not empty.** An empty scratch directory would also discard the
operator's `permissions.deny` policy, and D7 says that policy is deliberate and
Ralphy respects it. So the adapter **copies the operator's `cli-config.json`
into the scratch directory before spawning and never copies anything back**.
Policy flows in; mutations die with the run.

Two consequences to state rather than discover:

- The user-level skill root `~/.cursor/skills/` is not visible to an isolated
  run. This does not affect D12, which materializes into the *repository-local*
  root — but an operator with personal Cursor skills will find them absent under
  Ralphy, and that is a behaviour change worth documenting.
- The session store moves with the config dir, so D11's locator resolves it from
  the scratch path, not from `~/.cursor`. `scan_cursor` (Tier 4) still reads the
  operator's real `~/.cursor` — it is scanning *their* interactive sessions, not
  Ralphy's.

## D14 — Binary resolution probes two names in three places

Cursor installs **two names for one binary** — `agent` and `cursor-agent` — as
`.cmd` + `.ps1` shims on Windows (`%LOCALAPPDATA%\cursor-agent\`) and a single
file on Linux (`~/.local/bin/cursor-agent`), and it is **on `PATH` on neither**.
Cursor's own CI recipe names a third location, `$HOME/.cursor/bin`.

`ralphy-proc-util::resolve_program` already handles `.cmd` shims through
`PATHEXT` (the opencode precedent), but it resolves **through `PATH`** — so this
vendor needs an explicit probe list, the Kimi precedent (`~/.kimi-code/bin/kimi`).
The adapter tries, in order: `PATH` for both names, then
`%LOCALAPPDATA%\cursor-agent\agent.cmd` / `cursor-agent.cmd` on Windows, then
`~/.local/bin/cursor-agent` and `~/.cursor/bin/cursor-agent` elsewhere.

A Windows run is three hops — `.cmd` → `powershell.exe -NoProfile
-ExecutionPolicy Bypass -File cursor-agent.ps1` → the bundled `node.exe` — which
is why `CREATE_NO_WINDOW` and the existing `.cmd` routing both matter.

## D18 — The debug log is on by default, and Ralphy turns it off

The CLI writes a debug log for **every** invocation, unasked, to the OS temp
directory: `<tmpdir>/cursor-agent-logs-<user>/session-<iso>-<pid>-<n>.log`. This
spike alone produced **51 files, 514 KB**. Each records the working directory,
the repository context, the resolved user id, the file-index scan and a stream
of `analytics.track` events — the latter emitted even while the same log line
says `telem-lifecycle Telemetry disabled: privacy`.

`CURSOR_AGENT_DISABLE_DEBUG_LOG` is the off switch, found in the CLI bundle
rather than in any documentation. **Ralphy sets it**, for the same reason it
sets nothing else gratuitously: a queue run is not an interactive session, it
can produce hundreds of invocations, and silently filling the operator's temp
directory with logs naming their repositories is a side effect they did not ask
for. An operator debugging the vendor can unset it.

The same bundle scan surfaced the vendor's real environment surface, none of it
in `--help` — worth recording because several are levers a future decision may
want: `CURSOR_CONFIG_DIR`, `CURSOR_DATA_DIR`, `CURSOR_PLUGIN_ROOT`,
`CURSOR_WORKTREES_ROOT`, `CURSOR_ALLOWED_WRITE_SUBDIRS`,
`CURSOR_FORCED_SHELL_EGRESS` (plus allow/deny domain and writable-path
variants), `CURSOR_API_ENDPOINT` / `CURSOR_API_BASE_URL`,
`CURSOR_LOCAL_AGENT_BASE_URL`, `CURSOR_AGENT_CLI_AUTHLESS_MODE`,
`CURSOR_ENABLE_BEDROCK`, `CURSOR_RIPGREP_PATH`, `CURSOR_STATSIG_OVERRIDES`.

`CURSOR_ALLOWED_WRITE_SUBDIRS` in particular is a write-scoping lever Ralphy has
no equivalent for on any other vendor. It is **not** used today — no evidence
was gathered on its semantics — but it is the first thing to reach for if the
blast radius ever needs narrowing further.

## D15 — `ACCEPTS_IMAGES` is `false`

No attachment channel appears anywhere in the headless surface
([ADR-0025](./0025-triage-attachment-evidence-fetch.md)). A model that
advertises vision without a headless delivery path is `false`.

## D16 — Overlay slots

`assets/prompts/plan/overlay.cursor.md` exists even where slots are empty — the
assembly test is the anti-drift gate (ADR-0040 Tier 2). Filled:

- **`execution-model`** — the charter arrives on stdin as a single turn; there
  is no resume-with-more-instructions idiom in Ralphy's use of this vendor.
- **`skill-invocation`** — skills are discovered from `<repo>/.cursor/skills/`
  by name, alongside up to ~78 unrelated skills harvested from other vendors'
  directories; the plan must name the skill it wants precisely.
- **`mode-rules`** — the vendor's own plan mode is not in use (D9); the planning
  pass runs in execution mode and *must* write `.ralphy/plan.md` itself.

The remaining five slots are deliberately empty.

## D19 — The locator and the indexing gate live in `ralphy-proc-util`

`locate_cursor` / `locate_cursor_with` (D14) and `indexing_gate` (D6) are in
`crates/ralphy-proc-util/src/cursor.rs`. `ralphy-agent-cursor` keeps both entry
points and delegates, so its public API (`pub use command::locate_cursor`) is
unchanged.

The forcing constraint is
[ADR-0032](./0032-daemon-mode-supervised-launcher.md) §10: the daemon
never imports `ralphy-core`, and this adapter crate does — so a
daemon → `ralphy-agent-cursor` edge is out. But the workbench's interactive
launch spawns `cursor-agent` directly, and it needs BOTH: the locator (a bare
`PATH` name does not resolve this vendor) and the gate (a refusal the operator
can reach around by opening a console is not a refusal). `ralphy-proc-util` is
core-free, is already the daemon's program resolver, and already carries
vendor-shaped path knowledge (`CODEX_HOME`, `opencode.cmd`).

**Measured amendment to D14 (#248).** "On `PATH` under neither name" is not
universal: the Windows installer puts `%LOCALAPPDATA%\cursor-agent` **on `PATH`**,
so on such a host a plain `PATH` search for `cursor-agent` *does* find the
vendor — by accident, and only for that one install shape and that one name. It
is therefore not safe to test the daemon's routing by comparing the two
resolvers' live answers (they agree here), and still less safe to rely on the
accident in production. Both call sites pin the wiring at the source instead.

One implementation, because a product-stance refusal that two crates can
disagree about is worse than no refusal: the disagreement is invisible until an
upload has already happened. The daemon reads the opt-in flag by reparsing
`<repo>/.ralphy/settings.json` (`registry.rs`'s precedent for `repos.toml`),
defaulting to `false` on every failure path; a source-text pin
(`session::tests::the_optin_key_matches_the_adapters_own_schema`) reds if this
crate renames the key or the section.

## Consequences

- **Cursor is the first vendor Ralphy will refuse to run by default.** D6 turns
  a preflight into a policy gate. That is a new precedent and it should stay
  narrow: it is justified by a data flow the operator cannot see, not by taste.
- **The blast radius is priced in tokens too.** 78 harvested skills make a
  trivial run cost 18 KB of input. Any per-issue budget
  ([ADR-0038](./0038-per-issue-budget-vs-idle-watchdog.md)) tuned on another
  vendor will read wrong here.
- **`--model auto` on every argv** is the kind of line a future reader deletes as
  redundant. D4 exists so that reader finds the reason first.
- Tier 4 ships a `scan_cursor` that reports no tokens. That is a capability
  regression relative to every other vendor, and it is the vendor's, not
  Ralphy's.
