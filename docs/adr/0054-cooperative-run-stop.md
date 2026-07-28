# Cooperative run stop: the operator asks, the run decides

Status: accepted.

The workbench gains a **Stop** button, and the daemon still never kills anything.
Stop dispatches a second, short-lived `ralphy stop`, which writes a **runid-scoped
sentinel** beside the target run's snapshot document. The **run** notices its own
sentinel on the 250 ms snapshot tick, raises a process-global flag, reaps its own
vendor child, and unwinds through the ordinary teardown. Vocabulary
(**cooperative stop**) lives in [CONTEXT.md](../../CONTEXT.md).

## Why now

Starting a run from the workbench was a one-way door. `startRun()` dispatches
`ralphy run` over `/ws/command`, and the dispatched child is spawned with
`.stdin(Stdio::null())` (`dispatch.rs`), so there was no channel to deliver a
Ctrl-C even if a button had existed. An operator who started the wrong run
watched it burn tokens for up to an hour.

The obvious fix — let the daemon kill its child — is forbidden on purpose:
`dispatch.rs`'s TEARDOWN INVARIANT ("the daemon must NEVER kill it… Do not add a
kill to any arm"), `Verb::from_query` rejecting `kill`/`stop` with a test pinning
it, and [ADR-0032](0032-daemon-mode-supervised-launcher.md) §5/§6, which record
why: a dispatched run keeps its own lifecycle, so a daemon crash never kills a
run.

ADR-0032 §6 also pre-authorized the way out — *"Deliberate exclusion: no remote
kill… If remote kill ever justifies itself, it is its own decision with a strong
confirmation, never a v1 fat-finger."* This is that decision, and it turns out
not to need a remote kill at all.

## D1 — The daemon signals nothing; the run stops itself

Stop is a **Mutate** verb (`run.stop`), not a Spawn and not a signal. It composes
a fixed `ralphy stop --runid=<id>` and collects it, exactly as `sync push` does.
That process writes a file and exits. The daemon never touches the run.

Consequence: ADR-0032 §6's "no remote kill" exclusion **stands verbatim and is
not weakened**. The daemon writes no repo state (ADR-0036 §2's division rule:
writing repo state is a `ralphy` invocation), signals no process, and the run's
tree and branch are left in the *same* documented state a non-green stop leaves
them in — which is the exact harm §6 cited. The "strong confirmation" clause is
honoured by a `confirm()` on the button.

The bare strings `stop` and `kill` remain unrepresentable in `Verb::from_query`,
and its test still pins that. Do not relax it to accommodate `run.stop`.

**In the UI it is one control with two faces, not a second button.** The Runs
toolbar's first slot is `run` while the project is idle and `stop` while it has a
live run. A separate Stop button would mean two controls of which one is always
dead — the `run` face is `verbLocked()`-gated precisely because a live run
refuses a second one, so the slot is free exactly when stop is meaningful. The
`stop` face is deliberately NOT gated by `verbLocked()`: that gate reads "a run
holds the lock", which is the condition stop exists for. The state is derived
from the same snapshot-backed run list the lock note reads, so the two can never
disagree, and it returns to `run` on its own when the run's document is removed
at exit — no client-side bookkeeping.

## D2 — The channel is a runid-scoped sentinel in the snapshot directory

`<repo>/.ralphy/runstate/<runid>.stop`.

**Why there:** it is the only place runs are already addressable by `runid`; it
is already gitignored; it is already watched; and `list_runs` skips every
non-`.json` entry, so the sentinel is invisible to the reader *by construction*
rather than by a filter someone has to remember. A test pins that.

**Why runid-scoped:** a repo can host concurrent runs (`run.rs` lets them
proceed; `list_runs` returns a `Vec`), so an unscoped sentinel could not name a
target — and a stale one would kill the *next* run at its first tick. Scoping
makes an orphaned sentinel inert.

**The read is existence-only.** The file carries `{requested_at, by_pid}` for
forensics, but a torn or truncated write must never be the difference between
stopping and not.

**Cleanup, both ends:** `SnapshotGuard` removes the sentinel alongside the
document at exit, and `list_runs`' dead-pid sweep removes the sibling `.stop`
when it removes an orphaned document.

## D3 — The flag is a process-global atomic in `ralphy_core::stop`

Six sites must read it, and they share no seam:

