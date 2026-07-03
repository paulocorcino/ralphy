# CloudEvents HTTP event sink: the run as a remotely observable stream

Status: proposed (design interview 2026-07-03; not yet implemented).

Ralphy's observability already has a load-bearing shape: the core and the
adapters emit structured `tracing` events with stable messages and typed
fields, and every consumer is a `tracing_subscriber::Layer` folding that same
stream — the console presenter (ADR-0006), the `ralphy.log` file, and the
Telegram notifier (ADR-0007). The canonical decoder
([runstate.rs](../../crates/ralphy-cli/src/runstate.rs), `event_to_runevent`)
proves the point: the event bus is a *contract*, unit-tested per message, not
an accident of logging.

What is missing is a consumer that lives **outside the operator's machine**.
The motivating scenario is a web platform that follows runs live across
several developers' machines — multiple Ralphy processes, possibly concurrent
on one host — and that collects backlog/issue information *through Ralphy*,
never holding a GitHub token of its own. Ralphy is the bus; the platform is a
subscriber.

## Rejected alternatives

- **OTLP / OpenTelemetry export.** Instant Grafana/Datadog compatibility, but
  it drags in the `opentelemetry` SDK stack and presumes a collector as the
  destination, where the requirement is "POST JSON to a URL I configure". Too
  heavy for the need; revisitable later since the sink is just another Layer.
- **A bespoke NDJSON format.** Simplest possible, but the requirement is an
  established interchange format any tool can ingest without a custom parser.
- **Synchronous POST in the run path.** A slow endpoint would degrade an
  unattended overnight run — the one thing observability must never do.
- **Disk spool + replay (at-least-once).** Nothing in run telemetry justifies
  the extra state, dedup burden and replay code. Losing events is acceptable;
  losing the run is not.
- **Replacing the console presenter ("instead of stdout").** The sink is
  additive. Sinks are independent Layers by ADR-0006/0007 design; console,
  log file and Telegram are untouched.
- **A resident daemon that polls GitHub and pushes snapshots without a run.**
  Real, but deferred: it would turn a run-scoped CLI into a long-lived
  service. The design must not block it — a future daemon is just a periodic
  invoker of the same emission paths (see ADR-0020's `--push`).

## Decision

### 1. A fourth sink, CloudEvents 1.0 over HTTP

A new `tracing_subscriber::Layer` in the CLI, active only when an events URL
is configured. It decodes the bus through the same canonical decoder the
Telegram notifier uses and serializes each `RunEvent` as a **CloudEvents 1.0**
structured-mode JSON envelope (`Content-Type: application/cloudevents+json`),
POSTed to the configured URL. Event types live under the `dev.ralphy.*`
namespace; the full catalog, envelope and payload schemas are the living
contract in [docs/events.md](../events.md) — this ADR records the decisions,
that document records the shapes.

### 2. Delivery: asynchronous, best-effort, at-most-once

Events flow through a bounded in-memory queue (~1000) into a background
sender task. Transient failures (5xx, timeout, network) retry with short
backoff (~3 attempts, mirroring the posture of `gh_output`'s transient-retry
wrapper in [github.rs](../../crates/ralphy-core/src/github.rs)); 4xx is a
configuration error and drops immediately. On exhaustion or queue overflow
the event is dropped and a **single** warning reaches `ralphy.log` — emitted
*outside* the bus the sink consumes, so a failing endpoint can never feed
itself `run.notice` events in a loop. The run never waits on the sink. Consumers get
at-most-once delivery and must treat the stream as lossy: the heartbeat (§4)
and the CloudEvents `id` give them liveness and dedup.

### 3. Emitter identity: every event says who, where, which process

Multiple developers running multiple Ralphy processes on multiple OSes send
similar-looking events. PID alone cannot key anything (recycled by the OS,
repeated across hosts), so the **primary key is `runid`**: a ULID minted at
process start, unique without coordination. Everything else is attribution
and diagnostics. Because CloudEvents restricts extension attributes to simple
types, `runid` is the envelope's **only** extension (filterable without
parsing payloads), and the rest groups under a reserved `emitter` object
inside every event's `data`, keeping the header clean: `version` (binary —
which schema vintage is talking), `user` (`git config user.email` —
attribution to a person, zero new config), `host`, `os`, `pid` (find the
process among concurrent Ralphys on one host), `ip` (primary local IP,
best-effort diagnostic — never a key), `tz` (local timezone; the envelope
`time` is always UTC per RFC 3339, and the offset reconstructs local time).
The exact field table lives in [docs/events.md](../events.md).

