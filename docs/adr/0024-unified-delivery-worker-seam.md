# A unified event-delivery worker behind one transport seam

Status: accepted (implemented 2026-07-06, issue #132).

_Amends ADR-0007 (Telegram notifier) and ADR-0019 (CloudEvents sink): the
duplicated ring/worker/Layer/handle those two decisions each describe is now one
shared spine. Neither decision's observable contract (the Telegram card shape, the
CloudEvents wire shape) changes._

Two run-time event sinks — the Telegram notifier (ADR-0007) and the CloudEvents
HTTP sink (ADR-0019) — grew up mirroring each other. Each installed a
`tracing_subscriber::Layer` that decoded the event bus through the canonical
`event_to_runevent` and pushed onto a bounded, drop-oldest `Arc<EventQueue>` ring;
each ran one background worker that drained the ring, folded events into a
`RunState`, and drove a vendor transport behind a single-impl trait (`Transport`
vs `EventSink`); each carried a bounded-shutdown handle that joined the worker on a
helper thread under a timeout. The mirroring was literal enough that the sink
already imported the notifier's `EventQueue` — the shared substrate was
half-recognized but unnamed, and the ring/worker/handle/Layer were copied where the
compiler could not see them drift.

## D1 — One `crate::delivery` spine; the sink is a fold over it

A new binary-crate-internal module `crate::delivery` owns the substrate that was
never vendor-specific:

- **`EventQueue`** — the bounded, drop-oldest ring (moved verbatim from the
  notifier; the sink's ~1000 bound is just `with_capacity`).
- **`DeliveryLayer`** — the generic `tracing` Layer, parameterized by a runtime
  `self_target` substring so each sink keeps its own loop-guard marker
  (`"telegram::notifier"` / `"events::sink"`).
- **`run_delivery_worker` + `WorkerHandle` + `spawn_worker`** — the
  poll/drain/fold/shutdown lifecycle and the bounded-shutdown join.

The worker is parameterized by a **`DeliveryEngine`** fold — a per-tick state
machine, `on_start` / `on_event(RunEvent)` / `on_tick(changed: bool)` /
`on_finish` — that each sink implements. The engine owns its vendor transport
(`BotClient<T>` vs `T: EventSink`) and all message/envelope shaping; the worker
owns only the lifecycle. `TelegramEngine` maps to init-card / apply+sleep-push /
edit-gate / finished-footer; `CloudEventsEngine` maps to no-op / tokens+poller
reset+apply+deliver / poller-poll+heartbeat / no-op. `on_finish` runs on every
worker exit path, so a sink's terminal work (the Telegram `🏁` footer) is not tied
to how the loop ends.

## D2 — Sink-specific policy stays in the fold, not the spine

Retry-with-backoff is **not** a shared invariant. The CloudEvents sink retries a
transient POST up to four attempts; the Telegram notifier swallows each per-call
error with no retry. So `deliver` / `warn_dropped` and their tests stay in
`events/sink/delivery.rs`, tested once against a `ScriptedSink` fake — moving them
into the neutral module would couple it to the `EventSink` trait for one caller.
What the shared suite proves once is the substrate: the ring's drop-oldest
back-pressure (`event_queue_drops_oldest_at_capacity`), the Layer's off-the-run-path
enqueue (`layer_enqueue_is_off_the_run_path`), and the worker lifecycle +
bounded shutdown (`worker_drives_engine_through_lifecycle`,
`shutdown_returns_promptly_when_engine_blocks`) against a `FakeEngine`.

## D3 — The self-target loop guard is threaded, not dropped

A WARN/ERROR event folds into a `RunEvent::Notice` (`runstate/event.rs`), so a
worker's own `warn!` must stay filtered out of the ring or it loops. `DeliveryLayer`
keeps the runtime `self_target` `.contains` check. The one warn emitted from the
shared `WorkerHandle` (the detach notice on a wedged shutdown) cannot carry a
runtime target — `tracing`'s `target:` must be a compile-time literal — so each
sink passes a `detach_warn: fn()` hook that emits the notice under its OWN module
target (`ralphy_cli::telegram::notifier` / `ralphy_cli::events::sink`). Every other
engine `warn!` already sits in its vendor module and keeps its module-path target.

## Consequences

- The two ~590-line copies collapse to one spine plus two thin folds; a change to
  back-pressure, shutdown bounding, or the enqueue path is made and tested once.
- The transport seam the ADR-0019 motivation named ("two adapters, ADR-worthy") is
  now real: `DeliveryEngine` is the seam, and adding a third sink is implementing
  one trait, not copying a worker.
- Confined to `ralphy-cli` (a binary crate, no `lib.rs`): no external `pub` API
  moves. `telegram::notifier` re-exports `EventQueue` and `WorkerHandle as
  NotifierHandle`; `events::sink` re-exports `WorkerHandle as EventsHandle`, so the
  composition root and in-crate paths keep resolving.
- `crate::delivery` is deliberately NOT `ralphy-adapter-support`: that crate is
  vendor-neutral *child-driving* plumbing (CONTEXT.md — Adapter support), unrelated
  to event sinks.
