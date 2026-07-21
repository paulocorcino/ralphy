# Interactive token usage: a stateless session-store scan, served by the daemon's `usage` verb

Status: proposed (design interview 2026-07-10; not yet implemented).

ADR-0008 made every **run**'s token consumption a recorded fact in the
`~/.ralphy/usage/` ledger. What stays invisible is **interactive usage** — the
tokens the operator burns driving Claude Code, Codex, OpenCode or Kimi
directly (CLI or IDE), outside any run. The future control plane wants the
consolidated picture ("tokens per delivery" includes the hand-driven part),
and it will **ask each daemon periodically** for it. This ADR decides how
that gap is closed. Vocabulary (**Interactive usage**, **Usage scan**) is
defined in [CONTEXT.md](../../CONTEXT.md).

Prior art examined for this decision: `junhoyeo/tokscale` (MIT, Rust core) —
a mature session-store scanner for 40+ agent CLIs — read at source level
(snapshot under `docs/oracle/`, gitignored), and the ANTHROPIC_BASE_URL
proxy approach (mattpocock's `agent-proxy` gist).

## Decision

### 1. No proxy — the session stores already record everything

The capture mechanism is **reading the vendors' own on-disk session stores**,
which record *every* session — interactive or ralphy-driven — with the same
usage payloads ADR-0008 already validated live:

- Claude Code: transcript JSONL under `~/.claude/projects/<dashed-cwd>/`
- Codex: rollout JSONL under `~/.codex/sessions/` (and `archived_sessions/`)
- OpenCode: the `message` table in `opencode.db`
- Kimi: `wire.jsonl` under `~/.kimi/sessions/` (`StatusUpdate` payloads) or
  `~/.kimi-code/sessions/` (`usage.record`, `usageScope: "turn"` only)

**Rejected: a man-in-the-middle proxy** (the user story's opening idea).
ADR-0008 D1 already rejected it for runs; for interactive use every reason
gets *worse*: the operator would have to inject `ANTHROPIC_BASE_URL`-style
overrides into every tool, shell and IDE; non-Anthropic endpoints add TLS
interception; and the proxy becomes a single point of failure in front of
the operator's primary work tool — to re-derive numbers the vendor already
writes to disk. tokscale — the mature wheel the story asked not to
reinvent — independently converged on scan-not-proxy.

### 2. Stateless read-time scan; tokscale is prior art, not a dependency

The scan is **recomputed from scratch on every query**: parse the stores,
deduplicate in memory, answer. No watermarks, no delta files, no harvest
state — every piece of state discarded here was a source of double-count
bugs, and tokscale proves personal-machine volumes parse in negligible time.
Per-vendor in-memory dedup follows the validated rules: Claude by
`message.id` (ADR-0008's ~2.8× overcount trap) with tokscale's
`message.id:requestId` max-merge refinement, Codex by converting cumulative
`total_token_usage` to monotonic-guarded deltas, Kimi by `message_id`
keep-largest (progressive `StatusUpdate`s), OpenCode by message row id.

**Rejected: vendoring tokscale.** Its Rust core is a NAPI binding crate
(npm-distributed, not on crates.io) coupled to a TypeScript/Bun CLI, and
carries 36 parsers for tools not in use. We **port the parsing logic** for
our four vendors only — the Kimi parser from tokscale's `kimi.rs` (MIT,
attributed in the module header), the other three from our own
production-validated adapter harvest logic (ADR-0008 D5).

**Rejected: a stateful harvester** appending interactive lines to the
ledger. It needs per-session watermarks, delta records, and a state file —
and (§4) durability on this side is redundant anyway.

### 3. Served as the `usage` verb in the daemon's read-only query family

The platform **pulls**: a `usage` verb joins the ADR-0032 §6 read-only
request/response family (beside the forge queries — same shape, though it
never touches the forge), answered by the daemon from local data. Parameters:
a `since` cursor (filters interactive sessions by `last_ts` and ledger lines
by `ts`, purely a payload economy). Phase 1 serves it on the local HTTP
listener under the existing bind/token rules; Phase 2 rides the tunnel
unchanged. The CloudEvents contract does not move: run telemetry keeps its
sink (ADR-0019, ADR-0032 §5), and no periodic usage events are invented.

The response carries **two record kinds**:

- `run` — the ledger's lines as they are (ADR-0008 D6, plus §5's
  `session_id`), the durable per-phase facts.
- `interactive` — aggregated **per session × model**:
  `{ agent, model, session_id, project, actor_email, tokens
  {input, output, cache_read, cache_creation}, first_ts, last_ts }`,
  plus the responding daemon's `daemon_id` on the envelope.
  `tokens` is that object **or `null` when the vendor records no token count
  anywhere** — the key is always present, and `null` means *unavailable*,
  never *zero*. Cursor is the first such vendor (ADR-0042 D11: it bills in
  dollar-denominated credits and keeps no per-session totals in either
  store); a zeroed object would ship `0` on the wire and read as "this
  session spent nothing", so absence is encoded as absence. Consumers must
  handle `null` for every vendor, not only Cursor (#250).

Tokens only, no USD — pricing stays a read-time projection wherever the data
is consumed (ADR-0008 D2/D8).

### 4. Durability is the platform's; growing sessions are handled by upsert

The platform polls periodically and **persists what it receives, upserting
by `session_id`**: a still-growing session simply reappears on the next poll
with larger totals and overwrites its previous row. Idempotent by
construction — no deltas, no cursor bookkeeping beyond `since`, and a missed
poll heals itself on the next one.

The accepted trade-off, recorded honestly: the vendors' session stores are
**not durable archives** (Claude Code prunes transcripts per
`cleanupPeriodDays`, ~30 days by default; Codex/OpenCode retention is
non-contractual). If the platform stays down longer than the retention
window, that slice of interactive history is lost. That is deliberate: the
warehouse is the platform, and building a second durable store on the daemon
side to guard against a mostly-down warehouse is machinery without a buyer.
The local `ralphy usage` command (ADR-0008 D11) keeps reading the ledger
only; extending it with `--interactive` over the same scan crate is a cheap
future slice, not part of this decision.

### 5. Run-vs-scan dedup: the ledger lines gain `session_id`

Every run already knows its vendor session identity at the moment it writes
its ledger line (Claude plan: the result payload's `session_id`; Claude
exec: the snapshot-diff file name, ADR-0008 D10; Codex: the rollout it
reads; OpenCode: the session row; Kimi: its session dir). The ledger record
gains an **additive `session_id` field**, and the scan's exclusion rule is:
*a session whose id appears in the ledger is a run's — skip it*. Pre-change
ledger lines lack the field; that is harmless because their sessions age out
of the vendors' retention window regardless.

**Rejected: the directory heuristic** ("runs live in `.ralph-worktrees/`,
interactive lives elsewhere") — it lies the day a run executes in-place or
the operator opens an interactive session inside a worktree to debug.
Session identity is a fact; the directory is a circumstance.

### 6. Project attribution resolves through the repo registry

Interactive records are attributed to an `owner/repo` project (the ADR-0008
D7 identity) via the daemon's **repo registry** (ADR-0032 §3): Codex
(`session_meta.cwd`), OpenCode (`directory`) and kimi-code (workspace path
segment) expose a real cwd, matched against registered repo paths. Claude's
dashed-cwd directory key is lossy (irreversible), so the match runs the
other way: each registered repo path is dashed-encoded with the exact
ADR-0008 D10 rules and compared for equality. A session matching no
registered repo is reported with `project: null` and the raw workspace key —
**reported, never dropped**; unattributed spend is still spend.

Actor identity mirrors D7 with the same recorded limitation: `actor_email`
is read from the attributed repo's `git config user.email` at scan time
(`null` when unattributed), and the daemon's `daemon_id` is always present —
the machine identifies itself; no identity is pretended beyond that.

### 7. A new sync crate, `crates/ralphy-usage-scan`; adapters untouched

The scanner is a synchronous library crate with one module per vendor,
consumed by `ralphy-daemon` (the async bridge stays in the daemon, per
ADR-0032 §10's confinement). The adapters keep their in-run harvest exactly
as ADR-0008 shipped it — that is a different job (one correlated session, at
spawn time, on the run's hot path) over the same files, it is in production,
and the public-API stability rule applies. The parse-logic duplication
between adapter harvest and scanner is accepted and honest; consolidating
them is a future decision with its own buyer.

This does not reopen ADR-0008 D1's rejection of a "unified disk-reader":
that rejection was about replacing the adapters' run-time capture with one
component in the run path. The scan serves a different consumer (the
platform's periodic question), off the run's hot path, where
one-component-many-schemas is exactly the right shape — the shape tokscale
validated.

### 8. Default-on everywhere, because "on" costs nothing

There is no background job, no scheduler, no config flag: the scan executes
only when the verb is asked. A vendor store that does not exist contributes
zero. Store paths resolve per-OS (Windows and Linux both, per CLAUDE.md's
cross-platform rule), and each daemon scans **its own environment's** stores
only — the WSL daemon reports WSL usage (ADR-0032 §3, "WSL is just Linux"),
never across the boundary.

## Pre-implementation spike (three local checks — executed 2026-07-10)

1. **Codex interactive rollouts carry `token_count`: confirmed.** A
   same-day interactive session (`originator: "codex-tui"`, `source: "cli"`,
   cli 0.144.1, cwd `C:\Dev\ralphy`) held 17 `token_count` events with both
   `total_token_usage` and `last_token_usage` fully populated — the
   openai/codex#9660 gap is resolved on the current build. The events also
   carry the `rate_limits` block ADR-0008 D5 flagged as a future
   subscription-usage signal. `~/.codex/archived_sessions/` does not exist
   on this machine; the scanner treats it as optional.
2. **Kimi store format matches the oracle snapshot: confirmed** for the
   legacy store — 73 of 93 `wire.jsonl` files carry `token_usage`, and the
   newest (protocol 1.10, same-day) has the exact `StatusUpdate` shape the
   ported parser expects (`input_other` / `output` / `input_cache_read` /
   `input_cache_creation`, `message_id` for dedup). Two findings the
   snapshot's parser does not cover: (a) sessions nest
   **`subagents/<id>/wire.jsonl`** files that carry their own usage — the
   scanner must glob `**/wire.jsonl` and attribute subagent files to the
   *parent* session directory, not the subagent id; (b) the local
   `~/.kimi-code` store holds only config-stub sessions (no `usage.record`
   lines yet), so the kimi-code branch of the parser ships
   ported-but-locally-unvalidated — flagged for verification on first real
   kimi-code usage. Zero-usage files (interrupted/config-only sessions, ~20%
   of the store) parse to empty and cost nothing.
3. **Effective Claude retention: the default** — `cleanupPeriodDays` is not
   set in `~/.claude/settings.json`, so the ~30-day default applies.
   Recommendation adopted: pin it explicitly (e.g. `90`) on each machine
   before the platform's ingestion exists, so the first-ever poll can
   backfill months of interactive history instead of one; once the platform
   polls steadily, the value only needs to beat its worst outage window.

## Consequences

- The daemon's query vocabulary grows one read-only verb (`usage`);
  authorization stays binary (nothing writes). The CloudEvents contract,
  the ledger's role, and ADR-0008's harvest paths are all untouched.
- The ledger record gains one additive field (`session_id`), written by all
  four adapters from data already in hand.
- A new crate, `crates/ralphy-usage-scan` (sync, no tokio), with four
  vendor modules; the Kimi module carries MIT attribution to tokscale.
- The platform's ingestion contract is fixed early: poll `usage`, upsert
  interactive records by `session_id`, treat run records as append-only
  facts.
- Deliberately **not** built: any proxy, tokscale as a dependency, a
  stateful harvester, interactive writes to the ledger, parsers for tools
  not in use (Cursor, Copilot, …), USD in the verb's response, and a
  daemon-side durable archive of interactive history — each rejected above
  with its reason, each reachable later from the seams this leaves.
- Known residual risks: vendor session-store formats are non-contractual
  and can drift (mitigated: parsers are small, isolated per vendor, and the
  oracle snapshot pins the reference); retention-window loss when the
  platform is down long (accepted, §4); shared-machine actor collapse
  (inherited from ADR-0008 D7, accepted).
