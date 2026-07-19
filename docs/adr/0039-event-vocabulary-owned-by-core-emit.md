# ADR-0039 — The event vocabulary is owned by `ralphy_core::emit`

**Status:** accepted
**Date:** 2026-07-19
**Refines:** ADR-0007 D6 (the decoder contract gains an owned emit side)
**Evidence base:** `docs/audit-events-2026-07-19.md` (§2–§3, findings F1/F5)

## Context

Every consumer of the run's lifecycle — console presenter, Telegram notifier,
CloudEvents sink, `ralphy.log` — hangs off one decoder,
`event_to_runevent(target, message, fields)` (`ralphy-cli/src/runstate/event.rs`),
which matches on the **message string literal** of `tracing` events. The emit
side of that contract is 30 distinct string literals hand-typed at ~25 sites
across `ralphy-core`, all four vendor adapters, and the CLI itself, each with
free-form field names (`up`, `cr`, `cw`, `out`, `steps_json`, …) and a level
convention (INFO; WARN/ERROR short-circuit to `Notice`).

The audit found the loop between emitter and decoder is closed **for 1 of the
30 strings**: `IDLE_REAPED_MSG`, a shared constant in `ralphy-adapter-support`
consumed verbatim by both its two emitters and the decoder arm, pinned by a
level-contract test. For the other 29, the decoder's tests pin *the decoder
given the string* — nothing pins *that the emitter emits that string, with
those field names, at that level*. Renaming a message, a field, or logging at
WARN compiles clean, passes CI, and silently blinds three sinks at once.

This failure mode is not hypothetical:

- The kimi adapter needed its own decoder test (`event.rs`,
  `decoder_maps_kimi_planning_and_executing`) precisely because forgetting the
  per-adapter arms left "the live line, the Telegram card, and the heartbeat
  phase all … stuck on planning".
- Field encoding already varies per adapter today: claude emits
  `model = %model` (Display), opencode emits `model = ?self.model` (Debug of an
  `Option`) — a skew invisible to every existing test.
- 9 of the 30 strings exist only to name which adapter is planning/executing
  (`planning with codex exec`, `executing with kimi --print`, …), so **every
  new adapter must edit the decoder twice** or ship broken observability.

The unergonomic emit side is also the root cause of the audit's other findings:
an 8-field hand-typed `info!` is harder than a `print`, which is why border
outcomes bypassed the bus (F2) and the presenter grew a private reducer (F3).

Dependency-wise the fix has one natural home: `ralphy-core` is the workspace
root — the runner, all adapters, `ralphy-adapter-support`, and the CLI already
depend on it.

## Decision

### 1. One typed emit function per lifecycle event, in `ralphy_core::emit`

A new module `ralphy_core::emit` owns the vocabulary. For each consumed
lifecycle event there is exactly one function — `emit::issue_started(number,
title)`, `emit::queue_built(…)`, `emit::plan_written(…)`, `emit::planning(cmd,
model, effort)`, … — which owns three things nothing else may restate:

- the **message constant** (also `pub`, for the decoder's `match` arm),
- the **field names and encodings** (one `info!` invocation, written once),
- the **level** (the "level wins" rule of ADR-0007 D6 makes level part of the
  contract: an event logged at WARN loses its identity).

The core runner, the CLI's boundary emissions (`queue built`, `run started`,
`run finished`, consolidation), and the adapters call these functions instead
of raw `info!`. Raw `info!`/`warn!` remains fine for anything that is *not*
part of the decoded vocabulary — logs are still just logs.

The decoder keeps matching strings, but on the shared constants. The transport
does not change: this is still tracing-as-bus (ADR-0006/0007 stand); the seam
is ownership, not plumbing.

### 2. The round-trip gate: one test per `RunEvent` variant

In `ralphy-cli`, a test-only capturing `Layer` records `(level, target,
message, EventFields)`. For **every** `RunEvent` variant there is a round-trip
test: call the `ralphy_core::emit` function under the capturing subscriber,
decode the captured event with `event_to_runevent`, assert the exact variant
and payload.

This is the enforcement mechanism — it generalizes the `IDLE_REAPED_MSG`
pattern from 1/30 to 30/30. From then on, drift in message, field name, field
encoding (`%` vs `?`), or level is a red test, not a silent operator outage.
A new `RunEvent` variant without a round-trip test is an incomplete change by
convention, mirrored in the module docs of both `emit` and `event.rs`.

### 3. The per-adapter phase strings collapse to two messages

`planning` and `executing` become single messages; the human-readable command
becomes a `cmd` field (`emit::planning(cmd: "codex exec", model, effort)`).
The decoder drops the 7 redundant arms; **adding an adapter no longer touches
`event.rs` at all** — the adapter calls the same two helpers everyone else does.