### 4. Three new emissions the bus does not have today

The decoder consumes no run-boundary events — the Telegram notifier infers
them from Layer lifecycle. A remote consumer cannot. The runner therefore
gains stable events for **`run started`** (repo, queue labels, agent, branch
mode, deadline) and **`run finished`** (outcome, per-issue totals), and the
sink emits a **heartbeat** (~30s) carrying a compact `RunState` summary —
current phase, active issue, queue progress, token totals — so the platform
renders "now" without folding perfectly, and declares a run dead by heartbeat
silence rather than by guessing. The heartbeat keeps beating through
usage-limit sleeps (phase `sleeping`), so an hours-long ADR-0003 sleep is
never mistaken for a dead run; `run.finished` is emitted only on clean
termination — a crash or kill is *detected* by silence, never reported.

### 5. Configuration: global store, keyed per repo

`events.url` and `events.token` live together in a **global** store
(`~/.ralphy/events.toml`), holding one entry per repo — the same
global-but-repo-keyed shape the usage ledger already uses
(`~/.ralphy/usage/<project-id>.jsonl`, ADR-0008) and the Telegram precedent
of credentials in a global TOML. Nothing lands in `.ralphy/settings.json`:
the scratch dir is where the **unattended coding agent works**, and a secret
readable there is a secret one prompt-injected issue away from exfiltration.
Two reinforcements follow: the sink reads the token at boot and Ralphy
**strips** `RALPHY_EVENTS_TOKEN` from the environment of every child it
spawns (adapters never see it), and the env var remains as a per-run override
(CI, rotation) with precedence env > global entry > unauthenticated.

The operator UX is unchanged: `ralphy config set events.url …` /
`events.token …`, run inside a repo, write that repo's entry in the global
store. An absent entry disables the sink entirely, and non-users pay
nothing.

### 6. Depth: lifecycle, not firehose

The sink emits the full `RunEvent` contract plus the three new emissions —
roughly 20–40 events per issue: queue, per-issue phase transitions with token
breakdowns and models, every skip with its reason and parking label
(ADR-0016), usage-limit sleeps, knowledge consolidation, warnings. It does
**not** emit agent tool-calls, PTY output or any transcript/code content:
payloads carry metadata about the work, never the work itself — both a volume
and a secret-hygiene boundary. A finer debug level can grow later as a
`settings.json` knob without breaking the contract (additive evolution,
[docs/events.md](../events.md)).

## Consequences

- The platform can be built against [docs/events.md](../events.md) alone:
  fold events by `runid` exactly as the Telegram notifier folds `RunEvent`,
  dedup by `id`, group by `emitter.user`/`emitter.host`, detect death by
  heartbeat gap.
- An HTTP client dependency enters the CLI crate for the first time (choice
  of crate is a code-stage decision; the workspace has none today).
- The runner's stable-message contract grows two messages (`run started`,
  `run finished`); `event_to_runevent` and its per-event tests grow with it.
- Event payloads are additive-only from the first release; removing or
  renaming a field is a breaking change requiring a new event type or a
  versioned type name (rules in [docs/events.md](../events.md)).
- The deferred daemon mode and the ADR-0020 `--push` snapshot both reuse this
  sink unchanged — emission paths, identity and delivery semantics are shared.
