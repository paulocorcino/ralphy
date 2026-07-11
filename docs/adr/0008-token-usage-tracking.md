# Token usage tracking: per-adapter harvest, tokens as the truth, priced at read-time

Ralphy gains the ability to account for the token consumption of every agentic
operation it drives, accumulate it durably per project, and later answer "how
efficient is this task versus what it cost" and "which developer spent more for
the same work". This is a **measurement** capability: it observes and records,
it does not yet gate. It adds no new vendor surface and no new running infra —
each adapter harvests the `usage` its CLI **already reports**, from the source
that adapter **already touches**, and normalizes it into one vendor-agnostic
`Usage` the core sums.

This is grounded in the documented output of the three installed CLIs (Claude
Code's OTEL/`stream-json`/session-JSONL, `codex exec --json`'s `turn.completed`,
OpenCode's assistant-message `tokens`/`cost`), in a source-level read of how
ccusage and `littlebearapps/untether` extract the same numbers, and in the
existing transcript-reading code Ralphy already runs
([`ralphy-agent-claude/src/lib.rs:832`](../../crates/ralphy-agent-claude/src/lib.rs#L832),
[`:949`](../../crates/ralphy-agent-claude/src/lib.rs#L949)).

Status: accepted — implemented: the `Usage` type, the append-only
`.ralphy/usage.jsonl` ledger, `ralphy usage`, read-time pricing, and all four
harvest paths (Claude interactive-exec transcript correlation, Claude headless,
Codex rollouts, OpenCode `opencode.db`) are in production. D5's
`server_tool_use` web-tool request counts remain the one recorded gap.

## D1 — Harvest the vendor's own `usage`; no proxy, no OTEL collector, no unified disk-reader

The capture mechanism is **per-adapter in-line harvest**: each adapter reads the
token counts its CLI emits and fills a normalized `Usage`. Three alternatives
were considered and rejected:

- **A man-in-the-middle proxy** in front of each vendor's API. This is the
  approach a fresh tool reaches for when the CLIs are opaque — but they are
  **not** opaque: all three report `usage` in their structured output, so a proxy
  re-derives data we already have, while adding a long-lived process, TLS
  interception, and a single point of failure in the hot path. It is the
  textbook over-engineering this ADR exists to avoid.
- **An OpenTelemetry collector** (`claude_code.token.usage`, `codex.turn.token_usage`,
  and OpenCode's third-party `@devtheops/opencode-plugin-otel`). This is the
  org-grade path, but it requires standing up an OTLP endpoint as new running
  infra, OpenCode's OTEL is plugin-only, and Codex's OTEL **metrics are not
  emitted from `codex exec`** at all (openai/codex#12913) — exactly our mode. It
  buys fleet aggregation we do not need for a single-operator orchestrator.
- **A unified disk-reader** that parses all three vendors' session JSONL from a
  single component, decoupled from the spawn. Tempting because it is symmetric,
  but it concentrates three divergent schemas, three OS-specific path discoveries,
  and the cross-correlation problem (which session file belongs to which run) in
  one place — and inherits Codex's bug where interactive sessions do not persist
  `token_count` (openai/codex#9660). We borrow its *idea* only where a vendor
  forces it (Claude exec, D5).

The chosen path mirrors what untether and ccusage both converged on
independently: **trust the CLI's reported numbers**, normalize per-vendor shapes
into one, keep capture adjacent to the spawn the adapter already owns. No new
process, no new endpoint, no new daemon.

*(2026-07-10: ADR-0033 covers the complementary gap — **interactive** usage,
outside any run — with a stateless session-store scan served by the daemon.
It re-rejects the proxy and does not reopen this D1: the scan serves a
different consumer, off the run's hot path. It also adds one additive field,
`session_id`, to the D6 record as the run-vs-scan dedup key.)*

## D2 — Tokens are the truth; USD is a derived estimate, never a stored fact

Ralphy runs on the operator's **subscription**, not metered API billing — both
the Codex and OpenCode adapters already `env_remove` the API keys (ADR-0004 D5,
ADR-0005 D6) precisely so a stray key cannot flip the run to paid API. A
consequence the cost story must respect: the `total_cost_usd` Claude reports and
the `cost` OpenCode reports are **client-side estimates against a bundled price
table**, and under a subscription they are a *ghost* cost — not what the operator
is charged. Treating them as the unit of account would be both inconsistent
across vendors (Codex reports **no** USD; OpenCode sometimes writes `cost: 0` for
providers it has no pricing for) and misleading about real spend.

So the recorded unit of truth is **tokens** — `input`, `output`, `cache_read`,
`cache_creation` — which is also the unit that actually answers "is this task
efficient": tokens-per-issue is the efficiency signal, dollars are a
presentation layer over it. USD is **derived at read-time** from a local price
table (D8), always labelled an estimate, and **never written into the ledger**.
Rejected: storing the CLI-reported USD (inconsistent, ghost, and freezes a
mutable price into an immutable record).

## D3 — Granularity is the phase; issue / run / project are pure roll-ups

The finest unit recorded is the **phase** — one record for `plan`, one for
`execute`, per issue. This is the smallest seam that carries real signal: it
separates "planning was expensive" from "execution kept re-deriving", which is
where inefficiency actually hides and which a per-issue total would blur. Issue,
run, and project totals are then `SUM` over the phase records — no separate
aggregates are written, so they can never drift from the line items. Recording at
the phase now means never re-instrumenting for a finer cut later. Rejected:
per-issue (loses plan-vs-exec) and per-run (blind to which issue or phase spent).

## D4 — `Usage` rides on the return types; the core stays vendor-agnostic

A `Usage { input, output, cache_read, cache_creation, model }` struct lands in
`ralphy-core` — vendor-agnostic, no vendor names, exactly the kind of neutral
type ADR-0002 allows the core to hold. `Plan` and `Outcome` each gain a
`usage: Usage` field; `plan()` and `execute()` fill it, the core sums it. This is
explicit and type-safe — the propagation cannot be silently forgotten the way a
side-channel could — and the change is localized to the two return types. The
`model` field rides along because Claude swaps `sonnet`/`opus` by complexity
(ADR-0004 D3) and OpenCode runs whatever provider/model the operator chose
(ADR-0005 D4), so the model is only knowable per-record, and it is the key the
price table resolves on (D8).

Rejected: a stateful `fn last_usage(&self)` on the trait (out-of-order reads
under any future concurrency) and having each adapter write the ledger itself
(scatters persistence across three crates and denies the core the seam a future
budget gate needs, D9).

## D5 — Each adapter harvests from the source it already touches

The capture point is whatever the adapter is already adjacent to — never a new
read where an existing one serves:

- **OpenCode** — the **confirmed** token source is the on-disk session store, not
  the stream. Live data (below) shows OpenCode's assistant **message** records
  carry `tokens: { input, output, reasoning, cache: { read, write } }`, persisted
  in `~/.local/share/opencode/opencode.db` (the `message` table's `data` JSON;
  older builds use a `storage/message/**.json` tree). Whether the
  `run --format json` *stream* also emits these fields per `step_finish` was **not**
  verifiable here (a live spawn would not run headless in this environment), so the
  earlier "near-free off the already-parsed stream" claim is **downgraded to
  unconfirmed** — the adapter reads the DB/session record after the run, the same
  shape as the Claude-exec transcript path. The CLI's own `cost` is ignored (D2):
  live data proved it is `0` for the operator's providers.
- **Codex** — the **confirmed, richest** source is the rollout JSONL at
  `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`: its `event_msg` →
  `token_count` events carry `info.total_token_usage` and `info.last_token_usage`,
  each `{ input_tokens, cached_input_tokens, output_tokens,
  reasoning_output_tokens, total_tokens }`. `total_token_usage` is **cumulative**
  (proven monotonic on real data), so the phase total is the **last** non-null
  `token_count` of the session — and, unlike the `exec --json` stream (which
  discards the per-call `.last`, openai/codex#17539), the rollout **keeps both**.
  The adapter therefore reads the rollout file rather than changing its `-o`
  invocation. **Mapping trap, load-bearing:** Codex's `input_tokens`
  **includes** `cached_input_tokens` (total = input + output, cached is a subset),
  whereas Claude reports cache *separately*. To keep the normalized four-way split
  additive and comparable, the adapter maps Codex as `input = input_tokens −
  cached_input_tokens`, `cache_read = cached_input_tokens`, `cache_creation = 0`
  (Codex has no cache-creation concept) — summing raw `input_tokens` + cached would
  double-count.
- **Claude (plan)** — planning is already headless `claude -p`; add
  `--output-format stream-json` and read the terminal `result` event's `usage`
  (`input_tokens`, `output_tokens`, `cache_creation_input_tokens`,
  `cache_read_input_tokens`) and `modelUsage` per-model breakdown. Clean,
  documented, per-invocation.
- **Claude (execute)** — the hard one, because execution is an **interactive
  ConPTY session that emits no JSON** (ADR-0002 keeps the PTY adapter-internal).
  Its only token source is the session **transcript JSONL** under
  `~/.claude/projects/<dashed-cwd>/<session>.jsonl`, whose assistant lines carry
  the raw Messages-API `usage` block (including the `cache_creation` 5m/1h
  ephemeral sub-tiers). Ralphy **already reads** this file to detect usage limits
  ([`latest_transcript_text`](../../crates/ralphy-agent-claude/src/lib.rs#L834))
  — token harvest reuses that exact equipment, parsing each line's `message.usage`
  instead of scanning for limit text. The correlation risk this introduces is D10.

The uniform output of all four paths is one filled `Usage` (D4); the price table
(D8) makes them comparable downstream. Note the convergence the validation forced:
the **confirmed** source for every exec path is the vendor's on-disk session store
(Claude transcript JSONL, Codex rollout JSONL, OpenCode SQLite), read **per
adapter** — which is *not* the unified disk-reader D1 rejected (that was one
component parsing all three schemas); each adapter still owns its vendor's reader.
The live-stream optimism survives only for the Claude `plan` path, which is
genuinely headless.

- *Validated (spike probe, live against real on-disk data, 2026-06-15)*: a live
  `codex`/`opencode` spawn **would not run** headless in this nested environment
  (both hung with zero output and were killed) — itself a finding: unlike Claude's
  clean headless binary, these were validated from the session stores their past
  real runs already wrote, the same tactic that proved the Claude PTY path.
  **Codex**: across 74 real rollouts, **6 768** `token_count` events in 70 of them;
  the richest session's `total_token_usage` rose monotonically 8 058 → 64 712 285
  (cumulativeness **confirmed**), shape `{input_tokens, cached_input_tokens,
  output_tokens, reasoning_output_tokens, total_tokens}` with `last_token_usage`
  also present, and the cached-subset double-count trap above was found here.
  A bonus channel surfaced: many `token_count` events carry `rate_limits`
  (`used_percent`, `window_minutes`, `resets_at` for the 5h/weekly windows) — a
  future subscription-usage signal, not captured by the spike. **OpenCode**: the
  `opencode.db` `message` table held 177 assistant messages with the `tokens`
  schema above (summing input 603 216 / output 100 527 / cache_read 13 528 320),
  and — decisively — their `cost` summed to **exactly 0.0** across all 177 because
  the operator's models (`big-pickle`, `k2p5`, `k2p6`) are un-priced custom
  providers. That is the empirical proof behind D2/D8: trusting the CLI's `cost`
  would report **$0 for 13.5M+ tokens**; deriving from our own model-keyed table
  (with those opaque IDs hitting the unknown-model path) is the only honest cost.

- *Validated (spike probe, live against real `~/.claude/projects`, 2026-06-15)*:
  parsing `message.usage` off a real Claude-exec transcript works, but exposed a
  trap the design must encode — **dedup by `message.id` is mandatory.** The probed
  transcript had 155 lines, 70 carrying `usage`, but only **25 unique
  `message.id`**: 45 were duplicate records (resume/branch replays and
  parallel-tool-call lines that reuse one `message.id`). A naïve sum overcounts
  by ~2.8×. The capture keeps one record per `message.id`. A second trap: each
  assistant line nests an `iterations[]` array that **repeats** the top-level
  `usage` — it must be ignored, not summed, or every counted line doubles. The
  empirical split was input 20 126 / output 47 023 / cache_read 1 710 858 /
  cache_creation 94 566 — **91% of all tokens were cache reads**, which is why the
  four-way split (D2) is load-bearing: collapsing cache into input would overstate
  cost by an order of magnitude. The probe also found a billable signal outside
  the token counts — `usage.server_tool_use.{web_search,web_fetch}_requests` —
  recorded here as a known gap (web tool calls are billed per-request, not in
  tokens); the spike does not yet capture it. Finally, the transcript sum was
  **cross-checked against the authoritative source**: a real `claude -p
  --output-format json` run reported `usage` = input 3747 / output 9 /
  cache_read 0 / cache_creation 23406, and the transcript-derived sum for that
  session matched it **exactly** — confirming the harvest reconciles with the
  payload Anthropic itself returns. One headless-path gotcha surfaced: that JSON
  is preceded on stdout by a human-readable warning line ("no stdin data received
  in 3s…"), so the `plan` parser must skip non-JSON preamble (read the line
  starting with `{`/the `type:"result"` record), not blindly `JSON.parse` stdout.

## D6 — A central, append-only JSONL ledger, one line per phase, keyed by project

Accumulation needs a home that **survives** the thing being measured. Ralphy's
`.ralphy/` is per-run scratch on an ephemeral run branch it never pushes — a
ledger there is wiped. So the ledger lives **outside the repo**, central and
durable:

```
~/.ralphy/usage/<project-id>.jsonl     # one file per project, append-only
```

One JSON object per line, one line per completed phase, appended at phase end.
Append-only means no read-modify-write race and a trivially auditable, streamable
history. A project's budget question is "read one file and sum"; the cross-cut
questions (per actor, per model, per phase) are group-bys over the same lines.
The record:

```jsonc
{
  "project":        "owner/repo",          // D7
  "actor_email":    "dev@example.com",     // D7
  "actor_name":     "Dev Name",            // D7
  "ralphy_version": "0.1.0-rc5",           // env!("CARGO_PKG_VERSION")
  "issue":          42,
  "phase":          "execute",             // plan | execute
  "agent":          "claude",              // claude | codex | opencode
  "model":          "claude-opus-4-8",     // resolves the price table (D8)
  "outcome":        "done",                // terminal status of THIS phase
  "tokens":         { "input": 0, "output": 0, "cache_read": 0, "cache_creation": 0 },
  "ts":             "2026-06-15T12:34:56Z"
}
```

Two of those fields exist to make efficiency *measurable over time*, which is the
whole point of the capability. **`outcome`** is the terminal status of the phase
the line records, so a report can ask the question that matters — "what fraction
of tokens bought a `done` versus was burned on a `blocked`/`timeout`/`stuck`/
`limit`?". It arrives for free: the core's `Outcome` enum (ADR-0001/0003) is in
hand exactly when the execute line is written, riding the same return as `Usage`
(D4). One honest asymmetry the append-only ledger forces: the **execute** line
carries the real `Outcome`, but the **plan** line is written *before* the issue's
outcome is known (plan → execute → outcome), so it carries only `ok`/`failed` for
planning itself; an issue's outcome is its execute line's, joined by `issue` at
read-time, and "tokens wasted on non-`done` issues" sums both phases under that
join. **`ralphy_version`** stamps the orchestrator build (`CARGO_PKG_VERSION`,
zero-cost, always present) so a later report can answer "did the rc5 prompt change
actually lower tokens-per-`done`-issue versus rc4?" — without it, comparing
efficiency across optimizations is blind. Both are immutable facts, append-only-
safe, and add no hot-path work.

Note the absence of any `cost`/`usd` field — that is derived, not stored (D2/D8).
Rejected: a repo-committed ledger (pollutes the review branch, merge conflicts,
and dies with the un-pushed branch) and SQLite (a dependency, a schema, and
migrations heavier than a measurement spike warrants — JSONL already answers
every question posed).

## D7 — Project identity from the git remote; developer identity from git config

Two stable keys ride on every record, both read from git the operator already
configured:

- **Project** = the `git remote get-url origin` normalized to an `owner/repo`
  slug. It is stable across clones, branches, and machines, and it is the very
  target Ralphy already operates on (the repo's GitHub backlog), so two developers
  on the same project accumulate into **one** budget. Fallback when there is no
  remote: a hash of the repo-root path (single-machine, but never wrong).
- **Actor** = `git config user.email` (key) plus `git config user.name`
  (display). Email is chosen over name because names collide and change; and the
  committing identity is exactly "who ran Ralphy", since Ralphy commits the work —
  so the actor on the record is the author on the commit, with no ambiguity. This
  makes the per-developer efficiency consolidation a group-by `actor_email`, with
  no change to the file layout (D6) — partitioning files per developer was
  rejected because a developer spans many projects and that would break the
  per-project roll-up.

Known limitation recorded, not solved: `git config user.email` is machine
config — two developers sharing one machine/config collapse into one actor. It is
the best available signal; multi-tenant identity is out of scope for the spike.

## D8 — Price is a property of the model, applied at read-time, from an overridable table

USD is computed **when a report is read**, never when a record is written, by
multiplying the stored tokens by a price table. Three reasons this beats a
write-time `cost` field: (1) tokens are an immutable fact while price is a
mutable opinion — read-time pricing **re-prices the entire history** by swapping
the table, with the ledger untouched; (2) it sidesteps OpenCode's `cost: 0`
entirely, because the CLI's cost is ignored and all three vendors are priced
uniformly; (3) the adapters stay dumb — no pricing logic in the hot path.

The table is keyed by **model**, not agent — opus costs the same per token
whether Claude or OpenCode ran it, and OpenCode's "many models" are resolved by
the `model` already on each record (D4). It ships with sane defaults for the
models actually in use and is operator-overridable at `~/.ralphy/pricing.toml`
(values per 1M tokens):

```toml
[claude-opus-4-8]
input = 15.0
output = 75.0
cache_read = 1.5
cache_creation = 18.75
```

A model **absent** from the table reports **unknown** cost (and logs "add
`<model>` to pricing.toml"), never `0` — zero is a lie that hides spend, whereas
the tokens are still recorded and re-priceable once the entry is added. Rejected:
auto-syncing LiteLLM/models.dev pricing over the network (a runtime dependency,
and under a subscription USD is a *relative* efficiency proxy, not a bill — the
precision bar is deliberately low).

## D9 — Measurement only; the budget gate is future work, with the seam left for it

The spike stops at observe-record-report: at run end it prints a summary (tokens
by phase / issue / run, plus the project's running total and a read-time USD
estimate). It does **not** gate. Enforcing a budget is deferred, and the
honest reason is recorded so the future design starts informed: untether's
experience shows a post-run check can only detect an overrun **after** the tokens
are spent — it cannot pre-empt mid-run, only refuse the *next* issue. A real gate
is therefore a policy decision (per-run vs per-day ceiling, warn-percent,
soft-warn vs hard-stop, where in the queue loop the check fires) that wants its
own ADR. The seam is already in place: because `Usage` flows through the core
(D4) and the ledger is centrally readable (D6), a gate is a check the core's
queue loop adds between issues — no adapter change. Rejected for now: a per-issue
gate (scope creep into policy) and a real-time mid-run kill (needs a live token
stream and kill logic, out of scope).

## D10 — The spike pierces Claude-exec transcript correlation via snapshot-diff

The one seam the doc cannot settle on paper — and therefore the one the spike's
tracer-bullet proves — is **which** transcript file belongs to **this** run.
Ralphy's existing
[`newest_jsonl`](../../crates/ralphy-agent-claude/src/lib.rs#L949) takes the
**globally** newest `*.jsonl` under `~/.claude/projects` within a 5-minute window;
that is adequate for "did a limit appear" but unsafe for token attribution, since
a second concurrent Claude session would have its tokens misattributed. The
tracer therefore correlates by **snapshot-diff**: list the `*.jsonl` in the
**dashed-cwd** sub-directory **before** the session starts, list again **after**.
A file that **appeared** (a new session-id) is this run's transcript; a file that
merely **grew** is a *concurrent pre-existing* session (e.g. the operator's own
Claude Code open in the same repo) and is excluded. This appeared-over-grew rule
is load-bearing and is what makes the diff safe under concurrency: Ralphy spawns a
**fresh** `claude` per exec, so the run's transcript is always a new file, while
any other live session only ever grows — deterministic, with no need to scrape a
session-id out of the PTY TUI. (The headless `plan` path is unambiguous by
construction: `--output-format json` returns the `session_id` in its result
payload, so it reads its file directly without a diff.)

The dashed-cwd encoding is more than "separators → `-`": Claude maps **every
non-alphanumeric character** to `-` and **preserves the drive-letter case** from
the exact cwd string it was launched with. So `c:\Dev\ralphy` → `c--Dev-ralphy`
(lower-case `c`, because the launch cwd is lower-case), and a *dotted* path like
`C:\Dev\.ralph-worktrees\issue-10` → `C--Dev--ralph-worktrees-issue-10` (the `.`
becomes a second `-`). The adapter must derive the directory from the byte-exact
cwd it passes to `claude`, not a normalized form.

- *Validated (spike probe, live against real `~/.claude/projects`, 2026-06-15)*:
  the `isalnum→'-'` derivation reproduced every real directory name on the
  machine (including the dotted-path and case-preserved cases above). The
  snapshot-diff was then run **for real** around an actual `claude` invocation
  (not simulated): before/after the run, the new session's file **appeared**
  (`c6ab25d8-…jsonl`, matching the payload's `session_id`) and was correctly
  isolated. Crucially, the diff **also** flagged a second file that *grew* — a
  **concurrent** Claude session active in the same cwd — which is exactly the
  ambiguity the appeared-over-grew rule resolves, and which a naïve
  "appeared-or-grew" or the existing global `newest_jsonl` (it chooses across
  *every* project's transcripts, here 148+ files) would have mis-selected. The
  PTY exec path is covered too: three **real ralphy ConPTY runs** (the
  `C--Dev--ralph-worktrees-issue-*` transcript dirs) were parsed and each carried
  usable `message.usage` (model `claude-sonnet-4-6`, Ralphy's exec default),
  confirming an interactive session writes the same harvestable usage as a
  headless one. The only seam not exercised end-to-end is Ralphy *driving* the
  ConPTY spawn itself — but that harness already runs in production and produces
  exactly these transcripts. The cache-token mapping below is unchanged. The cache-token mapping is recorded as lossy-by-design: Claude exposes
`cache_creation` + `cache_read` (with 5m/1h tiers we sum), Codex exposes only
`cached_input` (a read), OpenCode exposes `cache.read` + `cache.write` (write ≈
creation); the `Usage` struct keeps the union and zero-fills what a vendor does
not report.

## D11 — The consumption layer: the ledger is the data warehouse; three thin reads over it

Measurement is worthless unread, so the report surface is part of the contract —
but it adds **no** storage or pipeline beyond the append-only JSONL of D6. The
ledger, being flat and per-line, **is** the data layer; everything here is a read
over it, and USD is applied at read-time from the price table (D8), never stored.

- **Live UI** — the ADR-0006 console presenter already prints a per-issue line
  with status and wall-time (`✅ #45 done (12m56s)`); it gains **tokens inline**
  (`✅ #45 done (12m56s · 1.2M tok)`) — efficiency visible as it runs, without the
  ghost-USD noise on every line — and a **run-end footer** carrying the run total
  and the project's accumulated balance plus a read-time USD estimate
  (`run: 6 issues · 8.4M tok · ~$2.10` / `project: ocs-inventory · 142M · ~$35.6`).
  The Telegram notifier (ADR-0007) can mirror the footer as a `/usage` reply, the
  way untether does; optional, not core.
- **`ralphy usage` query command** — a new subcommand (the CLI already has
  `ralphy hook …`), reading the ledger for the current project and printing the
  **balance** plus group-by cuts: `--by phase|model|actor|version`, `--since
  <date>`, `--project owner/repo`. `--by version` is what answers "did rc5 lower
  tokens-per-`done` versus rc4?" (D6's `ralphy_version`); `--by actor`, who was
  more efficient (D7).
- **Export** — `ralphy usage --format csv` flattens the nested `tokens` object
  into columns and adds the read-time USD column, with a header row, ISO
  timestamps, and a UTF-8 BOM so Excel opens it clean on double-click;
  `--format json` for pipelines. Because the ledger is already line-delimited
  JSON, it is **also** directly consumable by DuckDB/PowerBI with no export step —
  the "one piece of equipment" doing double duty.

This is the report surface of D9's measurement-only scope; it still does not gate.
Rejected: a separate metrics store or dashboard service (the JSONL already answers
every question, and a service is the infra D1 refused), and baking USD into the
export (it must stay a read-time projection so a re-priced table re-exports
correctly).

## Consequences

- The core gains a vendor-agnostic `Usage` type and a `usage` field on `Plan`
  and `Outcome`; the queue loop sums it and appends ledger lines. No vendor names
  enter the core (ADR-0002 holds).
- `ralphy-agent-opencode` reads tokens off events it already parses (near-zero
  change). `ralphy-agent-codex` adds `--json` to its `exec` invocation and reads
  the last `turn.completed.usage` per phase (the `-o` sentinel file is untouched).
  `ralphy-agent-claude` adds `--output-format stream-json` to the `-p` plan path
  and a transcript-`usage` parser reusing the existing reader for the exec path.
- A new central artifact, `~/.ralphy/usage/<project-id>.jsonl`, and an optional
  `~/.ralphy/pricing.toml`. Neither touches the target repo; the `.ralphy/`
  in-repo scratch is unchanged.
- Deliberately **not** built: a proxy, an OTEL collector, a unified disk-reader, a
  budget gate, real-time mid-run enforcement, network price-sync, and per-turn
  granularity — each rejected above with its reason, each reachable later from the
  seams this leaves.
- Known residual risks carried into implementation: Codex `usage` cumulativeness
  (D5, mitigated by one-process-per-phase), Claude transcript correlation under
  concurrency (D10, mitigated by dashed-cwd snapshot-diff), shared-machine actor
  collapse (D7, accepted), and price-table staleness (D8, mitigated by the
  editable table and the unknown-model flag). Each is a recorded limitation, not
  an open question blocking the spike.
- The spike's exit artifact is this ADR, enriched by a **throwaway probe** that
  proved D5/D10 against real `~/.claude/projects` data on 2026-06-15 (encoding,
  snapshot-diff correlation, and `usage` parse-and-sum with dedup). The probe
  touched **no** production code or repo file — it ran from a temp scratch dir and
  its only durable output is the `*Validated*` notes folded into D5 and D10 above.
  It ran a **real** `claude` invocation and three real ralphy ConPTY-run
  transcripts through the full pipeline. Four corrections it forced are now in the
  contract: mandatory `message.id` dedup (a naïve sum overcounts ~2.8×); the full
  non-alphanumeric, case-preserving dashed-cwd encoding; the **appeared-over-grew**
  correlation rule (pure snapshot-diff is ambiguous when a concurrent session is
  live in the same cwd); and the headless-JSON warning-preamble skip. The harvest
  was reconciled exactly against the payload Anthropic returns. The one seam left
  for implementation is Ralphy driving the ConPTY spawn itself, which the
  production harness already does. OpenCode and Codex remain proven-by-design; the
  first production slice (the `Usage` type, the Claude-exec capture, the ledger
  append) starts from these validated foundations.