The visible cost is cosmetic: `ralphy.log` reads
`planning cmd="codex exec" model=…` instead of `planning with codex exec`.
The log-line format is explicitly *not* a stable contract; `docs/events.md`
(the wire contract) is unaffected — `Planning`/`Executing` variants and their
CloudEvents mapping are unchanged.

### 4. `IDLE_REAPED_MSG` joins the module; its import path survives

The idle-reap emission moves behind `ralphy_core::emit` like the rest.
`ralphy_adapter_support::IDLE_REAPED_MSG` remains as a re-export so existing
import paths keep compiling (the public-API-stability rule of CLAUDE.md).

The same absorption rule applies to any vocabulary that goes multi-emitter
**before** this ADR's migration lands. Concretely: issue #217 extends
`ApiDegraded`/`ApiRecovered` to the headless path, turning two single-site
literals (`agent-claude/src/interactive.rs`) into two-emitter events — #217
adopts the `IDLE_REAPED_MSG` pattern immediately (shared constants in
`ralphy-adapter-support`, consumed by both emitters and the decoder), and this
migration later absorbs those constants into `emit` with the same re-export
treatment. #217 is also independent evidence for Decision 3's seam: the
vendor-specific text (the degraded-line matcher) lives in the adapter, the
vendor-neutral clock and emission live in the shared crate — the exact split
`emit::planning(cmd, …)` applies to the phase events.

### 5. What this ADR deliberately does not do

- **`RunEvent` and the decoder stay in `ralphy-cli`.** Moving the semantic
  model into core would grow core's public surface with a CLI presentation
  concern and couple every adapter build to it. The contract is closed by the
  round-trip tests, not by colocating the types.
- **The wire format does not change.** Every envelope in `docs/events.md` is
  byte-identical before and after.
- **No typed event bus.** Rejected again, as in the audit: tracing-as-bus is
  cheap, keeps adapters free of CLI types, and gives buffering/filtering/file
  logs for free.

## Alternatives considered

**Shared string constants only (no emit functions).** Closes message drift but
not field-name, field-encoding, or level drift — `info!(count = …, MSG)` still
hand-types the fields at every site, and tracing macros require field names as
identifiers, so constants cannot cover them. The emit function is the smallest
unit that owns all three.

**Move `runstate` (RunEvent + decoder + fold) into `ralphy-core`.** Would let
emitters construct typed events directly and delete the string match. Rejected:
it is a much larger public-surface change; the decoder must exist anyway (the
bus transports tracing events, not enums); and core would inherit a model whose
churn is driven by presentation needs (card statuses, live-region hints).

**A macro that emits and registers the decoder arm.** Magic in exchange for
grep-ability; the repo is navigated by agents, and two plain functions (emit
fn + match arm) with a test binding them is more legible than one macro that
generates both.

## Consequences

- Emitting a lifecycle event becomes a one-line typed call — cheaper than the
  `print` it competed with. This is the precondition for the audit's Phase 2
  (border events) and Phase 3 (single fold), which land as separate changes.
- CI now owns the emit↔decode contract; the class of outage the kimi test
  memorializes cannot recur silently.
- `ralphy-core` gains a small module that does `tracing` emission (it already
  emits throughout the runner; this centralizes, it does not add a dependency).
- Migration is mechanical and behavior-pinned: the audit's Phase 0
  characterization tests capture today's `(message, fields, level)` triples
  before any emit site moves, so the swap is verified byte-equivalent — except
  the deliberate Decision-3 collapse, which the round-trip tests pin instead.
- One decoder arm per concept remains the law; the per-adapter arm family was
  the only violation and this removes it.

## Amendment (Fase 1a, #220): the `tracing` target collapses to `ralphy_core::emit`

Discovered while migrating: a helper cannot forward its caller's module path.
`tracing` builds each callsite's `Metadata` in a `static`, so `target:` must be
a compile-time constant — every event emitted through an `emit` helper carries
`target = "ralphy_core::emit"`, no matter which crate called it. A `run finished`
emitted by the CLI no longer reads `ralphy::run::report`.

Accepted, not worked around. It is safe because nothing routes on target: the
decoder ignores it (`event.rs`, `let _ = target;`), the default filter carries no
per-target directive (`run/wiring.rs`, `EnvFilter::new("info")`), and neither
delivery loop-guard marker (`events::sink`, `telegram::notifier`) collides with
it. The byte-identity claim above is over `(message, fields, level)` — that is
preserved exactly, and the Fase-0 characterization pins assert it verbatim.

Do NOT "fix" this by taking a `target` parameter: `tracing`'s macro will not
accept a runtime value there. A future need for per-emitter targets means giving
up the single-module design, not patching the helpers.