| Site | What it prevents |
|---|---|
| `runner.rs` queue loop | starting the next issue |
| `runner.rs` post-execute guard | spending minutes in the protocol lint + verify gate after a green execute |
| `phases.rs` resume loop | calling `execute()` **again** when a stop-killed child classifies as `Limit` |
| `clock.rs` `wait_for_reset` | an unbounded usage-limit wait ignoring the stop for days |
| `adapter-support` headless pump | the vendor child outliving the request |
| `agent-claude` PTY pump | the same, on the interactive path |

Three of those sit on the far side of the [ADR-0002](0002-core-agnostic-adapter-boundary.md)
boundary and never see a `RunClock`, a `QueueConfig`, or anything else core
injects — and they are the ones that matter, because the case being stopped is a
wedged four-hour child.

**Rejected: a `RunClock::stop_requested()` method.** It reaches two of the six
sites and would then have to coexist with a global anyway — two mechanisms for
one job.

**Rejected: threading an `Arc<AtomicBool>`.** It would cross `QueueConfig` (no
`Default`, 8 construction sites), `IssueBudget` (breaking its `Copy`, 6
adapters), Claude's separate exec config, and one `HeadlessCall` builder per
adapter — strictly *more* indirection for identical behaviour. The global is the
opposite of an abstraction; CLAUDE.md's `anti-over-abstraction` rule names
traits, generics, crates and layers of indirection, and this is none of them.

A `ralphy run` process drives exactly one run, so "this process was asked to
stop" is a process-wide fact, modelled as one. It is also the shape a future
Ctrl-C handler requires: an async-signal handler may touch an atomic and nothing
else.

Core knows no path. `request()`/`requested()`/`clear()`, and `ralphy-cli` — which
already depends on both core and the snapshot crate — owns the file→flag
translation. All thirteen headless entry points funnel through one poll loop, so
every vendor gained a working stop without a builder change.

### The test hazard, and the standing rule

`cargo nextest` runs each test in its own process; `cargo test` shares one
process **per test binary** and runs its tests on several threads. Both gate this
repo, so the weaker runner sets the rule:

> **Stop tests live in their own `tests/*.rs` binary, serialized behind a static
> mutex, and clear the flag through a guard that survives a panicking assertion.**

A leaked `true` would stop or reap every test that ran after it *as a timing
flake, not as an error*. `crates/ralphy-core/tests/stop.rs` and
`crates/ralphy-adapter-support/tests/stop.rs` exist for exactly this reason; no
stop test may join `queue.rs` or `headless.rs`.

## D4 — The setter is the snapshot delivery worker's tick

`SnapshotEngine::on_tick` already runs at 250 ms, already does an mtime-gated
file poll, and is the one component holding both `runid` and `repo_root`. It
latches: once the flag is up, the `exists()` syscall costs nothing for the rest
of the run, and a sentinel deleted mid-flight cannot un-stop the run.

No new thread, no new cadence.

**Consequence, stated rather than discovered:** `try_start_snapshot` returns
`Option<WorkerHandle>`. No worker ⇒ no poller ⇒ that run is unstoppable. It is
self-consistent — a run with no worker writes no snapshot document, so it never
appears in the Runs panel and has no Stop button — but it is a real hole and it
is written down here.

## D5 — A stop is a `StopReason`, never an `Outcome`

This is the load-bearing sentence of the ADR.

The runner's teardown matrix hands the branch back for every `Some(_)` stop and
force-checks-out the *original* branch when `stop` is `None` — and
`git checkout -f` destroys uncommitted tracked changes silently. A stop modelled
as anything else (an early return, a bool, "queue exhausted") would delete the
work the run had in flight. `a_stop_leaves_the_run_branch_checked_out_with_uncommitted_work_intact`
is that decision's regression guard.

`StopReason::Stopped { number: Option<u64> }` — `None` when the stop landed
between issues. It outranks `deadline_cut` and the outcome→reason mapping in the
non-green arm, because a stop-killed child ends with no verdict and would
otherwise report `Stuck`: telling the operator their agent got wedged when in
fact they pressed a button.

**No `Outcome::Stopped`, no `SkipReason::Stopped`.** An `Outcome` is the
*adapter's* verdict on an issue, and no adapter learns a button was pressed.

## D6 — What the issue in flight gets, and what the ledger says

