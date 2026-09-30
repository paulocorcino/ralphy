# Audit: event flow, normalization, and sink reuse — 2026-07-19

Deep diagnosis of the run's observability spine (tracing bus → decoder → sinks),
requested before any ADR is drafted. Every claim below was verified against the
working tree on `feat/mini-ide` at audit time; citations are `file:line`.

**Verdict up front:** the architecture (tracing-as-bus, canonical decoder, pure
fold, `DeliveryEngine` seam) is sound and should not be replaced. But this audit
finds **two contract-level defects** (F1, F2) and **one structural duplication
with an observable bug** (F3) that are real, not cosmetic. F4–F6 are lower-interest
debt. The proposal (§5) tightens existing seams; it introduces no new architecture.

---

## 1. The as-is architecture (verified)

There is no typed event bus. The bus is `tracing`; producers emit
`info!("<literal message>", fields…)` and every consumer is a
`tracing_subscriber::Layer` installed by `init_tracing`
(`crates/ralphy-cli/src/run/wiring.rs:244`), composed in
`install_observability` (`crates/ralphy-cli/src/run.rs:501`, called at
`run.rs:56` — before any border check, so the Layers *are* live at the borders).

The single point of normalization is the canonical decoder
`event_to_runevent(target, message, fields) -> Option<RunEvent>`
(`crates/ralphy-cli/src/runstate/event.rs:171`): a `match` over the **message
string literal**, WARN/ERROR short-circuiting to `RunEvent::Notice` ("level
wins", `event.rs:174`). `RunEvent` has 28 variants (`event.rs:18`).

Four consumers share that decoder:

| Sink | Mechanism | Fold |
|---|---|---|
| `ralphy.log` / stderr | `fmt::layer` | none (raw lines) |
| Console presenter | `Presenter: Layer` (`ui/presenter.rs:573`) → render thread | **own** `LiveState` + `QueueState` (see F3) |
| Telegram | `DeliveryLayer` + `TelegramEngine` (`telegram/notifier.rs:348`) | `RunState::apply` |
| CloudEvents | `DeliveryLayer` + `CloudEventsEngine` (`events/sink/delivery.rs:109`) | `RunState::apply` → `runevent_to_cloudevent` (`events/envelope.rs:220`) |

The shared spine (`crates/ralphy-cli/src/delivery.rs`, ADR-0024) is correct:
bounded drop-oldest ring, off-run-path enqueue, `DeliveryEngine` trait
(`delivery.rs:154`), bounded shutdown, self-target loop guard.

The pure fold `RunState::apply` (`runstate/state.rs:204`, ADR-0007 D6) is well
tested and is instantiated by both `DeliveryEngine` sinks — but **not** by the
presenter, despite `runstate.rs`'s own doc saying it was designed to be shared.

Dependency graph (from `crates/*/Cargo.toml`): `ralphy-core` is the root; the
four adapters and `ralphy-adapter-support` depend on it; `ralphy-cli` depends on
all of them. Anything placed in `ralphy-core` is importable by every emitter and
by the decoder.

---

## 2. Vocabulary inventory: every consumed message and its protection

30 distinct message strings are contract. Emit sites verified by grep:

| Message | Emitter | Decoder arm | Drift protection |
|---|---|---|---|
| `queue built` | cli `run.rs:614` | `event.rs:182` | decoder tests only |
| `run started` | cli `run.rs:328` | `event.rs:310` | decoder tests only |
| `run finished` | cli `run/report.rs:75` | `event.rs:321` | decoder tests only |
| `consolidating knowledge` / `knowledge consolidated` | cli `run/report.rs:39,43` | `event.rs:303,306` | decoder tests only |
| `issue started` | core `runner/phases.rs:285` | `event.rs:195` | decoder tests; core `tests/queue.rs` pins some strings against **its own copies** of the literals |
| `plan written` | core `phases.rs:403` | `event.rs:208` | ditto |
| `plan opened` / `plan closed` | core `phases.rs:407,977` | `event.rs:215,219` | ditto |
| `green — issue closed` | core `phases.rs:960` | `event.rs:235` | decoder tests only |
| `non-green — stopping run` | core `runner.rs:299` | `event.rs:240` | decoder tests only |
| `bundle plan — needs split` | core `phases.rs:431` | `event.rs:244` | decoder tests only |
| `blocked by open issue(s) — skipping` | core `phases.rs:267` | `event.rs:245` | decoder tests only |
| `blocked — waiting on human` | core `phases.rs:275` | `event.rs:254` | decoder tests only |
| `stop-before label — halting run before this issue` | core `runner.rs:190` | `event.rs:258` | decoder tests only |
| `human-return label — skipping issue` | core `runner.rs:208` | `event.rs:268` | decoder tests only |
| `verify gate failed — skipping issue` | core `runner.rs:371` | `event.rs:277` | decoder tests only |
| `deadline passed — not starting issue` | core `runner.rs:176` | `event.rs:283` | decoder tests only |
| `usage limit — waiting for reset` / `reset reached — resuming` | core `clock.rs:112,120` | `event.rs:286,290` | decoder tests only |
| `api degraded — child retrying` / `api recovered — child resuming` | claude `interactive.rs:240,241` | `event.rs:293,294` | decoder tests only |
| `planning with claude -p` \| `codex exec` \| `opencode run` \| `kimi --print` (4 strings) | each adapter (`agent-*/src/lib.rs`) | `event.rs:201-204` | decoder tests only |
| `executing with …` (5 strings: interactive PTY, headless loop, codex, opencode, kimi) | each adapter | `event.rs:225-229` | decoder tests only |
| `IDLE_REAPED_MSG` | **shared const** `adapter-support/src/idle.rs:34`, used by both execution paths | `event.rs:298` | **closed loop**: same constant on both ends + level-contract test |

**Reading of the table:** exactly **1 of 30** strings has its emit↔decode loop
closed by construction (`IDLE_REAPED_MSG` — and the comment at `event.rs:296`
states the rationale: "so the two emitters cannot drift apart"). For the other
29, the decoder tests pin *the decoder given the string*, never *that the
emitter emits that string with those field names at that level*. Renaming a
message, a field (`up`/`cr`/`cw`/`out`, `steps_json`, …), or logging at WARN
instead of INFO compiles clean, passes tests, and silently blinds console,
Telegram, and CloudEvents at once.

This failure mode is not hypothetical — the repo carries its scar tissue: the
kimi decoder test (`event.rs:725`) exists because "without them the live line,
the Telegram card, and the heartbeat phase all stay stuck on planning". Every
new adapter must remember to register two decoder arms (F5). And field encoding
already varies per adapter: claude emits `model = %model` (Display), opencode
emits `model = ?self.model` (Debug of an `Option`) — exactly the class of
skew a round-trip gate would catch mechanically.

---

## 3. Findings

### F1 — The event vocabulary has no owner *(contract-level, root cause)*
As per §2: the run's entire observable contract is 30 scattered string literals
plus free-form field names, matched by one decoder in another crate, with the
loop closed for 1 of 30. The emit side is unergonomic (`info!` with 8 hand-typed
fields at `run.rs:328`), which is *why* F2 and F3 happened: printing was easier
than emitting, and the presenter grew its own state.

### F2 — Border outcomes never reach the bus *(contract bug)*
Two exits produce **zero delivered events**:

- **Empty queue** (`run.rs:167-178`): returns before `emit_queue_built`
  (`run.rs:199`), before `run started` (`run.rs:328`), and — decisively — before
  the workers start (`try_start_notifier` `run.rs:225`, `try_start_sink`
  `run.rs:283`). Even the events already buffered in the rings are never drained.
- **`--if-idle` skip** (`run.rs:76-86`): does `info!("{msg}")` with a *dynamic*
  message → decoder returns `None` → invisible to every sink; and again no
  worker ever starts.

Consequence for the events platform (ADR-0019/0020, issues #93/#94): a
scheduled AFK run that found no work, or that deferred to a live run, is
**indistinguishable from Ralphy never having run**. The Telegram operator gets
nothing either. This breaks the ADR-0019 liveness semantics (`run.finished`
only on clean termination; absence = death) in the opposite direction: clean
terminations that emit nothing.

Related console-only borders (deliberate presentation, *not* defects, kept
imperative on purpose): `print_header` (`run.rs:162`), `print_info_line`
(`run.rs:165`), final panel (`report.rs:191`).

### F3 — Three reducers over one stream; one observable divergence *(structural)*
`RunState` is instantiated by the Telegram and CloudEvents engines. The
presenter instead maintains `LiveState`/`ActiveIssue` (`ui/presenter.rs:33,51`)
and `QueueState` (`ui.rs:80`). Semantic diff, verified field by field:

| Fact | Presenter | `RunState` | Divergence |
|---|---|---|---|
| Supersede (new `IssueStarted` while prior active issue is non-terminal — dry-run plan-only) | `QueueState::supersede` advances the bar (`presenter.rs:227`) | **absent** — the prior issue stays `Planning` forever | **Live bug**: in a `--dry-run` multi-issue run the Telegram card and the heartbeat's `counts().planning` count already-passed issues as active, forever |
| `stop_before` | kept, marks the cut on the pending bar (`ui.rs:139`) | **dropped** by `apply(QueueBuilt)` (`state.rs:206`) | remote consumers can't see the announced halt point except inside `queue.built` data |
| Queue order / pending list | kept (`QueueState.pending`) | partial (`queue: Vec<QueueRef>` has order, no progress) | two progress derivations |
| Per-issue plan usage stash, budget_min, wall-clock start | kept (`ActiveIssue`) | dropped | usage/budget are facts, not render state; today only the console can show issue totals |
| Consolidating/consolidated, agents, plan snapshots | absent | kept | presenter can't render these from its own state |

Three copies of "what happened" = three places any new lifecycle event must be
hand-threaded, and the guaranteed cost of a fourth sink (the ADR-0032
daemon/workbench will be one) is a fourth reducer.

### F4 — Final summary computed twice; `run.finished` internally incoherent *(debt)*
`emit_run_finished` (`report.rs:56-87`) and `render_final_panel`
(`report.rs:94-192`) re-derive done/skipped/blocked/hitl from the same
`QueueReport` with duplicated predicates; the panel has `blocked`/`hitl`, the
event does not. Worse: inside the one `run.finished` envelope, the scalars come
from `QueueReport` while `data.issues[]` is re-derived from the folded
`RunState` (`envelope.rs:471`); the ring is drop-oldest **by design**, so the
two can legally disagree within the same payload.

### F5 — Per-adapter decoder arms *(debt, feeds F1)*
9 of the 30 strings exist only to say *which* adapter is planning/executing;
the adapter identity is already a field-worthy datum. Every new adapter must
edit `event.rs` twice or silently break three sinks (the kimi scar, §2).

### F6 — Second envelope-delivery implementation *(minor)*
`ralphy issues --push` POSTs `queue.snapshot` synchronously via
`UreqEventTransport::post` directly (`issues.rs:290`), outside the delivery
spine — no retry, no warn-gate. (The *data builder* is correctly shared:
`queue_snapshot_data`, `envelope.rs:151`, the ADR-0020 anti-drift pattern.)

### Explicitly examined and judged healthy / not worth touching
- The tracing-as-bus decision (ADR-0006/0007) — cheap, decoupled, keeps
  adapters free of CLI types. **Do not replace.**
- The `delivery` spine (ADR-0024) — exactly right; needs no change for F1–F4.
- Emoji/label tables duplicated console vs Telegram — legitimately distinct
  presentations; unifying is cosmetic coupling. **Non-goal.**
- `plan.step` file-polling (`events/sink/poller.rs`) — deliberate design;
  the `reset_from_written` reconciliation is fragile but tested. **Non-goal.**
- `stop_before` dual computation — already mitigated by the shared predicate
  `ralphy_core::first_stop_before` (`run.rs:188`). Healthy pattern.

---

## 4. Why this is worth doing (impact, stated plainly)

- F2 is a **bug in the platform contract**, small in code, high in consequence:
  today "no events" cannot be trusted to mean "not running".
- F1 is the **structural mine**: this repo is developed largely by agents, and
  the contract is invisible to the compiler and to CI. The `IDLE_REAPED_MSG`
  precedent proves the fix pattern works and is idiomatic here.
- F3 has one **live observable bug** (dry-run stale `planning` on the Telegram
  card/heartbeat) and is the direct tax on the daemon/workbench sink (#185+,
  ADR-0032 Phase 1), which otherwise needs reducer #4.
- F4–F6 are hygiene that falls out almost free once F1's builder pattern exists.

If only F4–F6 existed, the honest answer would be "leave it alone".

---

## 5. Definitive proposal — "one vocabulary, one fold"

No new architecture. Four phases, strictly ordered, each independently
shippable and revertible, each landing green (`cargo fmt --check`,
`clippy -D warnings`, `cargo test`, both OSes).

### Phase 0 — Characterization harness (the no-regression mechanism)
Before touching any emit site, add to `ralphy-cli` a test-only capturing
subscriber (a `Layer` that records `(level, target, message, EventFields)`)
and **characterization tests** that drive the *current* emitters where feasible
and pin today's `(message, fields, level)` triples. This is the safety net every
later phase runs under; it also becomes the permanent round-trip gate.

### Phase 1 — Give the vocabulary an owner (fixes F1, F5)
1. New module `ralphy_core::emit` (name per ADR): **one typed emit function per
   lifecycle event** — `emit::issue_started(number, title)`,
   `emit::queue_built(…)`, `emit::planning(cmd, model, effort)`, … Each owns its
   message constant, its field names, and its level. Core's runner, the CLI's
   boundary emissions, and the adapters all call these instead of raw `info!`.
   (Dependency-wise this works: everything already depends on `ralphy-core`.)
2. Collapse the 9 per-adapter strings into two messages — `planning` /
   `executing` — with the human-readable command as a `cmd` field. The decoder
   drops 7 arms; adding an adapter no longer touches `event.rs`. (Cosmetic log
   change: `planning cmd="codex exec" …` instead of `planning with codex exec`;
   `ralphy.log` is not a stable contract, `docs/events.md` is unaffected.)
3. The decoder's `match` arms switch to the same constants.
4. **The gate:** one round-trip test per `RunEvent` variant — call the emit
   helper under the capturing subscriber, decode via `event_to_runevent`,
   assert the exact variant. From then on, any drift in message, field name,
   field encoding (`%` vs `?` — see §2), or level is red in CI. This
   generalizes the `IDLE_REAPED_MSG` pattern from 1/30 to 30/30.

Wire format: byte-identical. `docs/events.md`: unchanged.

### Phase 2 — Borders become events (fixes F2)
1. Restructure `run_cmd` so the empty-queue check runs **after** the workers can
   start (title is already derived by then), and emit:
   `queue built` (count 0, scoped) → `run started` → `run finished`
   (`issues_total: 0`, new outcome label, e.g. `no_work`) — then tear down
   normally. The Telegram card for a scheduled run that found nothing is a
   feature, not noise.
2. `--if-idle` deferral: new `RunEvent::RunSkipped { reason }` → CloudEvent
   `dev.ralphy.run.skipped`; the console notice becomes a render of that event
   (delete the imperative `print_notice` at `run.rs:84`).
3. Amend ADR-0019 + `docs/events.md` (additive: one new type, one new outcome).
4. `print_header`/`print_info_line`/final panel stay imperative — presentation,
   already covered by `run.started.data.git`.

### Phase 3 — One fold (fixes F3)
1. Move supersede semantics into `RunState::apply` (design point for the ADR:
   a terminal status for a plan-only pass, e.g. `Planned`, wire-additive) —
   this alone fixes the dry-run Telegram/heartbeat bug **for free in all sinks**.
2. Grow `RunState` with the facts the presenter keeps privately and drops today:
   `stop_before`, `budget_min`, per-issue usage stash. (Wall-clock `Instant`s
   stay presenter-local — arrival-time is render state, not event data.)
3. The presenter's render thread folds `RunState` like the other two engines and
   derives its view (progress bar, active line) from it; delete `QueueState`
   and the fact-tracking parts of `ActiveIssue`/`LiveState`.
   Instances stay per-sink (threads are independent); what unifies is the code.
4. No-regression: golden-render tests — feed fixed `RunEvent` sequences, assert
   `render_line`/bar-label output identical before/after (the render fns already
   have this style of test).

### Phase 4 — One summary builder (fixes F4)
1. `RunSummary::from(&QueueReport)` built once in `report.rs`; both the panel
   and `emit::run_finished` consume it. `blocked`/`hitl` join the event
   (additive).
2. `run finished` carries its own issues rollup (an `issues_json` field, the
   `queue built` precedent) derived from the same `RunSummary`; the envelope
   prefers it and falls back to the fold — the scalars and the array can no
   longer disagree inside one payload.

### Deferred / non-goals (deliberate)
- Replacing tracing with a typed bus; touching the `delivery` spine.
- Unifying emoji/label tables; reworking the `plan.step` poller.
- Routing `issues --push` through the spine (F6) — the command is synchronous
  by design; at most extract the retry helper later.

### ADR work that follows this audit (in order)
1. New ADR: "The event vocabulary lives in `ralphy-core::emit`" (Phase 1) —
   supersedes the implicit string contract; cites the `IDLE_REAPED_MSG` precedent.
2. Amendment to ADR-0019: border events (`run.skipped`, `no_work` outcome,
   empty-queue emission ordering) + the `run.finished` self-consistency rule
   (Phase 2/4).
3. Amendment to ADR-0007 D6 (or new): "the presenter folds `RunState`;
   supersede is a fold concern" (Phase 3).

### Effort & risk matrix

| Phase | Blast radius | Risk | Mitigation |
|---|---|---|---|
| 0 | test-only | none | — |
| 1 | every emit site (mechanical), decoder arms | low — behavior pinned by Phase 0 + round-trips | wire byte-identical; revert = revert one module |
| 2 | `run_cmd` orchestration order | medium — worker start order is subtle (`run.rs:194-199` comment) | events.md additive; characterization of teardown order; manual `--dry-run`/empty-queue smoke on both OSes |
| 3 | presenter internals, `RunState` growth | medium — richest UI surface | golden-render tests; `RunState` changes are additive (Telegram/CloudEvents folds must stay byte-stable — pinned by existing fold tests) |
| 4 | `report.rs` + envelope | low | additive fields; existing envelope tests |