Gates skipped, commits kept on the branch, issue left **open**, recorded as
worked-but-not-delivered (`ResultStatus::NonGreen`). Closing it would be the run
vouching for work no gate ever checked.

The ledger line is written by `execute_phase` before it returns. A stop-killed
child gives `exit: None`, `timed_out: false`, `done: false`, so the ADR-0023
ladder lands on `Outcome::Stuck` — *tokens spent, attempt achieved nothing*. That
is correct, and **retry burn** counting it is also correct: you paid for a
delivery you did not get. The operator-facing truth lives in the run report, the
panel line and the `run.stopped` event — never in the ledger's per-attempt
column.

**The ADR-0023 classification ladder is untouched.** It was examined and left
alone; this paragraph exists so a later reader knows that was a decision.

`HeadlessOutput.stopped` / `HeadlessRun.stopped` are added as diagnostics, the
way `idle_killed` was — but unlike `idle_killed`, `timed_out` is deliberately
**not** set alongside, mirroring the early-kill switch. A stop must never read as
a timeout to a caller or a test, and it must never reach `CompletionSignals`.

The verify gate is the one place a stop *is* reported as `timed_out`:
`CommandOutcome` has no third state and a stopped gate genuinely did not pass.
Nothing downstream misreads it, because the run is already unwinding on a
`StopReason` by the time that outcome is folded.

## D7 — `ralphy stop` is the one Mutate that skips the run lock

Every other write verb refuses under `LockState::HeldAlive` (ADR-0036 §6). This
one exists precisely to act *while* a run holds the lock; guarding it would make
it refuse in the only situation it is for. `runlock.rs`'s own refusal message
already says "wait for it to finish or **stop it**" — this is that.

## D8 — The exit-code contract, and no `--wait`

- **Nothing to stop ⇒ exit 0.** The operator asked for "this repo is not
  running", and that is already true. Same reasoning as `run --if-idle`'s clean
  deferral: a scheduler's history — and a Stop button — must not fill with false
  failures for a race they cannot avoid.
- **Ambiguity (>1 live run, no `--runid`) ⇒ non-zero**, listing the runids.
  Stopping is not reversible in the way that matters, so guessing "probably the
  newest" is the wrong kind of helpful.
- **A `--runid` that is not live ⇒ non-zero.** Collapsing it into the clean
  "nothing to stop" would let a typo report success while the run kept going.
- **`--format json` is for humans, CI and `jq`, never for the workbench**: the
  daemon's Mutate branch collapses exit 0 to `{"status":"ok"}` and discards
  stdout.

**No `--wait`, by decision.** The Mutate branch is synchronous over one socket;
blocking it would hold the UI for the tick + a vendor process-tree kill + the 5 s
collect grace, with no ceiling. Confirmation already rides a channel that exists:
`SnapshotGuard` deletes the document at exit, the runstate watcher fires
`runs.dirty`, and **the run leaving the panel is the confirmation**.

The verb requires `runid` even though the CLI can infer it for a single-run repo:
the browser is always addressing a run it can see, and inferring on its behalf
would let a click land on a run that started between the render and the request.

## Deliberately not built

No `--wait`. No `--all`. No cross-repo stop. No escalation to a kill, and no
`stop --force`. No stop for a run with no snapshot worker (see D4).

**The Ctrl-C handler is a follow-up, not part of this.** It would reverse two
*recorded* decisions (`runlock.rs`'s "deliberately no Ctrl-C/signal handler",
[ADR-0047](0047-run-state-snapshot-channel.md) §8), it changes the failure
envelope of every subcommand and the daemon, and it needs its own escape-hatch
design (a second Ctrl-C must still hard-kill). Deferring also makes it far
smaller: with this ADR landed, the handler's whole body is
`ralphy_core::stop::request()`, and the stale `run.lock` + orphan
`runstate/<runid>.json` that a Ctrl-C leaves today would start being cleaned up
by the guards that already exist.

## Consequences

- The workbench can stop a run in about a second (spawn + a ≤250 ms tick + a
  ≤500 ms poll), and the run then unwinds through teardown.
- Every vendor adapter gained a stop with no per-adapter change, because they all
  drive children through one poll loop.
- `ralphy_core` gained one process-global. It is the first, and the test rule in
  D3 is the price of it.
- `run.finished` gained the `outcome` value `stopped`, and a new event type
  `dev.ralphy.run.stopped` — both additive under docs/events.md evolution rule 2.
